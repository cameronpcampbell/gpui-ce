use std::{sync::Arc, time::Duration};

use gpui::{DevicePixels, Scene, Size};

use crate::{WgpuAtlas, WgpuContext};

use super::{WgpuRenderer, WgpuSurfaceConfig};

struct OffscreenTarget {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    readback: wgpu::Buffer,
    padded_bytes_per_row: u32,
    size: Size<DevicePixels>,
}

impl WgpuRenderer {
    pub(super) fn new_headless(
        context: &WgpuContext,
        size: Size<DevicePixels>,
    ) -> anyhow::Result<Self> {
        let mut renderer = Self::new_internal(
            None,
            context,
            None,
            WgpuSurfaceConfig {
                size,
                transparent: false,
                preferred_present_mode: None,
            },
            None,
            None,
            Arc::new(WgpuAtlas::from_context(context)),
        )?;

        renderer.target.clear_color = wgpu::Color::BLACK;

        Ok(renderer)
    }

    fn create_offscreen_target(&self) -> OffscreenTarget {
        let width = self.target.width();
        let height = self.target.height();
        let padded_bytes_per_row = (width * 4).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
        let texture = self
            .resources()
            .device
            .create_texture(&wgpu::TextureDescriptor {
                label: Some("gpui_offscreen_target"),
                size: wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: self.target.format(),
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let readback = self
            .resources()
            .device
            .create_buffer(&wgpu::BufferDescriptor {
                label: Some("gpui_offscreen_readback"),
                size: u64::from(padded_bytes_per_row) * u64::from(height),
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            });
        OffscreenTarget {
            texture,
            view,
            readback,
            padded_bytes_per_row,
            size: self.viewport_size(),
        }
    }

    fn read_offscreen_target(
        &self,
        target: &OffscreenTarget,
        submission: wgpu::SubmissionIndex,
    ) -> anyhow::Result<image::RgbaImage> {
        let width = target.size.width.0 as u32;
        let height = target.size.height.0 as u32;
        let bytes_per_row = width * 4;
        let (sender, receiver) = std::sync::mpsc::sync_channel(1);
        target
            .readback
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |result| {
                let _ = sender.send(result);
            });
        self.resources()
            .device
            .poll(wgpu::PollType::Wait {
                submission_index: Some(submission),
                timeout: None,
            })
            .map_err(|error| anyhow::anyhow!("failed to poll offscreen readback: {error}"))?;
        receiver
            .recv()
            .map_err(|error| anyhow::anyhow!("offscreen readback callback dropped: {error}"))?
            .map_err(|error| anyhow::anyhow!("failed to map offscreen readback: {error}"))?;

        let mapped = target.readback.slice(..).get_mapped_range();
        let mut pixels = Vec::with_capacity(bytes_per_row as usize * height as usize);
        for row in mapped.chunks_exact(target.padded_bytes_per_row as usize) {
            pixels.extend_from_slice(&row[..bytes_per_row as usize]);
        }
        drop(mapped);
        target.readback.unmap();
        if self.target.format() == wgpu::TextureFormat::Bgra8Unorm {
            for pixel in pixels.chunks_exact_mut(4) {
                pixel.swap(0, 2);
            }
        }
        image::RgbaImage::from_raw(width, height, pixels)
            .ok_or_else(|| anyhow::anyhow!("offscreen readback dimensions did not match its data"))
    }

    /// Renders through the normal scene path and reads back without presenting.
    pub fn render_to_image(&mut self, scene: &Scene) -> anyhow::Result<image::RgbaImage> {
        let target = self.create_offscreen_target();
        let submission = self
            .render_to_view_with_readback(scene, &target.view, target.readback_copy())
            .ok_or_else(|| anyhow::anyhow!("failed to render scene into the offscreen target"))?;
        self.read_offscreen_target(&target, submission)
    }
}

impl OffscreenTarget {
    fn readback_copy(&self) -> super::frame::ReadbackCopy<'_> {
        super::frame::ReadbackCopy {
            texture: &self.texture,
            buffer: &self.readback,
            bytes_per_row: self.padded_bytes_per_row,
            width: self.size.width.0 as u32,
            height: self.size.height.0 as u32,
        }
    }
}

/// Surface-free renderer used by GPUI visual tests and benchmarks.
pub struct WgpuHeadlessRenderer {
    renderer: WgpuRenderer,
    target: Option<OffscreenTarget>,
}

impl WgpuHeadlessRenderer {
    pub fn new() -> anyhow::Result<Self> {
        let context = WgpuContext::new_headless(None)?;

        Self::with_context(&context)
    }

    pub fn with_context(context: &WgpuContext) -> anyhow::Result<Self> {
        let renderer = WgpuRenderer::new_headless(
            context,
            Size {
                width: DevicePixels(1),
                height: DevicePixels(1),
            },
        )?;
        Ok(Self {
            renderer,
            target: None,
        })
    }

    pub fn measure_gpu_time(
        &mut self,
        scene: &Scene,
        size: Size<DevicePixels>,
    ) -> anyhow::Result<Duration> {
        self.ensure_target(size)?;
        let resources = self.renderer.resources();
        let device = &resources.device;
        anyhow::ensure!(
            device.features().contains(wgpu::Features::TIMESTAMP_QUERY),
            "GPU timing requires TIMESTAMP_QUERY"
        );
        let requirements = scene.render_plan().requirements();
        anyhow::ensure!(
            !requirements.uses_path_target && !requirements.uses_offscreen_target,
            "GPU timing requires a scene rendered in one pass"
        );
        let timestamps = device.create_query_set(&wgpu::QuerySetDescriptor {
            label: Some("headless_frame_timestamps"),
            ty: wgpu::QueryType::Timestamp,
            count: 2,
        });
        let resolve = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("headless_timestamp_resolve"),
            size: 16,
            usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let target = self.target.as_ref().expect("target was just ensured");
        let submission = super::frame::render_to_view_with_timestamps(
            &mut self.renderer,
            scene,
            &target.view,
            None,
            Some(&timestamps),
        )
        .ok_or_else(|| anyhow::anyhow!("failed to render timed frame"))?;
        let resources = self.renderer.resources();
        resources.device.poll(wgpu::PollType::Wait {
            submission_index: Some(submission),
            timeout: None,
        })?;
        let mut encoder =
            resources
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("headless_timestamp_readback"),
                });
        encoder.resolve_query_set(&timestamps, 0..2, &resolve, 0);
        resources.queue.submit([encoder.finish()]);

        let (sender, receiver) = std::sync::mpsc::sync_channel(1);
        wgpu::util::DownloadBuffer::read_buffer(
            &resources.device,
            &resources.queue,
            &resolve.slice(..),
            move |result| {
                let _send_result = sender.send(result);
            },
        );
        resources.device.poll(wgpu::PollType::wait_indefinitely())?;
        let mapped = receiver.recv()??;

        let start = u64::from_ne_bytes(mapped[..8].try_into()?);
        let end = u64::from_ne_bytes(mapped[8..].try_into()?);
        let nanoseconds =
            end.saturating_sub(start) as f64 * f64::from(resources.queue.get_timestamp_period());

        anyhow::ensure!(
            nanoseconds > 0.0,
            "GPU returned an empty timestamp interval: {start} -> {end}"
        );

        Ok(Duration::from_secs_f64(nanoseconds * 1e-9))
    }

    fn ensure_target(&mut self, size: Size<DevicePixels>) -> anyhow::Result<()> {
        anyhow::ensure!(
            size.width.0 > 0 && size.height.0 > 0,
            "headless render target must have positive dimensions"
        );
        if self
            .target
            .as_ref()
            .is_some_and(|target| target.size == size)
        {
            return Ok(());
        }
        self.renderer.update_drawable_size(size);
        self.target = Some(self.renderer.create_offscreen_target());
        Ok(())
    }

    /// Renders through the normal submission path and waits for that work to finish.
    pub fn render_scene_and_wait(
        &mut self,
        scene: &Scene,
        size: Size<DevicePixels>,
    ) -> anyhow::Result<()> {
        self.ensure_target(size)?;
        let target = self.target.as_ref().expect("target was just ensured");
        let submission =
            super::frame::render_to_view(&mut self.renderer, scene, &target.view, None)
                .ok_or_else(|| anyhow::anyhow!("failed to render headless scene"))?;
        self.renderer
            .resources()
            .device
            .poll(wgpu::PollType::Wait {
                submission_index: Some(submission),
                timeout: None,
            })
            .map_err(|error| anyhow::anyhow!("failed to wait for headless render: {error}"))?;
        Ok(())
    }
}

impl gpui::PlatformHeadlessRenderer for WgpuHeadlessRenderer {
    fn render_scene_to_image(
        &mut self,
        scene: &Scene,
        size: Size<DevicePixels>,
    ) -> anyhow::Result<image::RgbaImage> {
        self.ensure_target(size)?;
        let target = self.target.as_ref().expect("target was just ensured");
        let submission = self
            .renderer
            .render_to_view_with_readback(scene, &target.view, target.readback_copy())
            .ok_or_else(|| anyhow::anyhow!("failed to render headless scene"))?;
        self.renderer.read_offscreen_target(target, submission)
    }

    fn render_scene(&mut self, scene: &Scene, size: Size<DevicePixels>) -> anyhow::Result<()> {
        self.ensure_target(size)?;
        let target = self.target.as_ref().expect("target was just ensured");
        anyhow::ensure!(
            self.renderer.render_to_view(scene, &target.view),
            "failed to render headless scene"
        );
        Ok(())
    }

    fn sprite_atlas(&self) -> Arc<dyn gpui::PlatformAtlas> {
        self.renderer.sprite_atlas().clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{RendererTier, WgpuDeviceRequirements};
    use gpui::{
        Bounds, ContentMask, Corners, Edges, FilterBoundary, PlatformHeadlessRenderer, Quad,
        ScaledPixels, interleaved_liquid_scene, point, size,
    };

    #[test]
    fn gpu_timing_uses_executed_queries_and_requires_the_device_feature() {
        let mut scene = Scene::default();
        let bounds = Bounds::new(
            point(ScaledPixels(0.0), ScaledPixels(0.0)),
            size(ScaledPixels(32.0), ScaledPixels(32.0)),
        );
        scene.insert_primitive(Quad {
            bounds,
            content_mask: ContentMask {
                bounds,
                ..Default::default()
            },
            background: gpui::white().into(),
            ..Default::default()
        });
        scene.finish();
        let target = size(DevicePixels(32), DevicePixels(32));
        let mut renderer = WgpuHeadlessRenderer::new().unwrap();
        assert!(renderer.measure_gpu_time(&scene, target).is_err());
        let context = WgpuContext::new_headless(Some(&WgpuDeviceRequirements {
            features: wgpu::Features::TIMESTAMP_QUERY,
            ..Default::default()
        }))
        .unwrap();
        let mut renderer = WgpuHeadlessRenderer::with_context(&context).unwrap();

        for _sample in 0..3 {
            let elapsed = renderer.measure_gpu_time(&scene, target).unwrap();
            assert!(elapsed > Duration::ZERO && elapsed < Duration::from_secs(1));
        }

        let mut filtered = Scene::default();
        let boundary = FilterBoundary {
            order: 0,
            bounds,
            content_mask: scene.quads[0].content_mask,
            corner_radii: Default::default(),
            corner_smoothing: 0.0,
            filters: Default::default(),
            opacity: 0.5,
            is_start: true,
        };
        filtered.insert_primitive(boundary.clone());
        filtered.insert_primitive(scene.quads[0]);
        filtered.insert_primitive(FilterBoundary {
            is_start: false,
            ..boundary
        });
        filtered.finish();
        assert!(renderer.measure_gpu_time(&filtered, target).is_err());
    }

    #[test]
    fn liquid_data_texture_matches_storage_with_interleaved_component_draws() {
        let scene = interleaved_liquid_scene(
            Quad {
                bounds: Bounds::new(
                    point(ScaledPixels(10.0), ScaledPixels(20.0)),
                    size(ScaledPixels(30.0), ScaledPixels(36.0)),
                ),
                content_mask: ContentMask {
                    bounds: Bounds::new(
                        point(ScaledPixels(0.0), ScaledPixels(0.0)),
                        size(ScaledPixels(300.0), ScaledPixels(80.0)),
                    ),
                    ..Default::default()
                },
                background: gpui::rgba(0xff000080).into(),
                border_color: gpui::rgba(0x0080ff80).into(),
                border_widths: Edges::all(ScaledPixels(2.0)),
                corner_radii: Corners::all(ScaledPixels(8.0)),
                corner_smoothing: 0.6,
                ..Default::default()
            },
            100.0,
            38.0,
            gpui::rgb(0x101010).into(),
            None,
        );
        let mut images = Vec::new();

        for tier in [RendererTier::Modern, RendererTier::WebGl2] {
            let mut context = WgpuContext::new_headless(None).unwrap();
            context.force_renderer_tier_for_test(tier);
            let mut renderer = WgpuHeadlessRenderer {
                renderer: WgpuRenderer::new_headless(
                    &context,
                    size(DevicePixels(300), DevicePixels(80)),
                )
                .unwrap(),
                target: None,
            };

            images.push(
                renderer
                    .render_scene_to_image(&scene, size(DevicePixels(300), DevicePixels(80)))
                    .unwrap(),
            );
        }

        for (modern, downlevel) in images[0].pixels().zip(images[1].pixels()) {
            for channel_idx in 0..4 {
                assert!(
                    modern.0[channel_idx].abs_diff(downlevel.0[channel_idx]) <= 1,
                    "{modern:?} versus {downlevel:?}"
                );
            }
        }

        assert!(images[1].get_pixel(44, 38).0[0] > 100);
        assert!(images[1].get_pixel(144, 38).0[0] > 100);
        assert!(images[1].get_pixel(244, 38).0[0] > 100);
    }
}
