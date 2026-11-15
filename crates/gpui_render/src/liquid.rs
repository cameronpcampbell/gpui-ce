//! Packed liquid uploads shared by the native and downlevel renderers.

use crate::shaders::{
    interface::{bytes_of, slice_as_bytes},
    liquid::{LIQUID_RECORD_CELLS, LiquidCell},
};

use geometry::PreparedLiquidGeometry;
use gpui::{Corners, Edges, LiquidGpuRecord, Quad, ScaledPixels, Scene, Size};
use wgsl_rs::std::{Vec4f, vec2f, vec4u};

mod geometry;

#[cfg(any(test, feature = "test-support"))]
pub mod testing;

/// A retained whole-scene table. Source headers precede packed contour segments.
#[derive(Default)]
pub struct LiquidTable {
    /// Numeric cells uploaded at binding zero. All indices are table-relative.
    pub cells: Vec<LiquidCell>,
    fallback: Vec<bool>,
    prepared: Vec<Option<CachedSource>>,
}

#[derive(PartialEq)]
struct GeometryKey {
    size: Size<ScaledPixels>,
    radii: Corners<ScaledPixels>,
    widths: Edges<ScaledPixels>,
    smoothing: f32,
}

struct CachedSource {
    key: GeometryKey,
    geometry: PreparedLiquidGeometry,
}

impl LiquidTable {
    /// Compile complete components, falling back rather than truncating neighbors.
    pub fn prepare(&mut self, scene: &Scene, maximum_bytes: usize) {
        self.cells.clear();
        self.fallback.clear();
        self.fallback.resize(scene.liquid_records.len(), false);

        self.prepared
            .resize_with(scene.liquid_records.len(), || None);

        let maximum_cells = (maximum_bytes / 16).min(u32::MAX as usize);

        let header_cells = scene
            .liquid_records
            .len()
            .saturating_mul(LIQUID_RECORD_CELLS as usize);

        if header_cells > maximum_cells {
            self.fallback.fill(true);
            log::error!(
                "Liquid source table exceeds the device limit. Using ordinary decorations."
            );

            return;
        }

        if scene
            .liquid_components
            .iter()
            .all(|component| component.fallback)
        {
            self.fallback.fill(true);

            return;
        }

        self.cells.resize(
            header_cells,
            LiquidCell {
                words: vec4u(0, 0, 0, 0),
            },
        );

        for component in &scene.liquid_components {
            if component.fallback {
                self.fallback[component.records.clone()].fill(true);

                continue;
            }

            let component_start = self.cells.len();
            let mut valid = true;

            for record_idx in component.records.clone() {
                let record = &scene.liquid_records[record_idx];

                if record.source.reach.0 <= 0.0
                    || !contour_preparation_fits(&record.source.quad, maximum_bytes)
                {
                    valid = false;

                    break;
                }

                let quad = &record.source.quad;

                let key = GeometryKey {
                    size: quad.bounds.size,
                    radii: quad.corner_radii,
                    widths: quad.border_widths,
                    smoothing: quad.corner_smoothing,
                };

                if self.prepared[record_idx]
                    .as_ref()
                    .is_none_or(|cached| cached.key != key)
                {
                    self.prepared[record_idx] = Some(CachedSource {
                        key,
                        geometry: PreparedLiquidGeometry::new(quad),
                    });
                }

                let prepared = &self.prepared[record_idx].as_ref().unwrap().geometry;
                let inner_start = self.cells.len();
                let inner_count = prepared.inset_contour.len();
                let outer_start = inner_start.saturating_add(inner_count);
                let outer_count = prepared.outer_contour.len();
                let end = outer_start.saturating_add(outer_count);

                if end > maximum_cells {
                    valid = false;

                    break;
                }

                let translation = vec2f(quad.bounds.origin.x.0, quad.bounds.origin.y.0);

                for segment in prepared.inset_contour.iter().chain(&prepared.outer_contour) {
                    let start = segment.start + translation;
                    let end = segment.end + translation;

                    self.cells.push(LiquidCell {
                        words: vec4u(
                            start.x.to_bits(),
                            start.y.to_bits(),
                            end.x.to_bits(),
                            end.y.to_bits(),
                        ),
                    });
                }

                let corners = prepared.corners;
                let vector_array = |value: Vec4f| [value.x, value.y, value.z, value.w];

                let header = LiquidGpuRecord {
                    quad: record.source.quad,
                    horizontal_reaches: vector_array(corners.horizontal_reaches),
                    vertical_reaches: vector_array(corners.vertical_reaches),
                    smoothing_factors: vector_array(corners.smoothing_factors),
                    superellipse_power: corners.superellipse_power,
                    component_start: component.records.start as u32,
                    component_count: component.records.len() as u32,
                    reach: record.source.reach.0,
                    radius: record.source.blend_radius.0,
                    inner_start: inner_start as u32,
                    inner_count: if prepared.direct_inset == 0 {
                        inner_count as u32
                    } else {
                        prepared.direct_inset
                    },
                    outer_start: outer_start as u32,
                    outer_count: outer_count as u32,
                    fill_power: prepared.fill_power,
                };

                let bytes = bytes_of(&header);
                assert_eq!(bytes.len(), LIQUID_RECORD_CELLS as usize * 16);

                for (cell_idx, chunk) in bytes.chunks_exact(16).enumerate() {
                    let word = |word_idx: usize| {
                        u32::from_ne_bytes(
                            chunk[word_idx * 4..word_idx * 4 + 4].try_into().unwrap(),
                        )
                    };

                    self.cells[record_idx * LIQUID_RECORD_CELLS as usize + cell_idx] = LiquidCell {
                        words: vec4u(word(0), word(1), word(2), word(3)),
                    };
                }
            }

            if !valid {
                self.cells.truncate(component_start);
                self.fallback[component.records.clone()].fill(true);
                log::error!(
                    "Liquid component exceeds the device or contour preparation limit. Using ordinary decorations."
                );
            }
        }

        if self.fallback.iter().all(|fallback| *fallback) {
            self.cells.clear();
        }
    }

    /// Whether a source's entire component uses ordinary decoration.
    pub fn falls_back(&self, source_idx: usize) -> bool {
        self.fallback[source_idx]
    }

    /// Bytes reserved and uploaded for this frame.
    pub fn byte_len(&self) -> usize {
        slice_as_bytes(&self.cells).len()
    }
}

fn contour_preparation_fits(quad: &Quad, maximum_bytes: usize) -> bool {
    let radii = quad.corner_radii;
    let has_radii = [
        radii.top_left,
        radii.top_right,
        radii.bottom_right,
        radii.bottom_left,
    ]
    .iter()
    .any(|radius| radius.0 > 0.0);

    let needs_contour =
        has_radii && (quad.corner_smoothing > 0.0 || quad.border_widths.any(|width| width.0 > 0.0));

    if !needs_contour {
        return true;
    }

    // Bound contour preparation before float-to-integer conversion or allocation.
    let columns = (f64::from(quad.bounds.size.width.0) * 8.0).ceil() + 3.0;
    let rows = (f64::from(quad.bounds.size.height.0) * 8.0).ceil() + 3.0;

    columns * rows <= (maximum_bytes / std::mem::size_of::<f32>()) as f64
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::liquid::testing::LIQUID_TEST_LOCK;
    use crate::shaders::{
        common::{Paint, paint_color, prepare_paint},
        liquid::{LIQUID_CELLS, liquid_distances, liquid_material, liquid_paint, liquid_source},
    };
    use gpui::{Bounds, ContentMask, LiquidPhase, ScaledLiquidBox, point, size};
    use wgsl_rs::std::RuntimeArray;

    fn quad(source_idx: usize) -> Quad {
        Quad {
            bounds: Bounds::new(
                point(ScaledPixels(source_idx as f32 * 2.0), ScaledPixels(10.0)),
                size(ScaledPixels(24.0), ScaledPixels(20.0)),
            ),
            content_mask: ContentMask {
                bounds: Bounds::new(
                    point(ScaledPixels(-100.0), ScaledPixels(-100.0)),
                    size(ScaledPixels(2000.0), ScaledPixels(2000.0)),
                ),
                ..Default::default()
            },
            background: gpui::rgb(0x5599ff).into(),
            border_color: gpui::rgba(0xff000080).into(),
            border_widths: Edges {
                right: ScaledPixels(if source_idx == 0 { 3.0 } else { 2.0 }),
                ..Edges::all(ScaledPixels(2.0))
            },
            corner_radii: Corners::all(ScaledPixels(5.0)),
            corner_smoothing: if source_idx.is_multiple_of(2) {
                0.6
            } else {
                0.0
            },
            ..Default::default()
        }
    }

    fn scene(quads: impl IntoIterator<Item = Quad>) -> Scene {
        let mut scene = Scene::default();

        for (source_idx, quad) in quads.into_iter().enumerate() {
            let member = scene
                .register_liquid_box(ScaledLiquidBox {
                    quad,
                    group: None,
                    blend_radius: ScaledPixels(source_idx as f32),
                    reach: ScaledPixels(24.0),
                })
                .unwrap();

            scene.paint_liquid_phase(member, LiquidPhase::Fill);
            scene.paint_liquid_phase(member, LiquidPhase::Border);
        }

        scene.finish();

        scene
    }

    #[test]
    fn packed_headers_paints_and_complete_component_limits() {
        let _guard = LIQUID_TEST_LOCK.lock().unwrap();
        let mut scene = scene((0..40).map(quad));
        let mut table = LiquidTable::default();
        table.prepare(&scene, 16 * 1024 * 1024);
        assert_eq!(scene.liquid_components.len(), 1);
        assert!(!table.falls_back(39));

        LIQUID_CELLS.set(RuntimeArray {
            data: table.cells.clone(),
        });

        for source_idx in [0, 1, 39] {
            let decoded = liquid_source(source_idx);
            let source = scene.liquid_records[source_idx as usize].source.quad;
            assert_eq!(decoded.component_start, 0);
            assert_eq!(decoded.component_count, 40);
            assert_eq!(decoded.radius, source_idx as f32);
            assert_eq!(decoded.reach, 24.0);
            assert_eq!(decoded.quad.bounds.origin.x, source.bounds.origin.x.0);
            assert_eq!(decoded.quad.bounds.origin.y, source.bounds.origin.y.0);

            for position in [vec2f(-2.0, 8.0), vec2f(20.0, 16.0), vec2f(90.0, 35.0)] {
                for border in [false, true] {
                    let background = if border {
                        source.border_color
                    } else {
                        source.background
                    };

                    let paint = Paint::new(background.into(), source.bounds.into());

                    let expected =
                        liquid_material(paint_color(paint, position, prepare_paint(paint)));

                    assert_eq!(liquid_paint(decoded, position, border), expected);
                }
            }
        }

        let capacity = table.cells.capacity();
        table.prepare(&scene, 16 * 1024 * 1024);
        assert_eq!(table.cells.capacity(), capacity);
        assert_eq!(table.byte_len(), table.cells.len() * 16);

        for maximum in [
            scene.liquid_records.len() * LIQUID_RECORD_CELLS as usize * 16,
            0,
        ] {
            table.prepare(&scene, maximum);
            assert!((0..40).all(|source_idx| table.falls_back(source_idx)));
            assert!(table.cells.is_empty());
        }

        scene.liquid_records[0].source.quad.bounds.size.width = ScaledPixels(f32::MAX);
        table.prepare(&scene, 16 * 1024 * 1024);
        assert!(table.cells.is_empty());
        assert!((0..40).all(|source_idx| table.falls_back(source_idx)));
        table.prepare(&Scene::default(), 0);
        assert!(table.cells.is_empty());
    }

    #[test]
    fn movement_and_paint_changes_reuse_geometry_and_upload_current_values() {
        let _guard = LIQUID_TEST_LOCK.lock().unwrap();
        let mut scene = scene([quad(0)]);
        let mut table = LiquidTable::default();
        table.prepare(&scene, 16 * 1024 * 1024);

        assert!(
            !table.prepared[0]
                .as_ref()
                .unwrap()
                .geometry
                .inset_contour
                .is_empty()
        );

        let contour = table.prepared[0]
            .as_ref()
            .unwrap()
            .geometry
            .inset_contour
            .as_ptr();

        let positions = [vec2f(12.0, 5.0), vec2f(20.0, 15.0), vec2f(30.0, 25.0)];

        LIQUID_CELLS.set(RuntimeArray {
            data: table.cells.clone(),
        });

        let distances = positions.map(|position| liquid_distances(liquid_source(0), position));
        let source = &mut scene.liquid_records[0].source;
        source.quad.bounds.origin.x += ScaledPixels(13.25);
        source.quad.bounds.origin.y += ScaledPixels(-2.5);
        source.quad.background = gpui::rgb(0x44ff88).into();
        source.quad.border_color = gpui::rgb(0xff8800).into();
        source.blend_radius = ScaledPixels(32.0);
        table.prepare(&scene, 16 * 1024 * 1024);

        assert_eq!(
            table.prepared[0]
                .as_ref()
                .unwrap()
                .geometry
                .inset_contour
                .as_ptr(),
            contour
        );

        LIQUID_CELLS.set(RuntimeArray { data: table.cells });
        let decoded = liquid_source(0);
        assert_eq!(decoded.radius, 32.0);
        assert_eq!(decoded.quad.bounds.origin, vec2f(13.25, 7.5));

        for border in [false, true] {
            let source = scene.liquid_records[0].source.quad;

            let background = if border {
                source.border_color
            } else {
                source.background
            };

            let paint = Paint::new(background.into(), source.bounds.into());
            let position = vec2f(25.0, 15.0);
            let expected = liquid_material(paint_color(paint, position, prepare_paint(paint)));
            assert_eq!(liquid_paint(decoded, position, border), expected);
        }

        for (position, expected) in positions.into_iter().zip(distances) {
            let actual = liquid_distances(decoded, position + vec2f(13.25, -2.5));
            assert!((actual.x - expected.x).abs() < 0.0001);
            assert!((actual.y - expected.y).abs() < 0.0001);
        }
    }

    #[test]
    fn example_sized_sources_need_only_headers() {
        for scale in [1.0, 2.0] {
            for smoothing in [0.0, 0.6, 1.0] {
                let scene = scene(
                    [
                        (180.0, 55.0, 155.0, 125.0, 1.0),
                        (344.0, 65.0, 155.0, 125.0, 8.0),
                        (24.0, 350.0, 350.0, 130.0, 4.0),
                        (234.0, 380.0, 180.0, 80.0, 8.0),
                    ]
                    .map(|(x, y, width, height, border)| Quad {
                        bounds: Bounds::new(
                            point(ScaledPixels(x * scale), ScaledPixels(y * scale)),
                            size(ScaledPixels(width * scale), ScaledPixels(height * scale)),
                        ),
                        border_widths: Edges::all(ScaledPixels(border * scale)),
                        corner_radii: Corners::all(ScaledPixels(32.0 * scale)),
                        corner_smoothing: smoothing,
                        ..quad(0)
                    }),
                );

                let mut table = LiquidTable::default();
                table.prepare(&scene, 128 * 1024 * 1024);
                assert_eq!(scene.liquid_components.len(), 2);
                assert_eq!(table.cells.len(), 4 * LIQUID_RECORD_CELLS as usize);
                assert!((0..4).all(|source_idx| !table.falls_back(source_idx)));
            }
        }
    }
}
