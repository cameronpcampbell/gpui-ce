use core_foundation_sys::{
    array::CFArrayRef, data::CFDataRef, preferences::CFPreferencesCopyAppValue,
    preferences::kCFPreferencesCurrentApplication,
};

#[cfg(test)]
use gpui::{FontId, GlyphId, PlatformTextSystem, font as gpui_font, px, rgba};

#[cfg(test)]
use gpui_parley::{FontSynthesis, ParleyTextSystem, SystemFonts};

#[cfg(test)]
use std::borrow::Cow;

use anyhow::{Context as _, Result, anyhow, ensure};
use core_foundation::{
    array::CFArray,
    base::{CFType, TCFType},
    data::CFData,
    dictionary::CFDictionary,
    number::CFNumber,
    string::CFString,
};
use core_graphics::{
    base::{CGFloat, kCGImageAlphaPremultipliedLast},
    color_space::CGColorSpace,
    context::{CGContext, CGTextDrawingMode},
    display::CGPoint,
    geometry::CGAffineTransform,
};
use core_text::{
    font,
    font_descriptor::{self, CTFontDescriptor, kCTFontOrientationDefault},
};
use gpui::{
    Bounds, DevicePixels, GlyphRenderMode, PreparedRasterStyle, RasterColorEffect,
    RasterStyleRequest, RasterizedGlyph, RasterizedGlyphFormat, RenderGlyphParams, Rgba8,
    SUBPIXEL_VARIANTS_X, SUBPIXEL_VARIANTS_Y, TextRenderingMode, point, size,
};
use gpui_parley::{GlyphRasterizer, RasterFace};
use objc2::rc::autoreleasepool;
use std::{
    collections::HashMap,
    f64::consts::PI,
    sync::{Arc, OnceLock},
};

const TTC_TAG: &[u8; 4] = b"ttcf";
const CHECKSUM_MAGIC: u32 = 0xb1b0_afba;

#[link(name = "CoreText", kind = "framework")]
unsafe extern "C" {
    fn CTFontManagerCreateFontDescriptorsFromData(data: CFDataRef) -> CFArrayRef;
}

#[allow(non_upper_case_globals)]
const kCGImageAlphaOnly: u32 = 7;

/// CoreText and CoreGraphics rasterization for the exact face selected by Parley.
pub(crate) struct MacGlyphRasterizer {
    faces: HashMap<gpui::FontId, NativeFace>,
    sources: HashMap<u64, Arc<SendCFData>>,
}

struct NativeFace {
    descriptor: CTFontDescriptor,
    // CoreText may defer reading tables from descriptors created from in-memory data until a
    // sized CTFont first draws. Keep the descriptor's source alive for the full cached-face
    // lifetime, as the pre-Parley backend did through its retained CGFont.
    _source_data: Arc<SendCFData>,
}

/// An immutable Core Foundation data object retained by the serialized macOS rasterizer.
struct SendCFData {
    data: CFData,
}

// SAFETY: CFData is immutable, and MacGlyphRasterizer only accesses native faces while its
// enclosing mutex is held. The value is retained solely to extend the source data's lifetime.
unsafe impl Send for SendCFData {}
// SAFETY: CFData is immutable, so retaining it from multiple native-face entries is safe.
unsafe impl Sync for SendCFData {}

impl MacGlyphRasterizer {
    pub(crate) fn new() -> Self {
        Self {
            faces: HashMap::default(),
            sources: HashMap::default(),
        }
    }

    fn native_face(&mut self, face: &RasterFace<'_>) -> Result<&NativeFace> {
        if self.faces.contains_key(&face.font_id) {
            return Ok(&self.faces[&face.font_id]);
        }

        let source = if let Some(source) = self.sources.get(&face.source_id) {
            source.clone()
        } else {
            source_from_bytes(face.data.to_vec())
        };

        let native =
            autoreleasepool(|_| NativeFace::new(face, source.clone())).with_context(|| {
                format!(
                    "CoreText could not create FontId {:?}, face index {}, variations {:?}",
                    face.font_id, face.face_index, face.variations
                )
            })?;
        if Arc::ptr_eq(&native._source_data, &source) {
            self.sources
                .entry(face.source_id)
                .or_insert_with(|| source.clone());
        }
        self.faces.insert(face.font_id, native);
        Ok(&self.faces[&face.font_id])
    }

    fn rasterize_inner(
        &mut self,
        face: &RasterFace<'_>,
        params: &RenderGlyphParams,
    ) -> Result<RasterizedGlyph> {
        let native = self.native_face(face)?;
        let font_size = f64::from(params.font_size);
        let scale_factor = f64::from(params.scale_factor);
        ensure!(
            font_size.is_finite() && font_size >= 0.0,
            "invalid font size"
        );
        ensure!(
            scale_factor.is_finite() && scale_factor > 0.0,
            "invalid raster scale factor"
        );
        let font = font::new_from_descriptor(&native.descriptor, font_size);
        let glyph: u16 = params
            .glyph_id
            .0
            .try_into()
            .context("CoreText glyph IDs are 16-bit")?;

        let skew = face
            .synthesis
            .skew_degrees
            .map_or(0.0, |degrees| f64::from(degrees) * PI / 180.0)
            .tan();
        let text_matrix = CGAffineTransform::new(1.0, 0.0, skew, 1.0, 0.0, 0.0);
        let glyph_rect = font
            .get_bounding_rects_for_glyphs(kCTFontOrientationDefault, &[glyph])
            .apply_transform(&text_matrix);

        if glyph_rect.is_empty() || glyph_rect.size.width <= 0.0 || glyph_rect.size.height <= 0.0 {
            return Ok(RasterizedGlyph::empty(format_for_mode(
                params.raster_style.mode,
            )));
        }

        let embolden = if face.synthesis.embolden {
            font_size / 48.0
        } else {
            0.0
        };

        let padding = (embolden * scale_factor).ceil() + 1.0;
        let left = (glyph_rect.origin.x * scale_factor - padding).floor();
        let mut right =
            ((glyph_rect.origin.x + glyph_rect.size.width) * scale_factor + padding).ceil();
        let top =
            (-(glyph_rect.origin.y + glyph_rect.size.height) * scale_factor - padding).floor();
        let bottom = (-glyph_rect.origin.y * scale_factor + padding).ceil();

        if params.subpixel_variant.x > 0 {
            right += 1.0;
        }

        let width = (right - left) as i32;
        let height = (bottom - top) as i32;

        if width <= 0 || height <= 0 {
            return Ok(RasterizedGlyph::empty(format_for_mode(
                params.raster_style.mode,
            )));
        }

        let format = format_for_mode(params.raster_style.mode);
        let bytes_per_pixel = if format == RasterizedGlyphFormat::BgraColor {
            4
        } else {
            1
        };

        let mut pixels = vec![0; width as usize * height as usize * bytes_per_pixel];
        {
            let color_space = if bytes_per_pixel == 4 {
                CGColorSpace::create_device_rgb()
            } else {
                CGColorSpace::create_device_gray()
            };

            let context = CGContext::create_bitmap_context(
                Some(pixels.as_mut_ptr().cast()),
                width as usize,
                height as usize,
                8,
                width as usize * bytes_per_pixel,
                &color_space,
                if bytes_per_pixel == 4 {
                    kCGImageAlphaPremultipliedLast
                } else {
                    kCGImageAlphaOnly
                },
            );
            configure_context(
                &context,
                params.raster_style,
                face.synthesis.embolden,
                embolden,
                text_matrix,
            );
            context.translate(-left, top + f64::from(height));
            context.scale(scale_factor, scale_factor);
            let offset = CGPoint::new(
                f64::from(params.subpixel_variant.x)
                    / f64::from(SUBPIXEL_VARIANTS_X)
                    / scale_factor,
                f64::from(params.subpixel_variant.y)
                    / f64::from(SUBPIXEL_VARIANTS_Y)
                    / scale_factor,
            );
            font.draw_glyphs(&[glyph], &[offset], context);
        }

        if format == RasterizedGlyphFormat::BgraColor {
            for pixel in pixels.chunks_exact_mut(4) {
                gpui::swap_rgba_pa_to_bgra(pixel);
            }
        }

        Ok(RasterizedGlyph {
            bounds: Bounds {
                origin: point(DevicePixels(left as i32), DevicePixels(top as i32)),
                size: size(DevicePixels(width), DevicePixels(height)),
            },
            size: size(DevicePixels(width), DevicePixels(height)),
            format,
            pixels,
        })
    }
}

impl GlyphRasterizer for MacGlyphRasterizer {
    fn prepare_style(&self, request: RasterStyleRequest) -> PreparedRasterStyle {
        if request.requested_mode == GlyphRenderMode::Color {
            return PreparedRasterStyle {
                mode: GlyphRenderMode::Color,
                color_effect: RasterColorEffect::Preblend(request.scene_color.into()),
            };
        }

        let color_effect = if font_smoothing_allowed_by_user() {
            let color = request.scene_color;
            let luminance = 0.2126 * color.red + 0.7152 * color.green + 0.0722 * color.blue;
            let dilation = ((4.0 * luminance) + 0.5).floor().clamp(0.0, 4.0) as u8;
            RasterColorEffect::Dilation(dilation)
        } else {
            RasterColorEffect::Dilation(0)
        };

        PreparedRasterStyle {
            mode: GlyphRenderMode::Grayscale,
            color_effect,
        }
    }

    fn rasterize(
        &mut self,
        face: RasterFace<'_>,
        params: &RenderGlyphParams,
    ) -> Result<RasterizedGlyph> {
        autoreleasepool(|_| self.rasterize_inner(&face, params))
    }

    fn recommended_mode(&self) -> TextRenderingMode {
        TextRenderingMode::Grayscale
    }
}

impl NativeFace {
    fn new(face: &RasterFace<'_>, shared_source: Arc<SendCFData>) -> Result<Self> {
        let (mut descriptor, source_data) = if face.data.get(..4) == Some(TTC_TAG) {
            match collection_descriptor(&shared_source.data, face.data, face.face_index) {
                Ok(descriptor) => (descriptor, shared_source),
                Err(error) => {
                    log::debug!(
                        "CoreText could not select collection face {}; using a compact SFNT: {error:#}",
                        face.face_index
                    );

                    let source =
                        source_from_bytes(compact_sfnt_for_face(face.data, face.face_index)?);
                    let descriptor = core_text::font_manager::create_font_descriptor_with_data(
                        source.data.clone(),
                    )
                    .map_err(|()| anyhow!("CoreText rejected the extracted font face"))?;
                    (descriptor, source)
                }
            }
        } else {
            ensure!(
                face.face_index == 0,
                "single font contains only face 0, requested {}",
                face.face_index
            );
            let descriptor = core_text::font_manager::create_font_descriptor_with_data(
                shared_source.data.clone(),
            )
            .map_err(|()| anyhow!("CoreText rejected the selected font face"))?;
            (descriptor, shared_source)
        };

        if !face.variations.is_empty() {
            let variations = face
                .variations
                .iter()
                .map(|variation| {
                    let tag = u32::from_be_bytes(variation.tag.to_be_bytes());
                    (
                        CFNumber::from(i64::from(tag)),
                        CFNumber::from(f64::from(variation.value)),
                    )
                })
                .collect::<Vec<_>>();
            let variations = CFDictionary::from_CFType_pairs(&variations);
            let variation_key = unsafe {
                CFString::wrap_under_get_rule(font_descriptor::kCTFontVariationAttribute)
            };

            let variation_value = unsafe { CFType::wrap_under_get_rule(variations.as_CFTypeRef()) };

            let attributes =
                CFDictionary::from_CFType_pairs(&[(variation_key, variation_value)]).into_untyped();
            descriptor = descriptor
                .create_copy_with_attributes(attributes)
                .map_err(|()| anyhow!("CoreText rejected the variation coordinates"))?;
        }

        Ok(Self {
            descriptor,
            _source_data: source_data,
        })
    }
}

fn source_from_bytes(bytes: Vec<u8>) -> Arc<SendCFData> {
    Arc::new(SendCFData {
        data: CFData::from_arc(Arc::new(bytes)),
    })
}

fn collection_descriptor(
    source: &CFData,
    data: &[u8],
    face_index: u32,
) -> Result<CTFontDescriptor> {
    let descriptors_ref =
        unsafe { CTFontManagerCreateFontDescriptorsFromData(source.as_concrete_TypeRef()) };
    ensure!(
        !descriptors_ref.is_null(),
        "CoreText rejected the font collection"
    );
    let descriptors =
        unsafe { CFArray::<CTFontDescriptor>::wrap_under_create_rule(descriptors_ref) };
    let face_offset = collection_face_offset(data, face_index)?;
    for descriptor in &descriptors {
        if descriptor_matches_face(&descriptor, data, face_offset)? {
            return Ok(descriptor.clone());
        }
    }

    Err(anyhow!(
        "none of CoreText's {} descriptors matched physical face {face_index}",
        descriptors.len()
    ))
}

fn descriptor_matches_face(
    descriptor: &CTFontDescriptor,
    data: &[u8],
    face_offset: usize,
) -> Result<bool> {
    let native_font = font::new_from_descriptor(descriptor, 0.0);

    // CoreText expands variable faces into named-instance descriptors, so descriptor array
    // positions are not collection face indexes. These raw tables identify the physical face.
    for tag in [b"name", b"head", b"maxp"] {
        let Some(expected) = face_table(data, face_offset, tag)? else {
            return Ok(false);
        };
        let Some(actual) = native_font.get_font_table(u32::from_be_bytes(*tag)) else {
            return Ok(false);
        };

        if !identity_table_matches(tag, actual.bytes(), expected) {
            return Ok(false);
        }
    }

    Ok(true)
}

fn identity_table_matches(tag: &[u8; 4], actual: &[u8], expected: &[u8]) -> bool {
    if tag != b"head" {
        return actual == expected;
    }

    // CoreText clears checkSumAdjustment when exposing a face from a collection.
    actual.len() >= 12
        && actual.len() == expected.len()
        && actual[..8] == expected[..8]
        && actual[12..] == expected[12..]
}

fn compact_sfnt_for_face(data: &[u8], face_index: u32) -> Result<Vec<u8>> {
    ensure!(
        data.get(..4) == Some(TTC_TAG),
        "compact extraction requires a font collection"
    );
    let face_offset = collection_face_offset(data, face_index)?;
    let table_count =
        read_u16(data, face_offset + 4).context("truncated selected SFNT header")? as usize;
    let directory_len = 12usize
        .checked_add(
            table_count
                .checked_mul(16)
                .context("selected SFNT table count overflow")?,
        )
        .context("selected SFNT directory length overflow")?;
    let directory = data
        .get(
            face_offset
                ..face_offset
                    .checked_add(directory_len)
                    .context("selected SFNT directory offset overflow")?,
        )
        .context("truncated selected SFNT directory")?;
    let mut sfnt = directory.to_vec();
    let mut head_offset = None;

    for table_idx in 0..table_count {
        let record_position = 12 + table_idx * 16;
        let tag: [u8; 4] = directory[record_position..record_position + 4]
            .try_into()
            .expect("validated table record width");
        let source_offset = read_u32(directory, record_position + 8)
            .context("truncated selected SFNT table offset")? as usize;
        let table_len = read_u32(directory, record_position + 12)
            .context("truncated selected SFNT table length")? as usize;
        let table = data
            .get(
                source_offset
                    ..source_offset
                        .checked_add(table_len)
                        .context("selected SFNT table end overflow")?,
            )
            .context("truncated selected SFNT table")?;

        pad_to_u32(&mut sfnt);
        let target_offset = sfnt.len();
        let target_offset_u32 = u32::try_from(target_offset)
            .context("extracted SFNT exceeds the OpenType offset range")?;
        sfnt[record_position + 8..record_position + 12]
            .copy_from_slice(&target_offset_u32.to_be_bytes());
        sfnt.extend_from_slice(table);

        if &tag == b"head" {
            ensure!(table_len >= 12, "truncated selected SFNT head table");
            head_offset = Some(target_offset);
        }
    }

    pad_to_u32(&mut sfnt);

    let head_offset = head_offset.context("selected SFNT has no head table")?;
    sfnt[head_offset + 8..head_offset + 12].fill(0);
    let adjustment = CHECKSUM_MAGIC.wrapping_sub(sfnt_checksum(&sfnt));
    sfnt[head_offset + 8..head_offset + 12].copy_from_slice(&adjustment.to_be_bytes());

    Ok(sfnt)
}

fn collection_face_offset(data: &[u8], face_index: u32) -> Result<usize> {
    let face_count = read_u32(data, 8).context("truncated font collection header")?;
    ensure!(
        face_index < face_count,
        "collection contains {face_count} faces, requested {face_index}"
    );

    let offset_position = 12usize
        .checked_add(face_index as usize * 4)
        .context("font collection face offset overflow")?;

    Ok(read_u32(data, offset_position).context("truncated font collection face offsets")? as usize)
}

fn face_table<'a>(
    data: &'a [u8],
    face_offset: usize,
    target_tag: &[u8; 4],
) -> Result<Option<&'a [u8]>> {
    let table_count =
        read_u16(data, face_offset + 4).context("truncated selected SFNT header")? as usize;

    for table_idx in 0..table_count {
        let record_position = face_offset
            .checked_add(12 + table_idx * 16)
            .context("selected SFNT table record overflow")?;
        let tag = data
            .get(record_position..record_position + 4)
            .context("truncated selected SFNT table tag")?;

        if tag != target_tag {
            continue;
        }

        let table_offset = read_u32(data, record_position + 8)
            .context("truncated selected SFNT table offset")? as usize;
        let table_len = read_u32(data, record_position + 12)
            .context("truncated selected SFNT table length")? as usize;
        let table_end = table_offset
            .checked_add(table_len)
            .context("selected SFNT table end overflow")?;

        return Ok(Some(
            data.get(table_offset..table_end)
                .context("truncated selected SFNT table")?,
        ));
    }

    Ok(None)
}

fn pad_to_u32(data: &mut Vec<u8>) {
    let padding = (4 - data.len() % 4) % 4;
    data.resize(data.len() + padding, 0);
}

fn sfnt_checksum(data: &[u8]) -> u32 {
    data.chunks_exact(4).fold(0, |checksum, bytes| {
        checksum.wrapping_add(u32::from_be_bytes(
            bytes.try_into().expect("four-byte checksum chunk"),
        ))
    })
}

fn read_u16(data: &[u8], offset: usize) -> Option<u16> {
    Some(u16::from_be_bytes(
        data.get(offset..offset + 2)?.try_into().ok()?,
    ))
}

fn read_u32(data: &[u8], offset: usize) -> Option<u32> {
    Some(u32::from_be_bytes(
        data.get(offset..offset + 4)?.try_into().ok()?,
    ))
}

fn configure_context(
    context: &CGContext,
    style: PreparedRasterStyle,
    embolden: bool,
    embolden_amount: CGFloat,
    text_matrix: CGAffineTransform,
) {
    context.set_text_drawing_mode(if embolden {
        CGTextDrawingMode::CGTextFillStroke
    } else {
        CGTextDrawingMode::CGTextFill
    });

    context.set_text_matrix(&text_matrix);
    context.set_allows_antialiasing(true);
    context.set_should_antialias(true);
    context.set_allows_font_subpixel_positioning(true);
    context.set_should_subpixel_position_fonts(true);
    context.set_allows_font_subpixel_quantization(false);
    context.set_should_subpixel_quantize_fonts(false);
    context.set_line_width(embolden_amount * 2.0);

    match style.color_effect {
        RasterColorEffect::Dilation(level) => {
            let luminance = f64::from(level) * 0.25;
            context.set_should_smooth_fonts(level > 0);
            context.set_gray_fill_color(luminance, 1.0);
            context.set_rgb_stroke_color(luminance, luminance, luminance, 1.0);
        }
        RasterColorEffect::Preblend(Rgba8 {
            red,
            green,
            blue,
            alpha,
        }) => {
            let [red, green, blue, alpha] =
                [red, green, blue, alpha].map(|channel| f64::from(channel) / 255.0);
            context.set_rgb_fill_color(red, green, blue, alpha);
            context.set_rgb_stroke_color(red, green, blue, alpha);
        }
        RasterColorEffect::Independent => {
            context.set_gray_fill_color(0.0, 1.0);
            context.set_rgb_stroke_color(0.0, 0.0, 0.0, 1.0);
        }
    }
}

fn format_for_mode(mode: GlyphRenderMode) -> RasterizedGlyphFormat {
    match mode {
        GlyphRenderMode::Color => RasterizedGlyphFormat::BgraColor,
        GlyphRenderMode::Grayscale | GlyphRenderMode::Subpixel => RasterizedGlyphFormat::AlphaMask,
    }
}

fn font_smoothing_allowed_by_user() -> bool {
    static ALLOWED: OnceLock<bool> = OnceLock::new();
    *ALLOWED.get_or_init(|| {
        let key = CFString::new("AppleFontSmoothing");
        let value_ref = unsafe {
            CFPreferencesCopyAppValue(key.as_concrete_TypeRef(), kCFPreferencesCurrentApplication)
        };

        if value_ref.is_null() {
            return true;
        }

        let value = unsafe { CFType::wrap_under_create_rule(value_ref) };

        value
            .downcast_into::<CFNumber>()
            .and_then(|number| number.to_i64())
            != Some(0)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const IBM_PLEX: &[u8] =
        include_bytes!("../../../assets/fonts/ibm-plex-sans/IBMPlexSans-Regular.ttf");
    const IBM_PLEX_ITALIC: &[u8] =
        include_bytes!("../../../assets/fonts/ibm-plex-sans/IBMPlexSans-Italic.ttf");
    const SOURCE_SERIF: &[u8] =
        include_bytes!("../../../assets/fonts/source-serif-4/SourceSerif4[opsz,wght].ttf");

    #[test]
    fn collection_faces_are_selected_and_compacted_by_physical_index() {
        let collection = test_collection(&[SOURCE_SERIF, IBM_PLEX, IBM_PLEX_ITALIC]);
        let source = source_from_bytes(collection.clone());
        let descriptor = collection_descriptor(&source.data, &collection, 1).unwrap();
        let font = font::new_from_descriptor(&descriptor, 16.0);

        assert_eq!(font.postscript_name(), "IBMPlexSans");

        let character = ['A' as u16];
        let mut glyph = [0];
        let mapped =
            unsafe { font.get_glyphs_for_characters(character.as_ptr(), glyph.as_mut_ptr(), 1) };

        assert!(mapped);
        assert_ne!(glyph[0], 0);

        let sfnt = compact_sfnt_for_face(&collection, 1).unwrap();
        assert!(sfnt.len() < collection.len());
        assert_eq!(sfnt_checksum(&sfnt), CHECKSUM_MAGIC);

        let data = CFData::from_arc(Arc::new(sfnt));
        let descriptor = core_text::font_manager::create_font_descriptor_with_data(data).unwrap();
        let font = font::new_from_descriptor(&descriptor, 16.0);
        assert_eq!(font.postscript_name(), "IBMPlexSans");

        let mut rasterizer = MacGlyphRasterizer::new();
        for (font_id, face_index) in [(FontId(1), 1), (FontId(2), 2)] {
            rasterizer
                .native_face(&RasterFace {
                    font_id,
                    source_id: 1,
                    data: &collection,
                    face_index,
                    variations: &[],
                    synthesis: FontSynthesis::default(),
                    has_color_glyphs: false,
                })
                .unwrap();
        }

        assert_eq!(rasterizer.sources.len(), 1);
        assert!(Arc::ptr_eq(
            &rasterizer.faces[&FontId(1)]._source_data,
            &rasterizer.faces[&FontId(2)]._source_data,
        ));
    }

    fn test_collection(faces: &[&[u8]]) -> Vec<u8> {
        let header_len = 12 + faces.len() * 4;
        let mut collection = vec![0; header_len];
        collection[..4].copy_from_slice(TTC_TAG);
        collection[4..8].copy_from_slice(&0x0001_0000u32.to_be_bytes());
        collection[8..12].copy_from_slice(&(faces.len() as u32).to_be_bytes());

        for (face_idx, face) in faces.iter().enumerate() {
            while collection.len() % 4 != 0 {
                collection.push(0);
            }

            let face_offset = collection.len();
            let offset_position = 12 + face_idx * 4;
            collection[offset_position..offset_position + 4]
                .copy_from_slice(&(face_offset as u32).to_be_bytes());
            collection.extend_from_slice(face);

            let table_count = read_u16(face, 4).unwrap() as usize;
            for table_idx in 0..table_count {
                let table_offset_position = face_offset + 12 + table_idx * 16 + 8;
                let table_offset = read_u32(&collection, table_offset_position).unwrap();
                let collection_offset = table_offset + face_offset as u32;
                collection[table_offset_position..table_offset_position + 4]
                    .copy_from_slice(&collection_offset.to_be_bytes());
            }
        }

        collection
    }

    #[test]
    fn in_memory_variable_font_renders_stably_across_glyphs_and_sizes() {
        let system = ParleyTextSystem::new_with_rasterizer(
            SystemFonts::Skip,
            "Source Serif 4",
            MacGlyphRasterizer::new(),
        );
        system.add_fonts(vec![Cow::Borrowed(SOURCE_SERIF)]).unwrap();
        let font_id = system.font_id(&gpui_font("Source Serif 4")).unwrap();
        let render_pass = || {
            "Ag&"
                .chars()
                .enumerate()
                .map(|(idx, character)| {
                    let step = idx as u8;
                    let glyph = system
                        .rasterize_glyph(&RenderGlyphParams {
                            font_id,
                            glyph_id: system.glyph_for_char(font_id, character).unwrap(),
                            font_size: px(12.0 * f32::from(step + 1)),
                            subpixel_variant: point(step, step),
                            scale_factor: 1.0 + f32::from(step) * 0.5,
                            raster_style: PreparedRasterStyle {
                                mode: GlyphRenderMode::Grayscale,
                                color_effect: RasterColorEffect::Dilation(step * 2),
                            },
                        })
                        .unwrap();
                    glyph.validate().unwrap();
                    assert!(
                        glyph.pixels.iter().any(|&coverage| coverage != 0),
                        "'{character}' produced an empty coverage mask"
                    );
                    glyph
                })
                .collect::<Vec<_>>()
        };

        let first_pass = render_pass();
        let second_pass = render_pass();
        for (character, (expected, actual)) in
            "Ag&".chars().zip(first_pass.iter().zip(&second_pass))
        {
            assert_eq!(
                actual.bounds, expected.bounds,
                "bounds changed for '{character}'"
            );
            assert_eq!(
                actual.pixels, expected.pixels,
                "pixels changed for '{character}'"
            );
        }
    }

    #[test]
    fn core_text_obeys_platform_style_mask_color_baseline_and_empty_glyph_behavior() {
        let system = ParleyTextSystem::new_with_rasterizer(
            SystemFonts::Skip,
            "Source Serif 4",
            MacGlyphRasterizer::new(),
        );
        system.add_fonts(vec![Cow::Borrowed(SOURCE_SERIF)]).unwrap();
        let font_id = system
            .font_id(&gpui_font("Source Serif 4").bold().italic())
            .unwrap();

        let render_style = |glyph_id: GlyphId, raster_style, variant| {
            system
                .rasterize_glyph(&RenderGlyphParams {
                    font_id,
                    glyph_id,
                    font_size: px(24.0),
                    subpixel_variant: variant,
                    scale_factor: 2.0,
                    raster_style,
                })
                .unwrap()
        };

        let render = |glyph_id: GlyphId, mode, color, variant| {
            render_style(
                glyph_id,
                system.prepare_raster_style(RasterStyleRequest {
                    scene_color: color,
                    requested_mode: mode,
                }),
                variant,
            )
        };

        let letter = system.glyph_for_char(font_id, 'A').unwrap();
        let normalized_subpixel = system.prepare_raster_style(RasterStyleRequest {
            scene_color: rgba(0x303030ff),
            requested_mode: GlyphRenderMode::Subpixel,
        });

        assert_eq!(normalized_subpixel.mode, GlyphRenderMode::Grayscale);

        let light_style = system.prepare_raster_style(RasterStyleRequest {
            scene_color: rgba(0xffffffff),
            requested_mode: GlyphRenderMode::Grayscale,
        });

        assert_eq!(
            light_style.color_effect,
            RasterColorEffect::Dilation(if font_smoothing_allowed_by_user() {
                4
            } else {
                0
            })
        );

        let undilated = render_style(
            letter,
            PreparedRasterStyle {
                mode: GlyphRenderMode::Grayscale,
                color_effect: RasterColorEffect::Dilation(0),
            },
            point(0, 0),
        );
        let dilated = render_style(
            letter,
            PreparedRasterStyle {
                mode: GlyphRenderMode::Grayscale,
                color_effect: RasterColorEffect::Dilation(4),
            },
            point(0, 0),
        );
        assert_ne!(undilated.pixels, dilated.pixels);

        let shifted = render_style(
            letter,
            PreparedRasterStyle {
                mode: GlyphRenderMode::Grayscale,
                color_effect: RasterColorEffect::Dilation(0),
            },
            point(SUBPIXEL_VARIANTS_X - 1, 0),
        );
        assert_eq!(shifted.bounds.origin, undilated.bounds.origin);
        assert_eq!(shifted.size.height, undilated.size.height);
        assert_eq!(shifted.size.width.0, undilated.size.width.0 + 1);

        let mask = render(
            letter,
            GlyphRenderMode::Grayscale,
            rgba(0x303030ff),
            point(3, 0),
        );
        assert_eq!(mask.format, RasterizedGlyphFormat::AlphaMask);
        assert_eq!(mask.bounds.size, mask.size);
        assert!(mask.bounds.origin.y.0 < 0);
        assert!(mask.size.width.0 > 0 && mask.size.height.0 > 0);
        mask.validate().unwrap();

        let color = render(
            letter,
            GlyphRenderMode::Color,
            rgba(0xe02010ff),
            point(1, 0),
        );
        assert_eq!(color.format, RasterizedGlyphFormat::BgraColor);
        color.validate().unwrap();
        let colored_pixel = color
            .pixels
            .chunks_exact(4)
            .find(|pixel| pixel[3] > 128)
            .expect("colored glyph pixel");
        assert!(colored_pixel[2] > colored_pixel[0], "{colored_pixel:?}");

        let space = system.glyph_for_char(font_id, ' ').unwrap();
        let empty = render(
            space,
            GlyphRenderMode::Grayscale,
            rgba(0x000000ff),
            point(0, 0),
        );
        assert_eq!(empty.size, gpui::Size::default());
        assert!(empty.pixels.is_empty());

        let emoji_system = ParleyTextSystem::new_with_rasterizer(
            SystemFonts::Load,
            ".AppleSystemUIFont",
            MacGlyphRasterizer::new(),
        );
        let emoji_font = emoji_system
            .font_id(&gpui_font("Apple Color Emoji"))
            .expect("Apple Color Emoji is available on macOS");
        let emoji = emoji_system
            .rasterize_glyph(&RenderGlyphParams {
                font_id: emoji_font,
                glyph_id: emoji_system.glyph_for_char(emoji_font, '😀').unwrap(),
                font_size: px(24.0),
                subpixel_variant: point(2, 0),
                scale_factor: 2.0,
                raster_style: emoji_system.prepare_raster_style(RasterStyleRequest {
                    scene_color: rgba(0xffffffff),
                    requested_mode: GlyphRenderMode::Color,
                }),
            })
            .unwrap();
        assert_eq!(emoji.format, RasterizedGlyphFormat::BgraColor);
        emoji.validate().unwrap();
        assert!(emoji.pixels.chunks_exact(4).any(|pixel| {
            pixel[3] > 128
                && (pixel[0].abs_diff(pixel[1]) > 20
                    || pixel[1].abs_diff(pixel[2]) > 20
                    || pixel[0].abs_diff(pixel[2]) > 20)
        }));
    }
}
