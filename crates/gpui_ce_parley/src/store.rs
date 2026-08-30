use anyhow::{Context as _, Result, bail};
use fontique::{Blob, Synthesis};
use gpui::{
    Bounds, FontId, FontMetrics, GlyphId, RasterizedGlyph, RenderGlyphParams, SUBPIXEL_VARIANTS_X,
    SUBPIXEL_VARIANTS_Y, Size, point, size,
};
use skrifa::{
    FontRef, MetadataProvider as _, Tag,
    instance::{Location, NormalizedCoord, Size as SkrifaSize},
};
use std::collections::HashMap;
use swash::{
    CacheKey as SwashCacheKey, FontRef as SwashFontRef,
    scale::{Render, ScaleContext, Source, StrikeWith},
    zeno::{Angle, Format, Transform, Vector},
};

const CANONICAL_FONT_ID_BIT: usize = 1 << (usize::BITS - 1);

/// The identity assigned to one immutable font-data blob.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct SourceIdentity(u64);

impl SourceIdentity {
    /// Returns the stable identity carried by a Fontique blob.
    fn of(data: &Blob<u8>) -> Self {
        Self(data.id())
    }
}

/// Synthetic outline changes which affect raster output.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct SynthesisKey {
    embolden: bool,
    skew_bits: Option<u32>,
}

impl From<Synthesis> for SynthesisKey {
    fn from(synthesis: Synthesis) -> Self {
        Self {
            embolden: synthesis.embolden(),
            skew_bits: synthesis.skew().map(f32::to_bits),
        }
    }
}

/// Full identity of a selected font instance.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct FontKey {
    source_identity: SourceIdentity,
    face_index: u32,
    normalized_coords: Vec<NormalizedCoord>,
    synthesis: SynthesisKey,
}

/// A concrete font face and variation instance selected for GPUI.
#[derive(Clone, Debug)]
pub(crate) struct LoadedFont {
    /// Immutable bytes containing the selected face.
    pub(crate) data: Blob<u8>,
    /// Face index within the font collection.
    pub(crate) index: u32,
    /// Normalized variation coordinates in axis order.
    pub(crate) normalized_coords: Vec<NormalizedCoord>,
    /// Synthetic styling requested for the selected face.
    pub(crate) synthesis: Synthesis,
    swash_cache_key: SwashCacheKey,
}

impl LoadedFont {
    fn skrifa_ref(&self) -> Result<FontRef<'_>> {
        FontRef::from_index(self.data.as_ref(), self.index)
            .context("Skrifa could not parse the stored font face")
    }

    fn swash_ref(&self) -> Result<SwashFontRef<'_>> {
        let mut font = SwashFontRef::from_index(self.data.as_ref(), self.index as usize)
            .context("Swash could not parse the stored font face")?;
        font.key = self.swash_cache_key;
        Ok(font)
    }

    fn location(&self) -> Location {
        let mut location = Location::new(self.normalized_coords.len());
        location
            .coords_mut()
            .copy_from_slice(&self.normalized_coords);
        location
    }

    /// Returns whether this face contains a color glyph table.
    pub(crate) fn has_color_glyphs(&self) -> Result<bool> {
        let font = self.skrifa_ref()?;
        Ok([*b"CBDT", *b"sbix", *b"COLR", *b"SVG "]
            .into_iter()
            .any(|tag| font.table_data(Tag::new(&tag)).is_some()))
    }

    /// Reads global metrics in font units from the canonical bytes.
    pub(crate) fn metrics(&self) -> Result<FontMetrics> {
        let font = self.skrifa_ref()?;
        let location = self.location();
        let metrics = font.metrics(SkrifaSize::unscaled(), &location);
        Ok(FontMetrics {
            units_per_em: metrics.units_per_em.into(),
            ascent: metrics.ascent,
            descent: -metrics.descent,
            line_gap: metrics.leading,
            underline_position: metrics.underline.map_or(0.0, |underline| underline.offset),
            underline_thickness: metrics
                .underline
                .map_or(0.0, |underline| underline.thickness),
            cap_height: metrics.cap_height.unwrap_or(metrics.ascent),
            x_height: metrics.x_height.unwrap_or(metrics.ascent),
            bounding_box: Bounds {
                origin: point(0.0, 0.0),
                size: size(
                    metrics.max_width.unwrap_or(0.0),
                    metrics.ascent - metrics.descent,
                ),
            },
        })
    }

    /// Maps a Unicode scalar to a nominal glyph using the canonical bytes.
    pub(crate) fn glyph_for_char(&self, character: char) -> Result<Option<GlyphId>> {
        Ok(self
            .skrifa_ref()?
            .charmap()
            .map(character)
            .map(|glyph| GlyphId(glyph.to_u32())))
    }

    /// Returns the unscaled advance for a glyph.
    pub(crate) fn advance(&self, glyph_id: GlyphId) -> Result<Size<f32>> {
        let font = self.skrifa_ref()?;
        let location = self.location();
        let metrics = font.glyph_metrics(SkrifaSize::unscaled(), &location);
        let glyph_id = skrifa::GlyphId::new(glyph_id.0);
        Ok(size(metrics.advance_width(glyph_id).unwrap_or(0.0), 0.0))
    }

    /// Returns the glyph's control bounds in font units.
    pub(crate) fn glyph_bounds(&self, glyph_id: GlyphId) -> Result<Bounds<f32>> {
        let font = self.skrifa_ref()?;
        let location = self.location();
        let bounds = font
            .glyph_metrics(SkrifaSize::unscaled(), &location)
            .bounds(skrifa::GlyphId::new(glyph_id.0));
        let Some(bounds) = bounds else {
            return Ok(Bounds::default());
        };
        Ok(Bounds {
            origin: point(bounds.x_min, bounds.y_min),
            size: size(bounds.x_max - bounds.x_min, bounds.y_max - bounds.y_min),
        })
    }
}

/// Canonical storage for concrete font faces and variation instances.
#[derive(Default)]
pub(crate) struct FontStore {
    fonts: Vec<LoadedFont>,
    ids_by_key: HashMap<FontKey, FontId>,
}

impl FontStore {
    /// Interns a face using Fontique's selected variation and synthesis settings.
    pub(crate) fn intern_synthesized(
        &mut self,
        data: Blob<u8>,
        index: u32,
        synthesis: Synthesis,
    ) -> Result<FontId> {
        let normalized_coords = {
            let font = FontRef::from_index(data.as_ref(), index)
                .context("cannot intern a font face Skrifa cannot parse")?;
            font.axes()
                .location(synthesis.variation_settings().iter().copied())
                .coords()
                .to_vec()
        };
        self.intern(data, index, &normalized_coords, synthesis)
    }

    /// Interns a selected font instance and returns its canonical GPUI ID.
    pub(crate) fn intern(
        &mut self,
        data: Blob<u8>,
        index: u32,
        normalized_coords: &[NormalizedCoord],
        synthesis: Synthesis,
    ) -> Result<FontId> {
        FontRef::from_index(data.as_ref(), index)
            .context("cannot intern a font face Skrifa cannot parse")?;
        let key = FontKey {
            source_identity: SourceIdentity::of(&data),
            face_index: index,
            normalized_coords: normalized_coords.to_vec(),
            synthesis: synthesis.into(),
        };
        if let Some(id) = self.ids_by_key.get(&key) {
            return Ok(*id);
        }
        if self.fonts.len() >= CANONICAL_FONT_ID_BIT {
            bail!("canonical font store exhausted its FontId namespace");
        }
        let id = FontId(CANONICAL_FONT_ID_BIT | self.fonts.len());
        self.fonts.push(LoadedFont {
            data,
            index,
            normalized_coords: normalized_coords.to_vec(),
            synthesis,
            swash_cache_key: SwashCacheKey::new(),
        });
        self.ids_by_key.insert(key, id);
        Ok(id)
    }

    /// Returns the stored font for a canonical ID.
    pub(crate) fn get(&self, id: FontId) -> Option<&LoadedFont> {
        canonical_index(id).and_then(|index| self.fonts.get(index))
    }
}

fn canonical_index(id: FontId) -> Option<usize> {
    (id.0 & CANONICAL_FONT_ID_BIT != 0).then_some(id.0 & !CANONICAL_FONT_ID_BIT)
}

/// Swash raster state shared by all canonical font instances.
pub(crate) struct FontRasterizer {
    scale_context: ScaleContext,
}

impl Default for FontRasterizer {
    fn default() -> Self {
        Self {
            scale_context: ScaleContext::new(),
        }
    }
}

impl FontRasterizer {
    /// Produces atlas-compatible alpha, subpixel, or color bitmap bytes.
    pub(crate) fn rasterize_glyph(
        &mut self,
        font: &LoadedFont,
        params: &RenderGlyphParams,
    ) -> Result<RasterizedGlyph> {
        let mut image = self.render_glyph_image(font, params)?;
        let bounds = Bounds {
            origin: point(image.placement.left.into(), (-image.placement.top).into()),
            size: size(image.placement.width.into(), image.placement.height.into()),
        };
        let pixels = match image.content {
            swash::scale::image::Content::Color | swash::scale::image::Content::SubpixelMask => {
                for pixel in image.data.chunks_exact_mut(4) {
                    pixel.swap(0, 2);
                }
                image.data
            }
            swash::scale::image::Content::Mask if params.subpixel_rendering => {
                image.data.iter().flat_map(|&alpha| [alpha; 4]).collect()
            }
            swash::scale::image::Content::Mask => image.data,
        };
        Ok(RasterizedGlyph {
            bounds,
            size: bounds.size,
            pixels,
        })
    }

    fn render_glyph_image(
        &mut self,
        font: &LoadedFont,
        params: &RenderGlyphParams,
    ) -> Result<swash::scale::image::Image> {
        let font_ref = font.swash_ref()?;
        let subpixel_offset = Vector::new(
            params.subpixel_variant.x as f32 / SUBPIXEL_VARIANTS_X as f32 / params.scale_factor,
            params.subpixel_variant.y as f32 / SUBPIXEL_VARIANTS_Y as f32 / params.scale_factor,
        );
        let coords = font
            .normalized_coords
            .iter()
            .map(|coord| coord.to_bits())
            .collect::<Vec<_>>();
        let mut scaler = self
            .scale_context
            .builder(font_ref)
            .size(f32::from(params.font_size) * params.scale_factor)
            .normalized_coords(&coords)
            .hint(true)
            .build();
        let sources: &[Source] = if params.is_emoji {
            &[
                Source::ColorOutline(0),
                Source::ColorBitmap(StrikeWith::BestFit),
                Source::Outline,
            ]
        } else {
            &[Source::Bitmap(StrikeWith::ExactSize), Source::Outline]
        };
        let mut renderer = Render::new(sources);
        if params.subpixel_rendering {
            renderer.format(Format::subpixel_bgra());
        } else {
            renderer.format(Format::Alpha);
        }
        renderer.offset(subpixel_offset);
        if font.synthesis.embolden() {
            renderer.embolden(f32::from(params.font_size) * params.scale_factor / 48.0);
        }
        if let Some(degrees) = font.synthesis.skew() {
            renderer.transform(Some(Transform::skew(
                Angle::from_degrees(degrees),
                Angle::ZERO,
            )));
        }
        let glyph_id: u16 = params.glyph_id.0.try_into()?;
        renderer
            .render(&mut scaler, glyph_id)
            .with_context(|| format!("unable to render canonical glyph for {params:?}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const IBM_PLEX: &[u8] =
        include_bytes!("../../../assets/fonts/ibm-plex-sans/IBMPlexSans-Regular.ttf");

    #[test]
    fn identity_includes_blob_face_variations_and_synthesis() {
        let data = Blob::from(IBM_PLEX.to_vec());
        let mut store = FontStore::default();
        let first = store
            .intern(data.clone(), 0, &[], Synthesis::default())
            .unwrap();
        let duplicate = store
            .intern(data.clone(), 0, &[], Synthesis::default())
            .unwrap();
        assert_eq!(first, duplicate);

        let varied = store
            .intern(
                data,
                0,
                &[NormalizedCoord::from_f32(0.5)],
                Synthesis::default(),
            )
            .unwrap();
        assert_ne!(first, varied);

        let copied_source = store
            .intern(Blob::from(IBM_PLEX.to_vec()), 0, &[], Synthesis::default())
            .unwrap();
        assert_ne!(first, copied_source);
        assert!(store.get(first).is_some());
        assert!(store.get(FontId(0)).is_none());
    }
}
