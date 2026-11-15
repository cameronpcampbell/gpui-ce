//! CPU execution of the packed liquid shader for tests and review images.

use crate::liquid::LiquidTable;
use crate::shaders::{common::*, liquid::*};
use gpui::{LiquidPhase, LiquidRecord, Scene};
use std::sync::{Mutex, MutexGuard};
use wgsl_rs::std::{RuntimeArray, Vec2f, Vec4f, abs, vec2f};

/// Serialize CPU shader bindings across table, field, and image tests.
pub static LIQUID_TEST_LOCK: Mutex<()> = Mutex::new(());

/// A scene uploaded once to the CPU shader bindings, retained for all samples.
pub struct PreparedLiquidComponent<'scene> {
    scene: &'scene Scene,
    _lock: MutexGuard<'static, ()>,
}

impl<'scene> PreparedLiquidComponent<'scene> {
    pub fn new(scene: &'scene Scene) -> Self {
        let lock = LIQUID_TEST_LOCK.lock().unwrap();
        let mut table = LiquidTable::default();
        table.prepare(scene, 256 * 1024 * 1024);
        assert!((0..scene.liquid_records.len()).all(|source_idx| !table.falls_back(source_idx)));
        LIQUID_CELLS.set(RuntimeArray { data: table.cells });

        Self { scene, _lock: lock }
    }

    pub fn fields(&self, source_idx: usize, position: Vec2f) -> Vec2f {
        if self.scene.liquid_records.is_empty() {
            return vec2f(0.0, 0.0);
        }

        let emitter = liquid_source(source_idx as u32);

        sample_liquid_fields(emitter, source_idx as u32, position).fields
    }

    /// Use centered device-pixel differences outside a fragment dispatch.
    pub fn sample(&self, source_idx: usize, position: Vec2f) -> LiquidSample {
        if self.scene.liquid_records.is_empty() {
            return LiquidSample::default();
        }

        let emitter = liquid_source(source_idx as u32);
        let sample = sample_liquid_fields(emitter, source_idx as u32, position);

        let horizontal = self.fields(source_idx, position + vec2f(0.5, 0.0))
            - self.fields(source_idx, position - vec2f(0.5, 0.0));

        let vertical = self.fields(source_idx, position + vec2f(0.0, 0.5))
            - self.fields(source_idx, position - vec2f(0.0, 0.5));

        let derivative = abs(horizontal) + abs(vertical);

        if sample.active_count == 0 {
            return LiquidSample::default();
        }

        let owner = liquid_source(sample.owner);

        LiquidSample {
            owner: Some(self.scene.liquid_records[sample.owner as usize].ordinal),
            fill: liquid_component_material(owner, position, sample, false),
            border: liquid_component_material(owner, position, sample, true),
            coverage: liquid_coverage(owner, position, sample, derivative),
            fields: sample.fields,
            blend_radius: sample.radius,
            active_count: sample.active_count,
        }
    }
}

/// Materials are linear premultiplied RGBA, independent of target format.
#[derive(Clone, Copy, Default)]
pub struct LiquidSample {
    pub owner: Option<u64>,
    pub fill: Vec4f,
    pub border: Vec4f,
    pub coverage: Vec2f,
    pub fields: Vec2f,
    pub blend_radius: f32,
    pub active_count: u32,
}

impl LiquidSample {
    pub fn phase(&self, record: &LiquidRecord, position: Vec2f, phase: LiquidPhase) -> Vec4f {
        if self.owner != Some(record.ordinal) {
            return transparent();
        }

        let mask: ContentMask = record.source.quad.content_mask.into();

        if is_clipped(clip_distances(position, mask.bounds)) {
            return transparent();
        }

        let (material, coverage) = match phase {
            LiquidPhase::Fill => (self.fill, self.coverage.x),
            LiquidPhase::Border => (self.border, self.coverage.y),
        };

        material * (coverage * ContentMask::alpha(mask, position))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shaders::quad::ordinary_quad_coverage;
    use gpui::{
        Bounds as SceneBounds, ContentMask as SceneMask, Corners as SceneCorners,
        Edges as SceneEdges, Quad, ScaledLiquidBox, ScaledPixels, point, rgba, size,
    };

    fn source(x: f32, widths: [f32; 4], smoothing: f32, radius: f32) -> ScaledLiquidBox {
        ScaledLiquidBox {
            quad: Quad {
                bounds: SceneBounds::new(
                    point(ScaledPixels(x), ScaledPixels(0.0)),
                    size(ScaledPixels(40.0), ScaledPixels(60.0)),
                ),
                content_mask: SceneMask {
                    bounds: SceneBounds::new(
                        point(ScaledPixels(-100.0), ScaledPixels(-100.0)),
                        size(ScaledPixels(400.0), ScaledPixels(300.0)),
                    ),
                    ..Default::default()
                },
                background: rgba(0x4488ff80).into(),
                border_color: rgba(0xff884480).into(),
                corner_radii: SceneCorners::all(ScaledPixels(12.0)),
                border_widths: SceneEdges {
                    top: ScaledPixels(widths[0]),
                    right: ScaledPixels(widths[1]),
                    bottom: ScaledPixels(widths[2]),
                    left: ScaledPixels(widths[3]),
                },
                corner_smoothing: smoothing,
                ..Default::default()
            },
            group: None,
            blend_radius: ScaledPixels(radius),
            reach: ScaledPixels(24.0),
        }
    }

    fn make_scene(sources: &[ScaledLiquidBox]) -> Scene {
        let mut scene = Scene::default();

        for source in sources {
            let member = scene.register_liquid_box(source.clone()).unwrap();
            scene.paint_liquid_phase(member, LiquidPhase::Fill);
            scene.paint_liquid_phase(member, LiquidPhase::Border);
        }

        scene.finish();

        scene
    }

    fn assert_close(actual: f32, expected: f32) {
        assert!(
            (actual - expected).abs() < 0.0001,
            "{actual} versus {expected}"
        );
    }

    #[test]
    fn compact_field_preserves_single_source_and_bridge_threshold() {
        for distance in [-100.0, -24.0, -3.0, 0.0, 4.0, 24.0, 100.0] {
            let potential = liquid_potential(distance, 24.0);
            assert!((0.0..=4.0).contains(&potential));

            if (-24.0..24.0).contains(&distance) {
                assert_close(24.0 * (1.0 - potential.sqrt()), distance);
            }
        }

        assert_eq!(field_coverage_with_width(0.0, 100.0), 0.0);
        let gap = 48.0 * (1.0 - 1.0 / 2.0_f32.sqrt());
        assert_close(2.0 * liquid_potential(gap / 2.0, 24.0), 1.0);
        assert!(2.0 * liquid_potential((gap - 0.1) / 2.0, 24.0) > 1.0);
        assert!(2.0 * liquid_potential((gap + 0.1) / 2.0, 24.0) < 1.0);
    }

    #[test]
    fn nested_sources_keep_local_fill_and_submerge_borders() {
        for radius in [0.0, 8.0] {
            let mut parent = source(0.0, [4.0; 4], 0.6, radius);
            parent.quad.bounds.size = size(ScaledPixels(120.0), ScaledPixels(100.0));
            let mut child = source(35.0, [3.0; 4], 0.6, radius);
            child.quad.bounds.origin.y = ScaledPixels(30.0);
            child.quad.bounds.size = size(ScaledPixels(40.0), ScaledPixels(40.0));
            child.quad.background = rgba(0xff448880).into();
            let scene = make_scene(&[parent, child]);
            let prepared = PreparedLiquidComponent::new(&scene);

            for position in [vec2f(55.0, 50.0), vec2f(35.0, 50.0)] {
                let sample = prepared.sample(0, position);
                assert_eq!(sample.owner, Some(scene.liquid_records[1].ordinal));
                assert_eq!(sample.coverage.y, 0.0);
                assert_close(sample.fill.w, 128.0 / 255.0);

                if position.x == 55.0 {
                    assert_eq!(sample.fill, liquid_paint(liquid_source(1), position, false));
                }
            }
        }
    }

    #[test]
    fn shell_materials_keep_transparent_contributors_and_borderless_owners() {
        let mut bordered = source(0.0, [8.0; 4], 0.0, 0.0);
        bordered.quad.corner_radii = Default::default();
        let mut borderless = source(50.0, [0.0; 4], 0.0, 0.0);
        borderless.quad.corner_radii = Default::default();
        borderless.quad.border_color = rgba(0).into();
        let scene = make_scene(&[bordered.clone(), borderless]);
        let prepared = PreparedLiquidComponent::new(&scene);
        let position = vec2f(46.0, 2.0);
        let sample = prepared.sample(0, position);
        assert_eq!(sample.owner, Some(scene.liquid_records[1].ordinal));

        assert_eq!(
            sample.border,
            liquid_paint(liquid_source(0), position, true)
        );

        drop(prepared);

        let mut transparent = bordered.clone();
        bordered.blend_radius = ScaledPixels(8.0);
        transparent.blend_radius = ScaledPixels(8.0);
        transparent.quad.border_color = rgba(0).into();
        let scene = make_scene(&[bordered, transparent]);
        let prepared = PreparedLiquidComponent::new(&scene);
        assert_close(prepared.sample(0, vec2f(20.0, 1.0)).border.w, 64.0 / 255.0);
    }

    #[test]
    fn material_weights_stay_finite_for_mixed_and_tiny_radii() {
        for radius in [
            0.0,
            f32::from_bits(1),
            f32::MIN_POSITIVE,
            0.00001,
            8.0,
            10000.0,
            f32::MAX,
        ] {
            let scene = make_scene(&[
                source(0.0, [1.0; 4], 0.0, radius),
                source(50.0, [8.0; 4], 0.0, radius),
            ]);

            let prepared = PreparedLiquidComponent::new(&scene);

            for x in [0.0, 20.0, 40.0, 44.99, 45.0, 45.01, 50.0, 70.0, 90.0] {
                let sample = prepared.sample(0, vec2f(x, 1.0));
                assert!(sample.blend_radius.is_finite());

                assert!(
                    (0usize..4)
                        .all(|idx| sample.fill[idx].is_finite() && sample.border[idx].is_finite())
                );

                assert_close(sample.fill.w, 128.0 / 255.0);
                assert_close(sample.border.w, 128.0 / 255.0);
            }
        }
    }

    #[test]
    fn empty_consumed_and_masked_components_emit_correctly() {
        let empty_scene = Scene::default();
        let empty = PreparedLiquidComponent::new(&empty_scene);
        assert_eq!(empty.sample(0, vec2f(0.0, 0.0)).owner, None);
        drop(empty);

        let mut source = source(0.0, [21.0; 4], 0.0, 8.0);
        source.quad.bounds.size = size(ScaledPixels(20.0), ScaledPixels(20.0));
        source.quad.content_mask.bounds = source.quad.bounds;
        source.quad.content_mask.fade_out.left = ScaledPixels(20.0);
        let scene = make_scene(&[source]);
        let prepared = PreparedLiquidComponent::new(&scene);
        let sample = prepared.sample(0, vec2f(10.0, 10.0));
        let record = &scene.liquid_records[0];
        assert_eq!(sample.coverage.y, 1.0);

        assert_close(
            sample.phase(record, vec2f(10.0, 10.0), LiquidPhase::Fill).w,
            64.0 / 255.0,
        );

        assert_eq!(
            sample.phase(record, vec2f(-1.0, 10.0), LiquidPhase::Fill),
            transparent()
        );
    }

    #[test]
    fn singleton_keeps_ordinary_coverage_for_thin_and_asymmetric_borders() {
        for smoothing in [0.0, 0.6] {
            for widths in [[0.0; 4], [0.3; 4], [1.0, 8.0, 4.0, 0.0]] {
                let scene = make_scene(&[source(10.0, widths, smoothing, 8.0)]);
                let prepared = PreparedLiquidComponent::new(&scene);
                let emitter = liquid_source(0);

                for y in (-5..66).step_by(3) {
                    for x in (5..56).step_by(3) {
                        let position = vec2f(x as f32 + 0.25, y as f32 + 0.25);
                        let sample = prepared.sample(0, position);

                        let expected =
                            ordinary_quad_coverage(emitter.quad, position, liquid_corners(emitter));

                        assert_eq!(sample.coverage, expected);
                    }
                }
            }
        }
    }

    #[test]
    fn border_contours_change_continuously_at_zero_material_radius() {
        let mut previous: Option<f32> = None;

        for idx in 0..29 {
            let width = 1.0 + idx as f32 * 0.25;

            let mut sources = [
                source(0.0, [1.0; 4], 0.0, 0.0),
                source(44.0, [width; 4], 0.0, 0.0),
            ];

            for source in &mut sources {
                source.quad.corner_radii = Default::default();
            }

            let scene = make_scene(&sources);
            let prepared = PreparedLiquidComponent::new(&scene);

            let crossing = |inner: bool| {
                let mut lower = -15.0;
                let mut upper = 20.0;

                for _iteration in 0..24 {
                    let middle = (lower + upper) / 2.0;
                    let field = prepared.fields(0, vec2f(42.0, middle));
                    let value = if inner { field.y } else { field.x };

                    if value < 1.0 {
                        lower = middle;
                    } else {
                        upper = middle;
                    }
                }

                (lower + upper) / 2.0
            };

            let thickness = crossing(true) - crossing(false);
            assert!(thickness >= 0.0);

            if let Some(previous) = previous {
                assert!(
                    (thickness - previous).abs() < 0.5,
                    "width {width}, thickness {thickness}, previous {previous}"
                );
            }

            previous = Some(thickness);
        }
    }

    #[test]
    fn geometry_is_order_independent_and_owners_stay_in_raster_support() {
        let mut third = source(28.0, [3.0; 4], 0.6, 8.0);
        third.quad.bounds.origin.y = ScaledPixels(20.0);
        third.quad.bounds.size = size(ScaledPixels(30.0), ScaledPixels(20.0));
        let sources = [
            source(0.0, [1.0; 4], 0.0, 8.0),
            source(50.0, [8.0; 4], 0.6, 8.0),
            third,
        ];
        let reversed = [sources[2].clone(), sources[1].clone(), sources[0].clone()];
        let mut original = Vec::new();

        for (order_idx, sources) in [sources, reversed].iter().enumerate() {
            let scene = make_scene(sources);
            let prepared = PreparedLiquidComponent::new(&scene);
            let mut sample_idx = 0;

            for y in (-28..112).step_by(7) {
                for x in (-28..126).step_by(7) {
                    let position = vec2f(x as f32, y as f32);
                    let sample = prepared.sample(0, position);

                    if order_idx == 0 {
                        original.push(sample.fields);
                    } else {
                        let previous: Vec2f = original[sample_idx];
                        assert_close(sample.fields.x, previous.x);
                        assert_close(sample.fields.y, previous.y);
                    }

                    if sample.coverage.x > 0.0 {
                        let owner_idx = scene
                            .liquid_records
                            .iter()
                            .position(|record| Some(record.ordinal) == sample.owner)
                            .unwrap();
                        let owner = &scene.liquid_records[owner_idx].source;
                        let delta = position
                            - vec2f(owner.quad.bounds.origin.x.0, owner.quad.bounds.origin.y.0);
                        let padding = owner.reach.0 + 1.0;
                        assert!(delta.x >= -padding && delta.y >= -padding);
                        assert!(
                            delta.x <= owner.quad.bounds.size.width.0 + padding
                                && delta.y <= owner.quad.bounds.size.height.0 + padding
                        );
                        assert!(
                            liquid_distances(liquid_source(owner_idx as u32), position).x
                                < owner.reach.0
                        );
                    }

                    sample_idx += 1;
                }
            }
        }
    }
}
