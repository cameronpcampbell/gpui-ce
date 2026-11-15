use super::{
    WgpuRenderer, begin_color_render_pass_with_timestamps,
    buffers::{InstanceSlice, InstanceTransport, InstanceUpload},
    filters::{FILTER_UNIFORMS_PER_COMPOSITE, FrameUniformRequirements},
    path_types,
};
use gpui::{
    FilterRenderTarget, MonochromeSprite, PolychromeSprite, PrimitiveBatch, Quad, RenderCommand,
    Scene, ScenePlanRequirements, Shadow, SubpixelSprite, Underline,
};
use gpui_render::TargetStack;
use gpui_render::blur::{FilterCompositeClip, FilterCompositeParameters};
use gpui_render::shaders::{
    common::{FontRasterizationUniforms, GlobalUniforms, ShaderBool},
    interface as shader_interface,
    liquid::LiquidCell,
};

pub(super) fn render_to_view(
    renderer: &mut WgpuRenderer,
    scene: &Scene,
    frame_view: &wgpu::TextureView,
    readback: Option<ReadbackCopy<'_>>,
) -> Option<wgpu::SubmissionIndex> {
    render_to_view_with_timestamps(renderer, scene, frame_view, readback, None)
}

pub(super) fn render_to_view_with_timestamps(
    renderer: &mut WgpuRenderer,
    scene: &Scene,
    frame_view: &wgpu::TextureView,
    readback: Option<ReadbackCopy<'_>>,
    timestamps: Option<&wgpu::QuerySet>,
) -> Option<wgpu::SubmissionIndex> {
    let Some(targets) = PreparedTargets::prepare(renderer, scene, frame_view) else {
        return None;
    };
    // Prune once against the complete scene. Doing this inside each draw batch would evict a
    // texture that reappears after an intervening primitive and recreate its platform view.
    renderer.retain_surface_cache(&scene.surfaces);

    match FrameEncoder::new(renderer, scene, targets).encode(readback, timestamps) {
        Ok((upload, command_buffer)) => Some(
            renderer
                .resources()
                .queue
                .submit(upload.into_iter().chain([command_buffer])),
        ),
        Err(DrawError::ExternalSurface) => None,
        Err(DrawError::CapacityPlanningInvariant) => {
            log::error!("frame storage exceeded its precomputed capacity");
            None
        }
        Err(DrawError::MissingIntermediateTarget) => {
            log::error!("frame preparation did not create a required intermediate target");
            None
        }
    }
}

pub(super) struct ReadbackCopy<'a> {
    pub(super) texture: &'a wgpu::Texture,
    pub(super) buffer: &'a wgpu::Buffer,
    pub(super) bytes_per_row: u32,
    pub(super) width: u32,
    pub(super) height: u32,
}

struct PreparedTargets {
    active: wgpu::TextureView,
    presentation: wgpu::TextureView,
    offscreen: Option<wgpu::TextureView>,
    instances: InstanceUpload,
}

impl PreparedTargets {
    fn prepare(
        renderer: &mut WgpuRenderer,
        scene: &Scene,
        frame_view: &wgpu::TextureView,
    ) -> Option<Self> {
        if !begin_frame(renderer) {
            return None;
        }

        let transport = renderer.resources().instances.transport();
        let baseline = FrameRequirements::for_scene(scene, transport);
        let resources = renderer.resources_mut();
        let overhead = baseline
            .storage_bytes
            .saturating_add(baseline.instance_batches.saturating_mul(256));
        let maximum = resources.instances.maximum_size().saturating_sub(overhead);
        resources.liquid.prepare(scene, maximum as usize);
        let requirements =
            FrameRequirements::with_liquid(scene, transport, resources.liquid.byte_len() as u64);
        let device = renderer.resources().device.clone();
        let resources = renderer.resources_mut();
        if !resources.instances.ensure_capacity(
            &device,
            &resources.bind_group_layouts,
            requirements.storage_bytes,
            requirements.instance_batches,
        ) {
            return None;
        }
        let instances = {
            let resources = renderer.resources();
            resources.instances.begin_upload(
                &resources.queue,
                requirements.storage_bytes,
                requirements.instance_batches,
            )?
        };
        if !renderer.ensure_uniform_capacity(requirements.uniforms) {
            return None;
        }

        if requirements.planned.uses_path_target {
            renderer.ensure_path_textures();
        }
        if requirements.planned.uses_offscreen_target {
            renderer.ensure_filter_textures(&requirements.planned);
        }
        write_shader_globals(renderer);

        if requirements.planned.uses_offscreen_target {
            let resources = renderer.resources();
            let offscreen = resources
                .scene_color_view
                .as_ref()
                .expect("blur texture preparation must create a scene target")
                .clone();
            Some(Self {
                active: offscreen.clone(),
                presentation: frame_view.clone(),
                offscreen: Some(offscreen),
                instances,
            })
        } else {
            Some(Self {
                active: frame_view.clone(),
                presentation: frame_view.clone(),
                offscreen: None,
                instances,
            })
        }
    }
}

fn begin_frame(renderer: &mut WgpuRenderer) -> bool {
    let Some(error) = renderer.faults.pending_error.lock().unwrap().take() else {
        renderer.faults.consecutive_failed_frames = 0;
        renderer.atlas.before_frame();
        return true;
    };

    renderer.faults.consecutive_failed_frames += 1;
    log::error!(
        "GPU error during frame (failure {} of 10): {error}",
        renderer.faults.consecutive_failed_frames
    );
    if renderer.faults.consecutive_failed_frames > 10 {
        panic!("too many consecutive GPU errors; last error: {error}");
    }
    if renderer.faults.consecutive_failed_frames > 5 {
        if let Some(resources) = renderer.resources.as_mut() {
            resources.invalidate_intermediate_textures();
        }
        renderer.atlas.clear();
        renderer.target.request_redraw();
        renderer.faults.consecutive_failed_frames = 0;
        return false;
    }

    renderer.atlas.before_frame();
    true
}

#[derive(Clone, Copy, PartialEq)]
pub(super) struct GlobalUniformState {
    globals: GlobalUniforms,
    path_globals: GlobalUniforms,
    font_rasterization: FontRasterizationUniforms,
}

fn write_shader_globals(renderer: &mut WgpuRenderer) {
    let font = renderer.rendering_params.font_rasterization;
    let font_rasterization = FontRasterizationUniforms {
        gamma_ratios: wgsl_rs::std::vec4f(
            font.gamma_ratios[0],
            font.gamma_ratios[1],
            font.gamma_ratios[2],
            font.gamma_ratios[3],
        ),
        grayscale_enhanced_contrast: font.grayscale_enhanced_contrast,
        subpixel_enhanced_contrast: font.subpixel_enhanced_contrast,
        uses_blue_green_red_subpixel_order: ShaderBool::from(
            renderer.subpixel_order == super::SubpixelOrder::BlueGreenRed,
        ),
        padding: 0,
    };
    let globals = GlobalUniforms {
        viewport_size: wgsl_rs::std::vec2f(
            renderer.target.width() as f32,
            renderer.target.height() as f32,
        ),
        premultiplied_alpha: ShaderBool::from(
            renderer.target.alpha_mode() == wgpu::CompositeAlphaMode::PreMultiplied,
        ),
        padding: 0,
    };
    let path_globals = GlobalUniforms {
        premultiplied_alpha: ShaderBool::Disabled,
        ..globals
    };
    let state = GlobalUniformState {
        globals,
        path_globals,
        font_rasterization,
    };
    if renderer.uploaded_globals == Some(state) {
        return;
    }

    let resources = renderer.resources();
    let globals_size = std::mem::size_of::<GlobalUniforms>();
    let font_size = std::mem::size_of::<FontRasterizationUniforms>();
    let upload_size = renderer.globals.font_offset + font_size as u64;
    let mut upload = resources
        .queue
        .write_buffer_with(
            &resources.globals_buffer,
            0,
            std::num::NonZeroU64::new(upload_size).expect("global uniforms are non-empty"),
        )
        .expect("global uniform upload must fit its buffer");
    upload.slice(..).fill(0);
    upload
        .slice(..globals_size)
        .copy_from_slice(shader_interface::bytes_of(&globals));
    upload
        .slice(
            renderer.globals.path_offset as usize
                ..renderer.globals.path_offset as usize + globals_size,
        )
        .copy_from_slice(shader_interface::bytes_of(&path_globals));
    upload
        .slice(
            renderer.globals.font_offset as usize
                ..renderer.globals.font_offset as usize + font_size,
        )
        .copy_from_slice(shader_interface::bytes_of(&font_rasterization));
    drop(upload);
    renderer.uploaded_globals = Some(state);
}

#[derive(Clone, Copy, Default)]
pub(super) struct FrameRequirements {
    storage_bytes: u64,
    /// Instance batches this frame; one downlevel range-uniform slot per batch.
    instance_batches: u64,
    pub(super) uniforms: FrameUniformRequirements,
    planned: ScenePlanRequirements,
}

impl FrameRequirements {
    pub(super) fn for_scene(scene: &Scene, transport: InstanceTransport) -> Self {
        Self::with_liquid(scene, transport, 0)
    }

    fn with_liquid(scene: &Scene, transport: InstanceTransport, liquid_bytes: u64) -> Self {
        let planned = scene.render_plan().requirements();
        let mut storage_bytes = liquid_bytes;
        let mut instance_batches = u64::from(liquid_bytes > 0);
        let mut reserve = |element_size: usize, count: usize| {
            if count > 0 {
                let stride = element_size as u64;
                storage_bytes = storage_bytes.next_multiple_of(transport.batch_alignment(stride));
                storage_bytes = storage_bytes.saturating_add(stride.saturating_mul(count as u64));
                instance_batches += 1;
            }
        };

        for command in scene.render_commands() {
            let RenderCommand::Batch(batch) = command else {
                continue;
            };
            match batch {
                PrimitiveBatch::Shadows { range, .. } => {
                    reserve(std::mem::size_of::<Shadow>(), range.len())
                }

                PrimitiveBatch::Quads { range, .. } => {
                    reserve(std::mem::size_of::<Quad>(), range.len())
                }

                PrimitiveBatch::Liquid { range, .. } => {
                    for _draw_idx in range.clone() {
                        reserve(std::mem::size_of::<Quad>(), 1);
                    }
                }
                PrimitiveBatch::Paths {
                    rasterization_vertex_count,
                    sprite_count,
                    ..
                } if *rasterization_vertex_count > 0 => {
                    reserve(
                        std::mem::size_of::<path_types::PathRasterizationVertex>(),
                        *rasterization_vertex_count,
                    );
                    reserve(std::mem::size_of::<path_types::PathSprite>(), *sprite_count);
                }
                PrimitiveBatch::Underlines(range) => {
                    reserve(std::mem::size_of::<Underline>(), range.len())
                }
                PrimitiveBatch::MonochromeSprites { range, .. } => {
                    reserve(std::mem::size_of::<MonochromeSprite>(), range.len())
                }
                PrimitiveBatch::SubpixelSprites { range, .. } => {
                    reserve(std::mem::size_of::<SubpixelSprite>(), range.len())
                }
                PrimitiveBatch::PolychromeSprites { range, .. } => {
                    reserve(std::mem::size_of::<PolychromeSprite>(), range.len())
                }
                PrimitiveBatch::Paths { .. }
                | PrimitiveBatch::Surfaces(_)
                | PrimitiveBatch::BackdropFilters(_)
                | PrimitiveBatch::FilterBoundary(_) => {}
            }
        }

        debug_assert_eq!(
            instance_batches as usize,
            planned.instance_batch_count + usize::from(liquid_bytes > 0)
        );

        Self {
            storage_bytes,
            instance_batches,
            uniforms: FrameUniformRequirements {
                filter_count: FILTER_UNIFORMS_PER_COMPOSITE
                    * (planned.backdrop_filter_count + planned.isolated_filter_count) as u64
                    + u64::from(planned.uses_offscreen_target)
                    + if planned.isolated_target_count > 0 {
                        (planned.backdrop_filter_count * (planned.isolated_target_count + 1)) as u64
                    } else {
                        0
                    },
                surface_count: planned.surface_count as u64,
            },
            planned: *planned,
        }
    }
}

struct FrameEncoder<'a> {
    renderer: &'a WgpuRenderer,
    scene: &'a Scene,
    encoder: wgpu::CommandEncoder,
    targets: TargetStack<wgpu::TextureView>,
    offscreen: Option<wgpu::TextureView>,
    presentation: wgpu::TextureView,
    instances: InstanceUpload,
}

impl<'a> FrameEncoder<'a> {
    fn new(renderer: &'a WgpuRenderer, scene: &'a Scene, targets: PreparedTargets) -> Self {
        let encoder =
            renderer
                .resources()
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("gpui_frame"),
                });
        Self {
            renderer,
            scene,
            encoder,
            targets: TargetStack::new(targets.active),
            offscreen: targets.offscreen,
            presentation: targets.presentation,
            instances: targets.instances,
        }
    }

    fn encode(
        mut self,
        readback: Option<ReadbackCopy<'_>>,
        timestamps: Option<&wgpu::QuerySet>,
    ) -> Result<(Option<wgpu::CommandBuffer>, wgpu::CommandBuffer), DrawError> {
        let result = self.encode_commands(timestamps);
        if result.is_ok() {
            if let Some(offscreen) = &self.offscreen {
                self.renderer
                    .blit_to_frame(&mut self.encoder, offscreen, &self.presentation);
            }
            if let Some(readback) = readback {
                self.encoder.copy_texture_to_buffer(
                    readback.texture.as_image_copy(),
                    wgpu::TexelCopyBufferInfo {
                        buffer: readback.buffer,
                        layout: wgpu::TexelCopyBufferLayout {
                            offset: 0,
                            bytes_per_row: Some(readback.bytes_per_row),
                            rows_per_image: Some(readback.height),
                        },
                    },
                    wgpu::Extent3d {
                        width: readback.width,
                        height: readback.height,
                        depth_or_array_layers: 1,
                    },
                );
            }
        }
        // Data textures must be filled before any render pass samples them.
        // Queue buffer writes already precede submission on the storage transport.
        let upload =
            if self.renderer.resources().instances.transport() == InstanceTransport::DataTexture {
                let mut encoder = self.renderer.resources().device.create_command_encoder(
                    &wgpu::CommandEncoderDescriptor {
                        label: Some("scene_data_upload"),
                    },
                );
                self.instances.finish(&mut encoder);

                Some(encoder.finish())
            } else {
                self.instances.finish(&mut self.encoder);

                None
            };

        self.renderer.resources().finish_frame_uploads();
        let command_buffer = self.encoder.finish();

        result.map(|()| (upload, command_buffer))
    }

    fn encode_commands(&mut self, timestamps: Option<&wgpu::QuerySet>) -> DrawResult {
        let liquid_slice = self
            .instances
            .write(&self.renderer.resources().liquid.cells)
            .ok_or(DrawError::CapacityPlanningInvariant)?;
        let mut pass = begin_scene_render_pass(
            self.renderer,
            &mut self.encoder,
            "main_pass",
            self.targets.current(),
            wgpu::LoadOp::Clear(self.renderer.target.clear_color()),
            timestamps,
        );

        for command in self.scene.render_commands() {
            match command {
                RenderCommand::Batch(PrimitiveBatch::Paths {
                    range,
                    rasterization_vertex_count,
                    ..
                }) => {
                    if *rasterization_vertex_count == 0 {
                        continue;
                    }
                    let paths = &self.scene.paths[range.clone()];
                    drop(pass);
                    let rasterized = self.renderer.draw_paths_to_intermediate(
                        &mut self.encoder,
                        paths,
                        &mut self.instances,
                    );
                    pass = begin_scene_render_pass(
                        self.renderer,
                        &mut self.encoder,
                        "after_paths",
                        self.targets.current(),
                        wgpu::LoadOp::Load,
                        None,
                    );
                    rasterized?;
                    self.renderer.draw_paths_from_intermediate(
                        paths,
                        &mut self.instances,
                        &mut pass,
                    )?;
                }
                RenderCommand::Batch(PrimitiveBatch::BackdropFilters(range)) => {
                    drop(pass);
                    for filter in &self.scene.backdrop_filters[range.clone()] {
                        let snapshot = self.targets.inherits_backdrop().then(|| {
                            self.renderer.snapshot_backdrop(
                                &mut self.encoder,
                                self.targets.backdrop_layers(),
                            )
                        });
                        let source = snapshot.as_ref().unwrap_or_else(|| self.targets.current());
                        self.renderer.blur_and_composite(
                            &mut self.encoder,
                            source,
                            self.targets.current(),
                            FilterCompositeParameters {
                                bounds: filter.bounds,
                                content_mask: filter.content_mask.bounds,
                                corner_radii: filter.corner_radii,
                                corner_smoothing: filter.corner_smoothing,
                                blur_radius: filter.max_blur_radius(),
                                opacity: filter.opacity,
                                clip: FilterCompositeClip::RoundedBounds,
                            },
                        );
                    }
                    pass = begin_scene_render_pass(
                        self.renderer,
                        &mut self.encoder,
                        "after_backdrop_filter",
                        self.targets.current(),
                        wgpu::LoadOp::Load,
                        None,
                    );
                }
                RenderCommand::Batch(PrimitiveBatch::FilterBoundary(_)) => {
                    unreachable!("filter boundaries must be compiled into render commands")
                }
                RenderCommand::Batch(batch) => encode_inline_batch(
                    self.renderer,
                    self.scene,
                    batch,
                    &mut self.instances,
                    &liquid_slice,
                    &mut pass,
                )?,
                RenderCommand::BeginFilter {
                    boundary_index,
                    target: FilterRenderTarget::Isolated(index),
                } => {
                    drop(pass);
                    let target =
                        self.renderer.resources().filter_group_views[index.as_usize()].clone();
                    self.targets.enter(
                        target,
                        self.scene.filter_boundaries[*boundary_index]
                            .filters
                            .is_empty(),
                    );
                    pass = begin_scene_render_pass(
                        self.renderer,
                        &mut self.encoder,
                        "filter_group",
                        self.targets.current(),
                        wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        None,
                    );
                }
                RenderCommand::EndFilter {
                    boundary_index,
                    target: FilterRenderTarget::Isolated(_),
                    ..
                } => {
                    drop(pass);
                    let (filtered, parent) = self.targets.exit();
                    let boundary = &self.scene.filter_boundaries[*boundary_index];
                    self.renderer.blur_and_composite(
                        &mut self.encoder,
                        &filtered,
                        parent,
                        FilterCompositeParameters {
                            bounds: boundary.bounds,
                            content_mask: boundary.content_mask.bounds,
                            corner_radii: boundary.corner_radii,
                            corner_smoothing: boundary.corner_smoothing,
                            blur_radius: boundary.max_blur_radius(),
                            opacity: boundary.opacity,
                            clip: FilterCompositeClip::ContentShape,
                        },
                    );
                    pass = begin_scene_render_pass(
                        self.renderer,
                        &mut self.encoder,
                        "after_content_filter",
                        self.targets.current(),
                        wgpu::LoadOp::Load,
                        None,
                    );
                }
                RenderCommand::BeginFilter {
                    target: FilterRenderTarget::Inline,
                    ..
                }
                | RenderCommand::EndFilter {
                    target: FilterRenderTarget::Inline,
                    ..
                } => {}
            }
        }
        drop(pass);
        self.targets.assert_balanced();
        Ok(())
    }
}

fn begin_scene_render_pass<'a>(
    renderer: &'a WgpuRenderer,
    encoder: &'a mut wgpu::CommandEncoder,
    label: &'a str,
    target: &'a wgpu::TextureView,
    load: wgpu::LoadOp<wgpu::Color>,
    timestamps: Option<&'a wgpu::QuerySet>,
) -> wgpu::RenderPass<'a> {
    let timestamp_writes = timestamps.map(|queries| wgpu::RenderPassTimestampWrites {
        query_set: queries,
        beginning_of_pass_write_index: Some(0),
        end_of_pass_write_index: Some(1),
    });
    let mut pass =
        begin_color_render_pass_with_timestamps(encoder, label, target, load, timestamp_writes);
    pass.set_bind_group(
        shader_interface::GLOBAL_BIND_GROUP,
        &renderer.resources().globals_bind_group,
        &[],
    );
    pass
}

#[derive(Debug)]
pub(super) enum DrawError {
    CapacityPlanningInvariant,
    ExternalSurface,
    MissingIntermediateTarget,
}

pub(super) type DrawResult = Result<(), DrawError>;

fn encode_inline_batch(
    renderer: &WgpuRenderer,
    scene: &Scene,
    batch: &PrimitiveBatch,
    instances: &mut InstanceUpload,
    liquid_slice: &InstanceSlice<LiquidCell>,
    pass: &mut wgpu::RenderPass<'_>,
) -> DrawResult {
    match batch {
        PrimitiveBatch::Quads { range, smoothed } => {
            renderer.draw_quads(&scene.quads[range.clone()], *smoothed, instances, pass)
        }

        PrimitiveBatch::Liquid { range, smoothed } => {
            let resources = renderer.resources();

            for draw in &scene.liquid_draws[range.clone()] {
                if resources.liquid.falls_back(draw.record_index) {
                    renderer.draw_quads(&[draw.quad], *smoothed, instances, pass)?;

                    continue;
                }

                let pipeline = match draw.phase {
                    gpui::LiquidPhase::Fill => &resources.pipelines.liquid_fill,
                    gpui::LiquidPhase::Border => &resources.pipelines.liquid_border,
                };

                let source_slice = instances
                    .liquid_source_slice(liquid_slice, draw.record_index)
                    .ok_or(DrawError::CapacityPlanningInvariant)?;
                pass.set_pipeline(pipeline);
                source_slice.set_data_bind_group(pass, resources.instances.bind_group());
                pass.draw(0..pipeline.fixed_vertex_count(), source_slice.range());
            }

            Ok(())
        }

        PrimitiveBatch::Shadows { range, smoothed } => {
            renderer.draw_shadows(&scene.shadows[range.clone()], *smoothed, instances, pass)
        }
        PrimitiveBatch::Underlines(range) => {
            renderer.draw_underlines(&scene.underlines[range.clone()], instances, pass)
        }
        PrimitiveBatch::MonochromeSprites { texture_id, range } => renderer
            .draw_monochrome_sprites(
                &scene.monochrome_sprites[range.clone()],
                *texture_id,
                instances,
                pass,
            ),
        PrimitiveBatch::SubpixelSprites { texture_id, range } => renderer.draw_subpixel_sprites(
            &scene.subpixel_sprites[range.clone()],
            *texture_id,
            instances,
            pass,
        ),
        PrimitiveBatch::PolychromeSprites {
            texture_id,
            range,
            smoothed,
        } => renderer.draw_polychrome_sprites(
            &scene.polychrome_sprites[range.clone()],
            *texture_id,
            *smoothed,
            instances,
            pass,
        ),
        PrimitiveBatch::Surfaces(range) => {
            renderer.draw_surfaces(&scene.surfaces[range.clone()], pass)
        }
        PrimitiveBatch::Paths { .. }
        | PrimitiveBatch::BackdropFilters(_)
        | PrimitiveBatch::FilterBoundary(_) => {
            unreachable!("pass-interrupting batches are handled by FrameEncoder")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{FrameRequirements, InstanceTransport};
    use gpui::{
        Bounds, ContentMask, LiquidPhase, PrimitiveBatch, Quad, RenderCommand, ScaledLiquidBox,
        ScaledPixels, Scene, point, size, white,
    };
    use std::mem::size_of;

    #[test]
    fn liquid_phases_reserve_fallback_storage_and_preserve_phase_order() {
        let bounds = Bounds::new(
            point(ScaledPixels(30.0), ScaledPixels(30.0)),
            size(ScaledPixels(40.0), ScaledPixels(40.0)),
        );
        let quad = Quad {
            bounds,
            content_mask: ContentMask {
                bounds: bounds.dilate(ScaledPixels(30.0)),
                ..Default::default()
            },
            background: white().into(),
            ..Default::default()
        };
        let mut scene = Scene::default();
        scene.insert_primitive(quad);

        for corner_smoothing in [0.0, 0.6] {
            let member = scene
                .register_liquid_box(ScaledLiquidBox {
                    quad: Quad {
                        corner_smoothing,
                        ..quad
                    },
                    group: None,
                    blend_radius: ScaledPixels(8.0),
                    reach: ScaledPixels(24.0),
                })
                .unwrap();
            scene.paint_liquid_phase(member, LiquidPhase::Fill);
            scene.paint_liquid_phase(member, LiquidPhase::Border);
        }

        scene.finish();
        assert_eq!(scene.quads.len(), 1);
        assert_eq!(scene.render_commands().len(), 3);

        assert_eq!(
            scene.render_commands()[1],
            RenderCommand::Batch(PrimitiveBatch::Liquid {
                range: 0..2,
                smoothed: false,
            })
        );

        assert_eq!(
            scene.render_commands()[2],
            RenderCommand::Batch(PrimitiveBatch::Liquid {
                range: 2..4,
                smoothed: true,
            })
        );

        assert_eq!(
            scene
                .liquid_draws
                .iter()
                .map(|draw| (draw.record_index, draw.phase))
                .collect::<Vec<_>>(),
            [
                (0, LiquidPhase::Fill),
                (0, LiquidPhase::Border),
                (1, LiquidPhase::Fill),
                (1, LiquidPhase::Border),
            ],
        );

        for transport in [
            InstanceTransport::StorageBuffer,
            InstanceTransport::DataTexture,
        ] {
            let requirements = FrameRequirements::for_scene(&scene, transport);
            let quad_bytes = size_of::<Quad>() as u64;
            let expected_bytes = match transport {
                InstanceTransport::StorageBuffer => 5 * quad_bytes,
                InstanceTransport::DataTexture => 4 * quad_bytes.next_multiple_of(16) + quad_bytes,
            };
            assert_eq!(requirements.storage_bytes, expected_bytes);
            assert_eq!(requirements.instance_batches, 5);
            assert_eq!(requirements.planned.instance_batch_count, 5);
            assert_eq!(requirements.planned.liquid_record_count, 2);
            assert_eq!(requirements.planned.liquid_component_count, 1);
            assert_eq!(requirements.planned.liquid_draw_count, 4);
        }

        scene.clear();
        scene.insert_primitive(quad);
        scene.finish();
        let requirements = FrameRequirements::for_scene(&scene, InstanceTransport::StorageBuffer);
        assert_eq!(requirements.storage_bytes, size_of::<Quad>() as u64);
        assert_eq!(requirements.instance_batches, 1);
        assert_eq!(requirements.planned.liquid_draw_count, 0);
    }
}
