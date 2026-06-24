use anyhow::Result;
use gpui::{
    Bounds, DevicePixels, Font, FontId, FontMetrics, FontRun, GlyphId, Hsla, LineLayout, Pixels,
    PlatformTextSystem, RenderGlyphParams, Size, TextRenderingMode,
};
use parking_lot::RwLock;
use parley::{FontContext, LayoutContext, StyleProperty};
use std::borrow::Cow;

pub struct ParleyTextSystem {
    state: RwLock<ParleyTextSystemState>,
}

struct ParleyTextSystemState {
    fallback_family: String,
    use_system_fonts: bool,
    font_context: FontContext,
    layout_context: LayoutContext,
    noop: gpui::NoopTextSystem,
}

impl ParleyTextSystem {
    pub fn new(system_font_fallback: &str) -> Self {
        Self::with_system_fonts(system_font_fallback, true)
    }

    pub fn new_without_system_fonts(system_font_fallback: &str) -> Self {
        Self::with_system_fonts(system_font_fallback, false)
    }

    fn with_system_fonts(system_font_fallback: &str, use_system_fonts: bool) -> Self {
        Self {
            state: RwLock::new(ParleyTextSystemState {
                fallback_family: system_font_fallback.to_string(),
                use_system_fonts,
                font_context: FontContext::new(),
                layout_context: LayoutContext::new(),
                noop: gpui::NoopTextSystem::new(),
            }),
        }
    }
}

impl PlatformTextSystem for ParleyTextSystem {
    fn add_fonts(&self, fonts: Vec<Cow<'static, [u8]>>) -> Result<()> {
        self.state.read().noop.add_fonts(fonts)
    }

    fn all_font_names(&self) -> Vec<String> {
        let state = self.state.read();
        if state.use_system_fonts {
            vec![state.fallback_family.clone()]
        } else {
            Vec::new()
        }
    }

    fn font_id(&self, descriptor: &Font) -> Result<FontId> {
        self.state.read().noop.font_id(descriptor)
    }

    fn font_metrics(&self, font_id: FontId) -> FontMetrics {
        self.state.read().noop.font_metrics(font_id)
    }

    fn typographic_bounds(&self, font_id: FontId, glyph_id: GlyphId) -> Result<Bounds<f32>> {
        self.state.read().noop.typographic_bounds(font_id, glyph_id)
    }

    fn advance(&self, font_id: FontId, glyph_id: GlyphId) -> Result<Size<f32>> {
        self.state.read().noop.advance(font_id, glyph_id)
    }

    fn glyph_for_char(&self, font_id: FontId, ch: char) -> Option<GlyphId> {
        self.state.read().noop.glyph_for_char(font_id, ch)
    }

    fn glyph_raster_bounds(&self, params: &RenderGlyphParams) -> Result<Bounds<DevicePixels>> {
        self.state.read().noop.glyph_raster_bounds(params)
    }

    fn rasterize_glyph(
        &self,
        params: &RenderGlyphParams,
        raster_bounds: Bounds<DevicePixels>,
    ) -> Result<(Size<DevicePixels>, Vec<u8>)> {
        self.state
            .read()
            .noop
            .rasterize_glyph(params, raster_bounds)
    }

    fn layout_line(&self, text: &str, font_size: Pixels, runs: &[FontRun]) -> LineLayout {
        let mut state = self.state.write();
        let ParleyTextSystemState {
            font_context,
            layout_context,
            noop,
            ..
        } = &mut *state;
        let mut builder = layout_context.ranged_builder(font_context, text, 1.0, true);
        builder.push_default(StyleProperty::FontSize(f32::from(font_size)));
        let _layout = builder.build(text);

        noop.layout_line(text, font_size, runs)
    }

    fn recommended_rendering_mode(&self, font_id: FontId, font_size: Pixels) -> TextRenderingMode {
        self.state
            .read()
            .noop
            .recommended_rendering_mode(font_id, font_size)
    }

    fn glyph_dilation_for_color(&self, color: Hsla) -> u8 {
        self.state.read().noop.glyph_dilation_for_color(color)
    }
}
