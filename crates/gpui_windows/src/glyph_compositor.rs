use std::{
    slice,
    thread::{self, ThreadId},
};

use anyhow::{Context, Result, ensure};
use gpui::{Bounds, DevicePixels, Size};
use gpui_render::shaders::emoji_rasterization::GlyphLayerTextureParams;
use parking_lot::Mutex;
use wgsl_rs::std::{vec2i, vec3f, vec4f};
use windows::Win32::Graphics::{
    Direct3D::D3D_PRIMITIVE_TOPOLOGY_TRIANGLESTRIP, Direct3D11::*, Dxgi::Common::*,
};

use crate::{directx_devices::DirectXDevices, directx_renderer::shader_resources::ShaderModule};

const MAX_TEXTURE_DIMENSION: i32 = 16384;
// HLSL aligns the trailing float3 to a new register; the shared Rust struct is 64 bytes.
const PARAMS_BUFFER_SIZE: u32 = 80;

pub(crate) struct ColorGlyphLayer {
    /// Layer placement relative to the complete glyph bitmap's top-left corner.
    pub bounds: Bounds<DevicePixels>,
    pub color: [f32; 4],
    pub coverage: Vec<u8>,
}

pub(crate) struct GlyphCompositor {
    owner_thread: ThreadId,
    state: Mutex<Option<CompositorState>>,
}

struct CompositorState {
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    sampler: ID3D11SamplerState,
    blend: ID3D11BlendState,
    rasterizer: ID3D11RasterizerState,
    vertex: ID3D11VertexShader,
    fragment: ID3D11PixelShader,
    params: ID3D11Buffer,
    target: Option<GlyphTarget>,
}

struct GlyphTarget {
    size: Size<DevicePixels>,
    texture: ID3D11Texture2D,
    view: ID3D11RenderTargetView,
    staging: ID3D11Texture2D,
}

struct RasterizerRestore<'a> {
    context: &'a ID3D11DeviceContext,
    previous: Option<ID3D11RasterizerState>,
}

impl Drop for RasterizerRestore<'_> {
    fn drop(&mut self) {
        unsafe { self.context.RSSetState(self.previous.as_ref()) };
    }
}

impl GlyphCompositor {
    pub(crate) fn new(devices: &DirectXDevices) -> Result<Self> {
        Ok(Self {
            owner_thread: thread::current().id(),
            state: Mutex::new(Some(CompositorState::new(devices)?)),
        })
    }

    pub(crate) fn reset(&self, devices: &DirectXDevices) -> Result<()> {
        ensure!(
            thread::current().id() == self.owner_thread,
            "glyph compositor recovery must run on its owner thread"
        );
        let mut state = self.state.lock();
        state.take();
        *state = Some(CompositorState::new(devices)?);

        Ok(())
    }

    pub(crate) fn composite(
        &self,
        layers: &[ColorGlyphLayer],
        size: Size<DevicePixels>,
        gamma_ratios: [f32; 4],
        grayscale_enhanced_contrast: f32,
    ) -> Result<Option<Vec<u8>>> {
        // The renderer and atlas use this immediate context without taking our mutex.
        // Only their UI thread may submit work through it.
        if thread::current().id() != self.owner_thread {
            return Ok(None);
        }

        let mut state = self.state.lock();
        let Some(state) = state.as_mut() else {
            return Ok(None);
        };
        validate_layers(layers, size)?;
        ensure!(
            gamma_ratios.iter().all(|ratio| ratio.is_finite())
                && grayscale_enhanced_contrast.is_finite()
                && grayscale_enhanced_contrast >= 0.0,
            "invalid glyph contrast or gamma settings"
        );

        state
            .composite(layers, size, gamma_ratios, grayscale_enhanced_contrast)
            .map(Some)
    }
}

impl CompositorState {
    fn new(devices: &DirectXDevices) -> Result<Self> {
        let device = &devices.device;
        let bytecode = ShaderModule::EmojiRasterization.bytecode()?;
        let mut vertex = None;
        let mut fragment = None;
        unsafe {
            device.CreateVertexShader(bytecode.vertex, None, Some(&mut vertex))?;
            device.CreatePixelShader(bytecode.fragment, None, Some(&mut fragment))?;
        }

        let mut sampler = None;
        let sampler_desc = D3D11_SAMPLER_DESC {
            Filter: D3D11_FILTER_MIN_MAG_MIP_POINT,
            AddressU: D3D11_TEXTURE_ADDRESS_BORDER,
            AddressV: D3D11_TEXTURE_ADDRESS_BORDER,
            AddressW: D3D11_TEXTURE_ADDRESS_BORDER,
            ComparisonFunc: D3D11_COMPARISON_ALWAYS,
            MaxAnisotropy: 1,
            ..Default::default()
        };
        unsafe { device.CreateSamplerState(&sampler_desc, Some(&mut sampler)) }?;

        let mut blend = None;
        let mut blend_desc = D3D11_BLEND_DESC::default();
        blend_desc.RenderTarget[0] = D3D11_RENDER_TARGET_BLEND_DESC {
            BlendEnable: true.into(),
            SrcBlend: D3D11_BLEND_ONE,
            DestBlend: D3D11_BLEND_INV_SRC_ALPHA,
            BlendOp: D3D11_BLEND_OP_ADD,
            SrcBlendAlpha: D3D11_BLEND_ONE,
            DestBlendAlpha: D3D11_BLEND_INV_SRC_ALPHA,
            BlendOpAlpha: D3D11_BLEND_OP_ADD,
            RenderTargetWriteMask: D3D11_COLOR_WRITE_ENABLE_ALL.0 as u8,
        };
        unsafe { device.CreateBlendState(&blend_desc, Some(&mut blend)) }?;

        let mut rasterizer = None;
        let rasterizer_desc = D3D11_RASTERIZER_DESC {
            FillMode: D3D11_FILL_SOLID,
            CullMode: D3D11_CULL_NONE,
            DepthClipEnable: true.into(),
            ..Default::default()
        };
        unsafe { device.CreateRasterizerState(&rasterizer_desc, Some(&mut rasterizer)) }?;

        let mut params = None;
        let params_desc = D3D11_BUFFER_DESC {
            ByteWidth: PARAMS_BUFFER_SIZE,
            Usage: D3D11_USAGE_DYNAMIC,
            BindFlags: D3D11_BIND_CONSTANT_BUFFER.0 as u32,
            CPUAccessFlags: D3D11_CPU_ACCESS_WRITE.0 as u32,
            ..Default::default()
        };
        unsafe { device.CreateBuffer(&params_desc, None, Some(&mut params)) }?;

        Ok(Self {
            device: device.clone(),
            context: devices.device_context.clone(),
            sampler: sampler.context("missing glyph sampler")?,
            blend: blend.context("missing glyph blend state")?,
            rasterizer: rasterizer.context("missing glyph rasterizer state")?,
            vertex: vertex.context("missing glyph vertex shader")?,
            fragment: fragment.context("missing glyph fragment shader")?,
            params: params.context("missing glyph constants buffer")?,
            target: None,
        })
    }

    fn composite(
        &mut self,
        layers: &[ColorGlyphLayer],
        size: Size<DevicePixels>,
        gamma_ratios: [f32; 4],
        grayscale_enhanced_contrast: f32,
    ) -> Result<Vec<u8>> {
        if self
            .target
            .as_ref()
            .is_none_or(|target| target.size != size)
        {
            self.target = Some(GlyphTarget::new(&self.device, size)?);
        }

        let target = self.target.as_ref().unwrap();
        let params_buffer = [Some(self.params.clone())];
        // Scene drawing retains its rasterizer state between frames, including path MSAA.
        let _restore_rasterizer = RasterizerRestore {
            context: &self.context,
            previous: unsafe { self.context.RSGetState() }.ok(),
        };
        unsafe {
            self.context.RSSetState(&self.rasterizer);
            self.context
                .IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLESTRIP);
            self.context.VSSetShader(&self.vertex, None);
            self.context.PSSetShader(&self.fragment, None);
            self.context.VSSetConstantBuffers(0, Some(&params_buffer));
            self.context.PSSetConstantBuffers(0, Some(&params_buffer));
            self.context
                .OMSetRenderTargets(Some(&[Some(target.view.clone())]), None);
            self.context.ClearRenderTargetView(&target.view, &[0.0; 4]);
            self.context
                .PSSetSamplers(2, Some(&[Some(self.sampler.clone())]));
            self.context.OMSetBlendState(&self.blend, None, u32::MAX);
        }

        for layer in layers {
            let layer_view = upload_layer(&self.device, layer)?;
            let params = GlyphLayerTextureParams {
                bounds_origin: vec2i(layer.bounds.origin.x.0, layer.bounds.origin.y.0),
                bounds_size: vec2i(layer.bounds.size.width.0, layer.bounds.size.height.0),
                run_color: vec4f(
                    layer.color[0],
                    layer.color[1],
                    layer.color[2],
                    layer.color[3],
                ),
                gamma_ratios: vec4f(
                    gamma_ratios[0],
                    gamma_ratios[1],
                    gamma_ratios[2],
                    gamma_ratios[3],
                ),
                grayscale_enhanced_contrast,
                padding: vec3f(0.0, 0.0, 0.0),
            };

            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            unsafe {
                self.context.Map(
                    &self.params,
                    0,
                    D3D11_MAP_WRITE_DISCARD,
                    0,
                    Some(&mut mapped),
                )?;
                std::ptr::write_bytes(mapped.pData.cast::<u8>(), 0, PARAMS_BUFFER_SIZE as usize);
                std::ptr::copy_nonoverlapping(&params, mapped.pData.cast(), 1);
                self.context.Unmap(&self.params, 0);
            }

            let viewport = D3D11_VIEWPORT {
                TopLeftX: layer.bounds.origin.x.0 as f32,
                TopLeftY: layer.bounds.origin.y.0 as f32,
                Width: layer.bounds.size.width.0 as f32,
                Height: layer.bounds.size.height.0 as f32,
                MinDepth: 0.0,
                MaxDepth: 1.0,
            };
            unsafe {
                self.context
                    .PSSetShaderResources(1, Some(&[Some(layer_view)]));
                self.context
                    .RSSetViewports(Some(slice::from_ref(&viewport)));
                self.context.Draw(4, 0);
            }
        }

        unsafe {
            self.context.PSSetShaderResources(1, Some(&[None]));
            self.context.OMSetRenderTargets(None, None);
            self.context.CopyResource(&target.staging, &target.texture);
        }

        target.read_pixels(&self.context)
    }
}

impl GlyphTarget {
    fn new(device: &ID3D11Device, size: Size<DevicePixels>) -> Result<Self> {
        let mut desc = D3D11_TEXTURE2D_DESC {
            Width: size.width.0 as u32,
            Height: size.height.0 as u32,
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: D3D11_BIND_RENDER_TARGET.0 as u32,
            ..Default::default()
        };

        let mut texture = None;
        unsafe { device.CreateTexture2D(&desc, None, Some(&mut texture)) }?;
        let texture = texture.context("missing glyph render target")?;
        let mut view = None;
        unsafe { device.CreateRenderTargetView(&texture, None, Some(&mut view)) }?;

        desc.Usage = D3D11_USAGE_STAGING;
        desc.BindFlags = 0;
        desc.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
        let mut staging = None;
        unsafe { device.CreateTexture2D(&desc, None, Some(&mut staging)) }?;

        Ok(Self {
            size,
            texture,
            view: view.context("missing glyph render target view")?,
            staging: staging.context("missing glyph staging texture")?,
        })
    }

    fn read_pixels(&self, context: &ID3D11DeviceContext) -> Result<Vec<u8>> {
        let row_bytes = self.size.width.0 as usize * 4;
        let mut pixels = vec![0; row_bytes * self.size.height.0 as usize];
        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        unsafe { context.Map(&self.staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped)) }?;

        for (row_idx, row) in pixels.chunks_exact_mut(row_bytes).enumerate() {
            unsafe {
                std::ptr::copy_nonoverlapping(
                    mapped
                        .pData
                        .cast::<u8>()
                        .add(row_idx * mapped.RowPitch as usize),
                    row.as_mut_ptr(),
                    row_bytes,
                );
            }
        }

        unsafe { context.Unmap(&self.staging, 0) };

        // GPU blending produces premultiplied pixels; the atlas accepts straight BGRA.
        for pixel in pixels.chunks_exact_mut(4) {
            if pixel[3] == 0 {
                pixel.fill(0);

                continue;
            }

            let inverse_alpha = 255.0 / f32::from(pixel[3]);
            for channel in &mut pixel[..3] {
                *channel = (f32::from(*channel) * inverse_alpha).clamp(0.0, 255.0) as u8;
            }
        }

        Ok(pixels)
    }
}

fn upload_layer(
    device: &ID3D11Device,
    layer: &ColorGlyphLayer,
) -> Result<ID3D11ShaderResourceView> {
    let desc = D3D11_TEXTURE2D_DESC {
        Width: layer.bounds.size.width.0 as u32,
        Height: layer.bounds.size.height.0 as u32,
        MipLevels: 1,
        ArraySize: 1,
        Format: DXGI_FORMAT_R8_UNORM,
        SampleDesc: DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        Usage: D3D11_USAGE_IMMUTABLE,
        BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32,
        ..Default::default()
    };
    let initial_data = D3D11_SUBRESOURCE_DATA {
        pSysMem: layer.coverage.as_ptr().cast(),
        SysMemPitch: layer.bounds.size.width.0 as u32,
        SysMemSlicePitch: 0,
    };
    let mut texture = None;
    unsafe { device.CreateTexture2D(&desc, Some(&initial_data), Some(&mut texture)) }?;
    let texture = texture.context("missing glyph coverage texture")?;
    let mut view = None;
    unsafe { device.CreateShaderResourceView(&texture, None, Some(&mut view)) }?;

    // The view retains its texture after this local COM reference is dropped.
    view.context("missing glyph coverage texture view")
}

fn validate_size(size: Size<DevicePixels>) -> Result<usize> {
    ensure!(
        (1..=MAX_TEXTURE_DIMENSION).contains(&size.width.0)
            && (1..=MAX_TEXTURE_DIMENSION).contains(&size.height.0),
        "glyph bitmap dimensions exceed the Direct3D 11 texture limits: {size:?}"
    );

    Ok(size.width.0 as usize * size.height.0 as usize)
}

fn validate_layers(layers: &[ColorGlyphLayer], size: Size<DevicePixels>) -> Result<()> {
    validate_size(size)?;

    for layer in layers {
        let pixel_count = validate_size(layer.bounds.size)?;
        ensure!(
            layer.coverage.len() == pixel_count,
            "glyph layer has {} coverage bytes, expected {pixel_count}",
            layer.coverage.len()
        );
        ensure!(
            layer
                .color
                .iter()
                .all(|channel| channel.is_finite() && (0.0..=1.0).contains(channel)),
            "invalid glyph layer color"
        );
        ensure!(
            (-MAX_TEXTURE_DIMENSION..=MAX_TEXTURE_DIMENSION).contains(&layer.bounds.origin.x.0)
                && (-MAX_TEXTURE_DIMENSION..=MAX_TEXTURE_DIMENSION)
                    .contains(&layer.bounds.origin.y.0),
            "glyph layer origin exceeds the Direct3D 11 viewport limits"
        );
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{bounds, point, size};

    fn layer(x: i32, y: i32, width: i32, height: i32, color: [f32; 4]) -> ColorGlyphLayer {
        ColorGlyphLayer {
            bounds: bounds(
                point(DevicePixels(x), DevicePixels(y)),
                size(DevicePixels(width), DevicePixels(height)),
            ),
            color,
            coverage: vec![255; width as usize * height as usize],
        }
    }

    #[test]
    fn reused_targets_clear_uncovered_pixels_and_preserve_layer_composition() -> Result<()> {
        let devices = DirectXDevices::new()?;
        let compositor = GlyphCompositor::new(&devices)?;
        let mut scene_rasterizer = None;
        let scene_desc = D3D11_RASTERIZER_DESC {
            FillMode: D3D11_FILL_SOLID,
            CullMode: D3D11_CULL_FRONT,
            DepthClipEnable: true.into(),
            ScissorEnable: true.into(),
            MultisampleEnable: true.into(),
            ..Default::default()
        };
        unsafe {
            devices
                .device
                .CreateRasterizerState(&scene_desc, Some(&mut scene_rasterizer))?;
            devices.device_context.RSSetState(scene_rasterizer.as_ref());
        }

        let bitmap_size = size(DevicePixels(8), DevicePixels(8));
        let poison = layer(0, 0, 8, 8, [1.0, 0.0, 0.0, 1.0]);
        compositor
            .composite(&[poison], bitmap_size, [0.0; 4], 0.0)?
            .unwrap();

        let layers = [
            layer(1, 1, 4, 4, [1.0, 0.0, 0.0, 0.5]),
            layer(3, 3, 4, 4, [0.0, 0.0, 1.0, 0.5]),
        ];
        let first = compositor
            .composite(&layers, bitmap_size, [0.0; 4], 0.0)?
            .unwrap();
        let pixel = |x: usize, y: usize| &first[(y * 8 + x) * 4..(y * 8 + x + 1) * 4];
        assert_eq!(pixel(0, 0), &[0, 0, 0, 0]);
        assert_eq!(pixel(7, 7), &[0, 0, 0, 0]);
        assert_eq!(pixel(1, 1), &[0, 0, 255, 128]);
        assert_eq!(pixel(6, 6), &[255, 0, 0, 128]);

        let mut restored = D3D11_RASTERIZER_DESC::default();
        unsafe { devices.device_context.RSGetState()?.GetDesc(&mut restored) };
        assert_eq!(restored.CullMode, D3D11_CULL_FRONT);
        assert!(restored.ScissorEnable.as_bool());
        assert!(restored.MultisampleEnable.as_bool());

        let overlap = pixel(3, 3);
        for (actual, expected) in overlap.iter().zip([170u8, 0, 85, 192]) {
            assert!(
                actual.abs_diff(expected) <= 2,
                "unexpected source-over pixel: {overlap:?}"
            );
        }

        for _iteration in 0..3 {
            let poison = layer(0, 0, 8, 8, [0.0, 1.0, 0.0, 1.0]);
            compositor.composite(&[poison], bitmap_size, [0.0; 4], 0.0)?;
            let repeated = compositor
                .composite(&layers, bitmap_size, [0.0; 4], 0.0)?
                .unwrap();
            assert_eq!(first, repeated);
        }

        let replacement = DirectXDevices::new()?;
        compositor.reset(&replacement)?;

        {
            let state = compositor.state.lock();
            let state = state.as_ref().unwrap();
            assert_eq!(state.device, replacement.device);
            assert_eq!(state.context, replacement.device_context);
            assert!(state.target.is_none());
        }

        assert_eq!(
            first,
            compositor
                .composite(&layers, bitmap_size, [0.0; 4], 0.0)?
                .unwrap()
        );

        Ok(())
    }

    #[test]
    fn malformed_layers_are_rejected_before_gpu_submission() {
        let bitmap_size = size(DevicePixels(8), DevicePixels(8));
        let mut invalid = layer(0, 0, 4, 4, [1.0; 4]);
        invalid.coverage.pop();
        assert!(validate_layers(&[invalid], bitmap_size).is_err());
        assert!(validate_layers(&[], size(DevicePixels(0), DevicePixels(8))).is_err());
        assert!(validate_layers(&[], size(DevicePixels(16385), DevicePixels(8))).is_err());

        let mut invalid = layer(0, 0, 4, 4, [1.0; 4]);
        invalid.color[0] = f32::NAN;
        assert!(validate_layers(&[invalid], bitmap_size).is_err());
    }
}
