use anyhow::{Context as _, Result};
use collections::HashMap;
use gpui::{
    Bounds, DevicePixels, Font, FontFallbacks, FontFeatures, FontId, FontMetrics, FontRun, GlyphId,
    Hsla, LineLayout, Pixels, PlatformTextSystem, RenderGlyphParams, SharedString, Size,
    TextRenderingMode,
};
use parking_lot::RwLock;
use parley::fontique::{self, Collection, CollectionOptions, FamilyInfo, FontInfo, SourceCache};
use parley::{FontContext, LayoutContext, StyleProperty};
use skrifa::{
    FontRef as SkrifaFontRef, GlyphId as SkrifaGlyphId, MetadataProvider,
    instance::{LocationRef, Size as SkrifaSize},
};
use smallvec::SmallVec;
use std::{borrow::Cow, sync::Arc};
use swash::{FontRef as SwashFontRef, StringId};

pub struct ParleyTextSystem {
    state: RwLock<ParleyTextSystemState>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct FontKey {
    family: SharedString,
    features: FontFeatures,
    fallbacks: Option<FontFallbacks>,
}

impl FontKey {
    fn new(family: SharedString, features: FontFeatures, fallbacks: Option<FontFallbacks>) -> Self {
        Self {
            family,
            features,
            fallbacks,
        }
    }
}

struct ParleyTextSystemState {
    fallback_family: String,
    font_context: FontContext,
    layout_context: LayoutContext,
    noop: gpui::NoopTextSystem,
    loaded_fonts: Vec<LoadedFont>,
    font_ids_by_font: HashMap<Font, Option<FontId>>,
    font_ids_by_family_key: HashMap<FontKey, SmallVec<[FontId; 4]>>,
    font_ids_by_loaded_font_key: HashMap<LoadedFontKey, FontId>,
    font_id_by_fontique_handle: HashMap<FontiqueHandleKey, FontId>,
}

#[allow(dead_code)]
struct LoadedFont {
    id: FontId,
    family_name: SharedString,
    postscript_name: Option<String>,
    style: fontique::FontStyle,
    weight: fontique::FontWeight,
    stretch: fontique::FontWidth,
    font_data: parley::FontData,
    features: FontFeatures,
    is_known_emoji_font: bool,
    user_fallback_chain: Arc<[(FontId, SharedString)]>,
    fontique_handle: FontiqueHandleKey,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct LoadedFontKey {
    handle: FontiqueHandleKey,
    features: FontFeatures,
    fallbacks: Option<FontFallbacks>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct FontiqueHandleKey {
    family_id: u64,
    family_index: usize,
}

impl ParleyTextSystem {
    pub fn new(system_font_fallback: &str) -> Self {
        Self::with_system_fonts(system_font_fallback, true)
    }

    pub fn new_without_system_fonts(system_font_fallback: &str) -> Self {
        Self::with_system_fonts(system_font_fallback, false)
    }

    fn with_system_fonts(system_font_fallback: &str, use_system_fonts: bool) -> Self {
        let font_context = FontContext {
            collection: Collection::new(CollectionOptions {
                shared: false,
                system_fonts: use_system_fonts,
            }),
            source_cache: SourceCache::default(),
        };

        Self {
            state: RwLock::new(ParleyTextSystemState {
                fallback_family: system_font_fallback.to_string(),
                font_context,
                layout_context: LayoutContext::new(),
                noop: gpui::NoopTextSystem::new(),
                loaded_fonts: Vec::new(),
                font_ids_by_font: HashMap::default(),
                font_ids_by_family_key: HashMap::default(),
                font_ids_by_loaded_font_key: HashMap::default(),
                font_id_by_fontique_handle: HashMap::default(),
            }),
        }
    }
}

impl PlatformTextSystem for ParleyTextSystem {
    fn add_fonts(&self, fonts: Vec<Cow<'static, [u8]>>) -> Result<()> {
        self.state.write().add_fonts(fonts)
    }

    fn all_font_names(&self) -> Vec<String> {
        let mut state = self.state.write();
        let mut result = state
            .font_context
            .collection
            .family_names()
            .map(str::to_string)
            .collect::<Vec<_>>();

        result.sort();
        result.dedup();

        result
    }

    fn font_id(&self, descriptor: &Font) -> Result<FontId> {
        self.state.write().font_id(descriptor)
    }

    fn font_metrics(&self, font_id: FontId) -> FontMetrics {
        let state = self.state.read();
        state
            .font_metrics(font_id)
            .unwrap_or_else(|| state.noop.font_metrics(font_id))
    }

    fn typographic_bounds(&self, font_id: FontId, glyph_id: GlyphId) -> Result<Bounds<f32>> {
        let state = self.state.read();
        if state.loaded_font(font_id).is_some() {
            state.typographic_bounds(font_id, glyph_id)
        } else {
            state.noop.typographic_bounds(font_id, glyph_id)
        }
    }

    fn advance(&self, font_id: FontId, glyph_id: GlyphId) -> Result<Size<f32>> {
        let state = self.state.read();
        if state.loaded_font(font_id).is_some() {
            state.advance(font_id, glyph_id)
        } else {
            state.noop.advance(font_id, glyph_id)
        }
    }

    fn glyph_for_char(&self, font_id: FontId, ch: char) -> Option<GlyphId> {
        let state = self.state.read();
        if state.loaded_font(font_id).is_some() {
            state.glyph_for_char(font_id, ch)
        } else {
            state.noop.glyph_for_char(font_id, ch)
        }
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

impl ParleyTextSystemState {
    #[profiling::function]
    fn add_fonts(&mut self, fonts: Vec<Cow<'static, [u8]>>) -> Result<()> {
        for bytes in fonts {
            self.font_context
                .collection
                .register_fonts(fontique::Blob::new(Arc::new(bytes)), None);
        }

        self.font_ids_by_font.clear();
        self.font_ids_by_family_key.clear();
        Ok(())
    }

    #[profiling::function]
    fn font_id(&mut self, font: &Font) -> Result<FontId> {
        if let Some(cached) = self.font_ids_by_font.get(font) {
            if let Some(font_id) = cached {
                return Ok(*font_id);
            }
            anyhow::bail!(
                "requested font family '{}' contains no font matching the descriptor",
                font.family
            );
        }

        match self.resolve_font_id(font) {
            Ok(font_id) => {
                self.font_ids_by_font.insert(font.clone(), Some(font_id));
                Ok(font_id)
            }
            Err(error) => {
                self.font_ids_by_font.insert(font.clone(), None);
                Err(error)
            }
        }
    }

    fn resolve_font_id(&mut self, font: &Font) -> Result<FontId> {
        let key = FontKey::new(
            font.family.clone(),
            font.features.clone(),
            font.fallbacks.clone(),
        );
        let candidates = if let Some(font_ids) = self.font_ids_by_family_key.get(&key) {
            font_ids.clone()
        } else {
            let font_ids =
                self.load_family(&font.family, &font.features, font.fallbacks.as_ref())?;
            self.font_ids_by_family_key
                .insert(key.clone(), font_ids.clone());
            font_ids
        };

        let ix = find_best_match(font, &candidates, self)?;
        Ok(candidates[ix])
    }

    #[profiling::function]
    fn load_family(
        &mut self,
        name: &str,
        features: &FontFeatures,
        fallbacks: Option<&FontFallbacks>,
    ) -> Result<SmallVec<[FontId; 4]>> {
        let user_fallback_chain = self.resolve_user_fallback_chain(features, fallbacks)?;
        let resolved_name = gpui::font_name_with_fallbacks(name, &self.fallback_family);
        let Some(family) = self.font_context.collection.family_by_name(resolved_name) else {
            return Ok(SmallVec::new());
        };

        let mut loaded_font_ids = SmallVec::new();
        for (family_index, font_info) in family.fonts().iter().enumerate() {
            if let Some(font_id) = self.load_font(
                &family,
                family_index,
                font_info,
                features,
                fallbacks.cloned(),
                Arc::clone(&user_fallback_chain),
            )? {
                loaded_font_ids.push(font_id);
            }
        }

        Ok(loaded_font_ids)
    }

    fn resolve_user_fallback_chain(
        &mut self,
        features: &FontFeatures,
        fallbacks: Option<&FontFallbacks>,
    ) -> Result<Arc<[(FontId, SharedString)]>> {
        let Some(fallbacks) = fallbacks else {
            return Ok(Arc::from(Vec::new()));
        };
        if fallbacks.fallback_list().is_empty() {
            return Ok(Arc::from(Vec::new()));
        }

        let mut chain = Vec::new();
        for fallback_name in fallbacks.fallback_list() {
            let fb_key = FontKey::new(
                SharedString::from(fallback_name.clone()),
                features.clone(),
                None,
            );
            let fb_ids = if let Some(cached) = self.font_ids_by_family_key.get(&fb_key) {
                cached.clone()
            } else {
                let loaded = self.load_family(fallback_name, features, None)?;
                self.font_ids_by_family_key
                    .insert(fb_key.clone(), loaded.clone());
                loaded
            };
            let Some(&fb_id) = fb_ids.first() else {
                continue;
            };
            chain.push((fb_id, self.loaded_fonts[fb_id.0].family_name.clone()));
        }

        Ok(Arc::from(chain))
    }

    fn load_font(
        &mut self,
        family: &FamilyInfo,
        family_index: usize,
        font_info: &FontInfo,
        features: &FontFeatures,
        fallbacks: Option<FontFallbacks>,
        user_fallback_chain: Arc<[(FontId, SharedString)]>,
    ) -> Result<Option<FontId>> {
        let handle = FontiqueHandleKey {
            family_id: family.id().to_u64(),
            family_index,
        };
        let loaded_key = LoadedFontKey {
            handle,
            features: features.clone(),
            fallbacks: fallbacks.clone(),
        };
        if let Some(font_id) = self.font_ids_by_loaded_font_key.get(&loaded_key) {
            return Ok(Some(*font_id));
        }

        let blob = font_info
            .load(Some(&mut self.font_context.source_cache))
            .context("could not load font data from Fontique")?;
        let font_ref = SwashFontRef::from_index(blob.as_ref(), font_info.index() as usize)
            .context("could not read font data loaded from Fontique")?;
        let postscript_name = font_ref
            .localized_strings()
            .find_by_id(StringId::PostScript, None)
            .map(|name| name.chars().collect::<String>());

        let allowed_bad_font_names = ["SegoeFluentIcons", "Segoe Fluent Icons"];

        if font_ref.charmap().map('m') == 0
            && !postscript_name
                .as_deref()
                .is_some_and(|name| allowed_bad_font_names.contains(&name))
        {
            return Ok(None);
        }

        let font_id = FontId(self.loaded_fonts.len());
        let family_name = SharedString::from(family.name().to_string());
        let is_known_emoji_font = postscript_name
            .as_deref()
            .is_some_and(check_is_known_emoji_font)
            || check_is_known_emoji_font(family.name());

        self.loaded_fonts.push(LoadedFont {
            id: font_id,
            family_name,
            postscript_name,
            style: font_info.style(),
            weight: font_info.weight(),
            stretch: font_info.width(),
            font_data: parley::FontData::new(blob, font_info.index()),
            features: features.clone(),
            is_known_emoji_font,
            user_fallback_chain,
            fontique_handle: handle,
        });
        self.font_ids_by_loaded_font_key.insert(loaded_key, font_id);
        self.font_id_by_fontique_handle
            .entry(handle)
            .or_insert(font_id);

        Ok(Some(font_id))
    }

    fn loaded_font(&self, font_id: FontId) -> Option<&LoadedFont> {
        self.loaded_fonts.get(font_id.0)
    }

    fn font_ref(&self, font_id: FontId) -> Result<SkrifaFontRef<'_>> {
        let loaded = self
            .loaded_font(font_id)
            .with_context(|| format!("font id {} was not loaded", font_id.0))?;
        SkrifaFontRef::from_index(loaded.font_data.data.as_ref(), loaded.font_data.index)
            .context("could not read font data loaded from Fontique")
    }

    fn font_metrics(&self, font_id: FontId) -> Option<FontMetrics> {
        let font_ref = self.font_ref(font_id).ok()?;
        let metrics = font_ref.metrics(SkrifaSize::unscaled(), LocationRef::default());
        let units_per_em = if metrics.units_per_em == 0 {
            1000
        } else {
            u32::from(metrics.units_per_em)
        };
        let fallback_height = units_per_em as f32;
        let (ascent, descent) = if metrics.ascent == 0.0 && metrics.descent == 0.0 {
            (fallback_height * 0.8, -(fallback_height * 0.2))
        } else {
            (metrics.ascent, metrics.descent)
        };
        let cap_height = metrics.cap_height.unwrap_or(ascent);
        let x_height = metrics.x_height.unwrap_or(cap_height * 0.7);
        let underline = metrics.underline.unwrap_or(skrifa::metrics::Decoration {
            offset: -(fallback_height * 0.1),
            thickness: fallback_height * 0.05,
        });
        let bounding_box = metrics.bounds.map(bounds_from_skrifa).unwrap_or(Bounds {
            origin: gpui::Point { x: 0.0, y: descent },
            size: Size {
                width: metrics
                    .max_width
                    .or(metrics.average_width)
                    .unwrap_or(fallback_height),
                height: ascent - descent,
            },
        });

        Some(FontMetrics {
            units_per_em,
            ascent,
            descent,
            line_gap: metrics.leading,
            underline_position: underline.offset,
            underline_thickness: underline.thickness,
            cap_height,
            x_height,
            bounding_box,
        })
    }

    fn typographic_bounds(&self, font_id: FontId, glyph_id: GlyphId) -> Result<Bounds<f32>> {
        let font_ref = self.font_ref(font_id)?;
        let glyph_metrics = font_ref.glyph_metrics(SkrifaSize::unscaled(), LocationRef::default());
        let glyph_id = SkrifaGlyphId::new(glyph_id.0);
        if let Some(bounds) = glyph_metrics.bounds(glyph_id) {
            Ok(bounds_from_skrifa(bounds))
        } else {
            let advance = self.advance(font_id, GlyphId(glyph_id.to_u32()))?;
            Ok(Bounds {
                origin: gpui::Point { x: 0.0, y: 0.0 },
                size: advance,
            })
        }
    }

    fn advance(&self, font_id: FontId, glyph_id: GlyphId) -> Result<Size<f32>> {
        let font_ref = self.font_ref(font_id)?;
        let glyph_metrics = font_ref.glyph_metrics(SkrifaSize::unscaled(), LocationRef::default());
        let width = glyph_metrics
            .advance_width(SkrifaGlyphId::new(glyph_id.0))
            .with_context(|| {
                format!(
                    "glyph id {} is outside the glyph range for font id {}",
                    glyph_id.0, font_id.0
                )
            })?;

        Ok(Size { width, height: 0.0 })
    }

    fn glyph_for_char(&self, font_id: FontId, ch: char) -> Option<GlyphId> {
        let font_ref = self.font_ref(font_id).ok()?;
        let glyph_id = font_ref.charmap().map(ch)?;
        (glyph_id != SkrifaGlyphId::NOTDEF).then_some(GlyphId(glyph_id.to_u32()))
    }
}

fn bounds_from_skrifa(bounds: skrifa::metrics::BoundingBox) -> Bounds<f32> {
    Bounds {
        origin: gpui::Point {
            x: bounds.x_min,
            y: bounds.y_min,
        },
        size: Size {
            width: bounds.x_max - bounds.x_min,
            height: bounds.y_max - bounds.y_min,
        },
    }
}

fn find_best_match(
    font: &Font,
    candidates: &[FontId],
    state: &ParleyTextSystemState,
) -> Result<usize> {
    if candidates.is_empty() {
        anyhow::bail!("requested font family contains no font matching the other parameters");
    }
    if candidates.len() == 1 {
        return Ok(0);
    }

    let target_weight = font.weight.0;
    let target_style = font_style_into_fontique(font.style);
    let mut best_index = 0;
    let mut best_score = f32::MAX;

    for (index, font_id) in candidates.iter().enumerate() {
        let loaded = state
            .loaded_font(*font_id)
            .context("font id candidate was not loaded")?;
        let score = style_penalty(target_style, loaded.style)
            + (loaded.weight.value() - target_weight).abs()
            + ((loaded.stretch.ratio() - fontique::FontWidth::NORMAL.ratio()).abs() * 100.0);

        if score < best_score {
            best_score = score;
            best_index = index;
        }
    }

    Ok(best_index)
}

fn font_style_into_fontique(style: gpui::FontStyle) -> fontique::FontStyle {
    match style {
        gpui::FontStyle::Normal => fontique::FontStyle::Normal,
        gpui::FontStyle::Italic => fontique::FontStyle::Italic,
        gpui::FontStyle::Oblique => fontique::FontStyle::Oblique(None),
    }
}

fn style_penalty(target: fontique::FontStyle, candidate: fontique::FontStyle) -> f32 {
    if target == candidate {
        0.0
    } else if is_sloped_style(target) == is_sloped_style(candidate) {
        100.0
    } else {
        1000.0
    }
}

fn is_sloped_style(style: fontique::FontStyle) -> bool {
    matches!(
        style,
        fontique::FontStyle::Italic | fontique::FontStyle::Oblique(_)
    )
}

fn check_is_known_emoji_font(name: &str) -> bool {
    matches!(
        name,
        "Apple Color Emoji" | "Noto Color Emoji" | "NotoColorEmoji" | "Segoe UI Emoji"
    ) || name.contains("Emoji")
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{FontWeight, font};

    const LILEX_REGULAR: &[u8] = include_bytes!("../../../assets/fonts/lilex/Lilex-Regular.ttf");
    const LILEX_BOLD: &[u8] = include_bytes!("../../../assets/fonts/lilex/Lilex-Bold.ttf");
    const LILEX_ITALIC: &[u8] = include_bytes!("../../../assets/fonts/lilex/Lilex-Italic.ttf");
    const IBM_PLEX_SANS_REGULAR: &[u8] =
        include_bytes!("../../../assets/fonts/ibm-plex-sans/IBMPlexSans-Regular.ttf");

    fn text_system_with_memory_fonts() -> ParleyTextSystem {
        let text_system = ParleyTextSystem::new_without_system_fonts("IBM Plex Sans");
        text_system
            .add_fonts(vec![
                Cow::Borrowed(LILEX_REGULAR),
                Cow::Borrowed(LILEX_BOLD),
                Cow::Borrowed(LILEX_ITALIC),
                Cow::Borrowed(IBM_PLEX_SANS_REGULAR),
            ])
            .unwrap();
        text_system
    }

    #[test]
    fn memory_font_appears_in_all_font_names() {
        let text_system = text_system_with_memory_fonts();
        let names = text_system.all_font_names();

        assert!(names.iter().any(|name| name == "Lilex"));
        assert!(names.iter().any(|name| name == "IBM Plex Sans"));
    }

    #[test]
    fn same_font_descriptor_returns_stable_font_id() {
        let text_system = text_system_with_memory_fonts();
        let descriptor = font("Lilex");

        assert_eq!(
            text_system.font_id(&descriptor).unwrap(),
            text_system.font_id(&descriptor).unwrap()
        );
    }

    #[test]
    fn missing_font_returns_error_for_text_system_fallback_stack() {
        let text_system = text_system_with_memory_fonts();

        assert!(text_system.font_id(&font("Missing Family")).is_err());
    }

    #[test]
    fn distinct_feature_sets_return_distinct_font_ids() {
        let text_system = text_system_with_memory_fonts();
        let default_features = font("Lilex");
        let mut disabled_ligatures = font("Lilex");
        disabled_ligatures.features = FontFeatures::disable_ligatures();

        assert_ne!(
            text_system.font_id(&default_features).unwrap(),
            text_system.font_id(&disabled_ligatures).unwrap()
        );
    }

    #[test]
    fn best_face_matches_weight_and_style() {
        let text_system = text_system_with_memory_fonts();
        let regular = font("Lilex");
        let mut bold = font("Lilex");
        bold.weight = FontWeight::BOLD;
        let italic = font("Lilex").italic();

        let regular_id = text_system.font_id(&regular).unwrap();
        let bold_id = text_system.font_id(&bold).unwrap();
        let italic_id = text_system.font_id(&italic).unwrap();

        assert_ne!(regular_id, bold_id);
        assert_ne!(regular_id, italic_id);
    }

    #[test]
    fn user_fallback_chain_resolves_before_system_fallback() {
        let text_system = text_system_with_memory_fonts();
        let mut descriptor = font("Lilex");
        descriptor.fallbacks = Some(FontFallbacks::from_fonts(vec![
            "IBM Plex Sans".to_string(),
            "Missing Family".to_string(),
        ]));

        let font_id = text_system.font_id(&descriptor).unwrap();
        let state = text_system.state.read();
        let loaded = state.loaded_font(font_id).unwrap();

        assert_eq!(loaded.user_fallback_chain.len(), 1);
        assert_eq!(loaded.user_fallback_chain[0].1.as_ref(), "IBM Plex Sans");
        let fallback_id = loaded.user_fallback_chain[0].0;
        assert!(
            state
                .loaded_font(fallback_id)
                .unwrap()
                .user_fallback_chain
                .is_empty()
        );
    }

    #[test]
    fn font_metrics_are_read_from_loaded_font_data() {
        let text_system = text_system_with_memory_fonts();
        let font_id = text_system.font_id(&font("Lilex")).unwrap();
        let metrics = text_system.font_metrics(font_id);

        assert!(metrics.units_per_em > 0);
        assert!(metrics.ascent > 0.0);
        assert!(metrics.descent < 0.0);
        assert!(metrics.cap_height > 0.0);
        assert!(metrics.x_height > 0.0);
        assert!(metrics.bounding_box.size.width > 0.0);
        assert!(metrics.bounding_box.size.height > 0.0);
    }

    #[test]
    fn font_metrics_are_deterministic() {
        let text_system = text_system_with_memory_fonts();
        let font_id = text_system.font_id(&font("IBM Plex Sans")).unwrap();
        let first = text_system.font_metrics(font_id);
        let second = text_system.font_metrics(font_id);

        assert_eq!(first.units_per_em, second.units_per_em);
        assert_eq!(first.ascent, second.ascent);
        assert_eq!(first.descent, second.descent);
        assert_eq!(first.cap_height, second.cap_height);
        assert_eq!(first.x_height, second.x_height);
        assert_eq!(first.bounding_box.origin.x, second.bounding_box.origin.x);
        assert_eq!(first.bounding_box.origin.y, second.bounding_box.origin.y);
        assert_eq!(
            first.bounding_box.size.width,
            second.bounding_box.size.width
        );
        assert_eq!(
            first.bounding_box.size.height,
            second.bounding_box.size.height
        );
    }

    #[test]
    fn glyph_for_char_uses_loaded_font_charmap() {
        let text_system = text_system_with_memory_fonts();
        let font_id = text_system.font_id(&font("Lilex")).unwrap();

        assert!(text_system.glyph_for_char(font_id, 'm').is_some());
        assert!(text_system.glyph_for_char(font_id, '\u{1f4a9}').is_none());
    }

    #[test]
    fn advance_and_typographic_bounds_use_loaded_font_metrics() {
        let text_system = text_system_with_memory_fonts();
        let font_id = text_system.font_id(&font("Lilex")).unwrap();
        let glyph_id = text_system.glyph_for_char(font_id, 'm').unwrap();

        let advance = text_system.advance(font_id, glyph_id).unwrap();
        let bounds = text_system.typographic_bounds(font_id, glyph_id).unwrap();

        assert!(advance.width > 0.0);
        assert_eq!(advance.height, 0.0);
        assert!(bounds.size.width > 0.0);
        assert!(bounds.size.height > 0.0);
    }
}
