use super::{BorderStyle, DrawOrder, PaintOperation, Quad, Scene};
use crate::{Bounds, ContentMask, Edges, ScaledPixels, SharedString};
use std::{
    collections::HashMap,
    ops::Range,
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT_SCENE_GENERATION: AtomicU64 = AtomicU64::new(1);

/// Semantic source data. Geometry stays separate from expanded raster coverage.
#[derive(Clone, Debug)]
#[expect(missing_docs)]
pub struct ScaledLiquidBox {
    pub quad: Quad,
    pub group: Option<SharedString>,
    pub blend_radius: ScaledPixels,
    pub reach: ScaledPixels,
}

/// A source token valid only in the scene generation that registered it.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct LiquidMemberId {
    generation: u64,
    idx: usize,
}

/// The two positions occupied by a liquid box in the paint stream.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LiquidPhase {
    /// Paint before inset shadows and children.
    Fill,
    /// Paint after children.
    Border,
}

/// One source in component order, independent of descriptor sorting.
#[derive(Clone, Debug)]
#[expect(missing_docs)]
pub struct LiquidRecord {
    pub source: ScaledLiquidBox,
    pub ordinal: u64,
}

/// Numeric source header in the packed renderer table. Contours follow all headers.
#[repr(C)]
#[derive(Clone, Copy)]
#[expect(missing_docs)]
pub struct LiquidGpuRecord {
    pub quad: Quad,
    pub horizontal_reaches: [f32; 4],
    pub vertical_reaches: [f32; 4],
    pub smoothing_factors: [f32; 4],
    pub superellipse_power: f32,
    pub component_start: u32,
    pub component_count: u32,
    pub reach: f32,
    pub radius: f32,
    pub inner_start: u32,
    pub inner_count: u32,
    pub outer_start: u32,
    pub outer_count: u32,
    pub fill_power: f32,
}

/// A connected set of compatible sources.
#[derive(Clone, Debug)]
#[expect(missing_docs)]
pub struct LiquidComponent {
    pub records: Range<usize>,
    /// Dashed contours retain ordinary per-box rendering.
    pub fallback: bool,
}

/// A phase descriptor in draw order. Record indices address component-ordered storage.
#[derive(Clone, Debug)]
#[expect(missing_docs)]
pub struct LiquidDraw {
    pub order: DrawOrder,
    pub bounds: Bounds<ScaledPixels>,
    /// Unclipped expanded coverage, separate from geometry and ordering bounds.
    pub raster_bounds: Bounds<ScaledPixels>,
    pub content_mask: ContentMask<ScaledPixels>,
    pub member: usize,
    pub record_index: usize,
    pub component_index: usize,
    pub phase: LiquidPhase,
    /// Ordinary decoration retained for complete-component fallback.
    pub quad: Quad,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct LiquidDomainKey {
    group: Option<SharedString>,
    isolation: u64,
    clip: usize,
    overlay_epoch: u64,
}

#[derive(Clone)]
struct LiquidMember {
    source: ScaledLiquidBox,
    domain: LiquidDomainKey,
    coverage: Bounds<ScaledPixels>,
    visible: bool,
}

pub(super) struct LiquidState {
    generation: u64,
    members: Vec<LiquidMember>,
    masks: Vec<ContentMask<ScaledPixels>>,
    isolation_stack: Vec<u64>,
    visibility_stack: Vec<bool>,
    next_isolation: u64,
    overlay_epoch: u64,
    replay_members: HashMap<LiquidMemberId, LiquidMemberId>,
    pub(super) normal_ordering: bool,
}

impl Default for LiquidState {
    fn default() -> Self {
        Self {
            generation: NEXT_SCENE_GENERATION.fetch_add(1, Ordering::Relaxed),
            members: Vec::new(),
            masks: Vec::new(),
            isolation_stack: Vec::new(),
            visibility_stack: Vec::new(),
            next_isolation: 0,
            overlay_epoch: 0,
            replay_members: HashMap::new(),
            normal_ordering: false,
        }
    }
}

impl LiquidState {
    pub(super) fn clear(&mut self) {
        self.generation = NEXT_SCENE_GENERATION.fetch_add(1, Ordering::Relaxed);
        self.members.clear();
        self.masks.clear();
        self.isolation_stack.clear();
        self.visibility_stack.clear();
        self.next_isolation = 0;
        self.overlay_epoch = 0;
        self.replay_members.clear();
        self.normal_ordering = false;
    }

    pub(super) fn filter_boundary(&mut self, is_start: bool, opacity: f32) {
        if is_start {
            self.next_isolation += 1;
            self.isolation_stack.push(self.next_isolation);
            let visible = self.visibility_stack.last().copied().unwrap_or(true) && opacity > 0.0;
            self.visibility_stack.push(visible);
        } else {
            self.isolation_stack.pop();
            self.visibility_stack.pop();
        }
    }

    pub(super) fn advance_epoch(&mut self) {
        self.overlay_epoch += 1;
    }
}

fn positive(value: ScaledPixels) -> ScaledPixels {
    if value.0.is_finite() && value.0 > 0.0 {
        return value;
    }

    ScaledPixels(0.0)
}

fn finite_bounds(bounds: &Bounds<ScaledPixels>) -> bool {
    [
        bounds.origin.x,
        bounds.origin.y,
        bounds.size.width,
        bounds.size.height,
        bounds.right(),
        bounds.bottom(),
    ]
    .iter()
    .all(|value| value.0.is_finite())
}

impl ScaledLiquidBox {
    fn sanitize(&mut self) -> bool {
        if !finite_bounds(&self.quad.bounds)
            || self.quad.bounds.is_empty()
            || !finite_bounds(&self.quad.content_mask.bounds)
        {
            return false;
        }

        self.blend_radius = positive(self.blend_radius);
        self.reach = positive(self.reach);
        self.quad.border_widths = self.quad.border_widths.map(|value| positive(*value));
        self.quad.corner_radii = self.quad.corner_radii.map(|value| positive(*value));
        self.quad.content_mask.fade_out = self
            .quad
            .content_mask
            .fade_out
            .map(|value| positive(*value));
        self.quad.corner_smoothing = if self.quad.corner_smoothing.is_finite() {
            self.quad.corner_smoothing.clamp(0.0, 1.0)
        } else {
            0.0
        };
        let visible_fill = !self.quad.background.is_transparent();
        let visible_border = !self.quad.border_color.is_transparent()
            && self.quad.border_widths.any(|width| width.0 > 0.0);

        finite_bounds(&self.raster_bounds()) && (visible_fill || visible_border)
    }

    fn raster_bounds(&self) -> Bounds<ScaledPixels> {
        self.quad.bounds.dilate(self.reach + ScaledPixels(1.0))
    }

    fn coverage(&self) -> Bounds<ScaledPixels> {
        self.raster_bounds()
            .intersect(&self.quad.content_mask.bounds)
    }
}

fn component_root(parents: &mut [usize], mut member_idx: usize) -> usize {
    while parents[member_idx] != member_idx {
        parents[member_idx] = parents[parents[member_idx]];
        member_idx = parents[member_idx];
    }

    member_idx
}

impl Scene {
    /// Advance the interaction epoch at a deferred overlay boundary without changing draw order.
    pub fn advance_liquid_overlay_epoch(&mut self) {
        self.is_finished = false;
        self.liquid.advance_epoch();
        self.paint_operations
            .push(PaintOperation::AdvanceLiquidOverlayEpoch);
    }

    /// Register one logical source. Empty and undecorated boxes have no liquid phases.
    /// Invisible targets retain semantic sources for cached replay and skip field compilation.
    pub fn register_liquid_box(&mut self, mut source: ScaledLiquidBox) -> Option<LiquidMemberId> {
        if !source.sanitize() || source.coverage().is_empty() {
            return None;
        }

        // Forced layer orders cannot account for expanded coverage. Replay prior ordinary
        // operations once, then use normal bounds ordering throughout this liquid scene.
        if !self.liquid.normal_ordering {
            self.use_normal_layer_ordering();
        }

        self.is_finished = false;
        let mask = source.quad.content_mask;
        let clip = match self
            .liquid
            .masks
            .iter()
            .position(|existing| *existing == mask)
        {
            Some(clip) => clip,
            None => {
                self.liquid.masks.push(mask);

                self.liquid.masks.len() - 1
            }
        };
        let member = LiquidMemberId {
            generation: self.liquid.generation,
            idx: self.liquid.members.len(),
        };
        let domain = LiquidDomainKey {
            group: source.group.clone(),
            isolation: self
                .liquid
                .isolation_stack
                .last()
                .copied()
                .unwrap_or_default(),
            clip,
            overlay_epoch: self.liquid.overlay_epoch,
        };
        let coverage = source.coverage();
        self.liquid.members.push(LiquidMember {
            source: source.clone(),
            domain,
            coverage,
            visible: self.liquid.visibility_stack.last().copied().unwrap_or(true),
        });
        self.paint_operations
            .push(PaintOperation::RegisterLiquid { member, source });

        Some(member)
    }

    /// Submit a phase using expanded coverage for spatial ordering.
    pub fn paint_liquid_phase(&mut self, member: LiquidMemberId, phase: LiquidPhase) {
        assert_eq!(
            member.generation, self.liquid.generation,
            "stale liquid member token"
        );
        let source = self.liquid.members[member.idx].source.clone();
        let coverage = self.liquid.members[member.idx].coverage;
        self.is_finished = false;
        let order = self.primitive_bounds.insert_above_all(coverage);
        self.primitive_bounds.set_order_floor(order + 1);
        let mut quad = source.quad;
        quad.order = order;

        match phase {
            LiquidPhase::Fill => {
                quad.border_widths = Edges::default();
                quad.border_color = quad.background.opacity(0.0);
            }

            LiquidPhase::Border => quad.background = quad.border_color.opacity(0.0),
        }

        let draw = LiquidDraw {
            order,
            bounds: coverage,
            raster_bounds: source.raster_bounds(),
            content_mask: source.quad.content_mask,
            member: member.idx,
            record_index: 0,
            component_index: 0,
            phase,
            quad,
        };

        if self.liquid.members[member.idx].visible {
            self.liquid_draws.push(draw);
        }

        // A phase retains semantic data so a cached range can start after registration.
        self.paint_operations.push(PaintOperation::LiquidPhase {
            member,
            source,
            phase,
        });
    }

    pub(super) fn replay_liquid_member(
        &mut self,
        original: LiquidMemberId,
        source: &ScaledLiquidBox,
    ) -> Option<LiquidMemberId> {
        if let Some(member) = self.liquid.replay_members.get(&original) {
            return Some(*member);
        }

        let member = self.register_liquid_box(source.clone())?;
        self.liquid.replay_members.insert(original, member);

        Some(member)
    }

    fn use_normal_layer_ordering(&mut self) {
        let operations = std::mem::take(&mut self.paint_operations);
        self.clear();
        self.liquid.normal_ordering = true;

        for operation in operations {
            self.replay_operation(&operation);
        }
    }

    pub(super) fn finish_liquid(&mut self) {
        self.liquid_records.clear();
        self.liquid_components.clear();
        let member_count = self.liquid.members.len();
        let mut active = vec![false; member_count];

        for draw in &self.liquid_draws {
            active[draw.member] = true;
        }

        let mut parents: Vec<_> = (0..member_count).collect();
        let mut domains: HashMap<&LiquidDomainKey, Vec<usize>> = HashMap::new();

        for (member_idx, member) in self.liquid.members.iter().enumerate() {
            if active[member_idx] {
                domains.entry(&member.domain).or_default().push(member_idx);
            }
        }

        for members in domains.values_mut() {
            members.sort_unstable_by_key(|member_idx| {
                self.liquid.members[*member_idx].coverage.origin.x
            });

            let mut candidates: Vec<usize> = Vec::new();

            for &member_idx in members.iter() {
                let coverage = self.liquid.members[member_idx].coverage;
                candidates.retain(|candidate_idx| {
                    self.liquid.members[*candidate_idx].coverage.right() > coverage.origin.x
                });

                for &candidate_idx in &candidates {
                    if coverage.intersects(&self.liquid.members[candidate_idx].coverage) {
                        let first_root = component_root(&mut parents, member_idx);
                        let second_root = component_root(&mut parents, candidate_idx);
                        parents[first_root.max(second_root)] = first_root.min(second_root);
                    }
                }

                candidates.push(member_idx);
            }
        }

        let mut connected = vec![Vec::new(); member_count];

        for member_idx in 0..member_count {
            if active[member_idx] {
                let root = component_root(&mut parents, member_idx);
                connected[root].push(member_idx);
            }
        }

        let mut record_indices = vec![0; member_count];
        let mut component_indices = vec![0; member_count];

        for component_members in connected {
            if component_members.is_empty() {
                continue;
            }

            let component_idx = self.liquid_components.len();
            let records_start = self.liquid_records.len();
            let mut fallback = false;

            for source_idx in component_members {
                let member = &self.liquid.members[source_idx];
                fallback |= member.source.quad.border_style == BorderStyle::Dashed;
                record_indices[source_idx] = self.liquid_records.len();
                component_indices[source_idx] = component_idx;
                self.liquid_records.push(LiquidRecord {
                    source: member.source.clone(),
                    ordinal: source_idx as u64,
                });
            }

            #[cfg(debug_assertions)]
            if fallback {
                log::debug!(
                    "Liquid component contains dashed borders; using ordinary box decoration."
                );
            }

            self.liquid_components.push(LiquidComponent {
                records: records_start..self.liquid_records.len(),
                fallback,
            });
        }

        for draw in &mut self.liquid_draws {
            draw.record_index = record_indices[draw.member];
            draw.component_index = component_indices[draw.member];
        }

        self.liquid_draws.sort_by_key(|draw| draw.order);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Corners, FilterBoundary, PrimitiveBatch, SharedString, point, rgb, size};

    fn bounds(x: f32, y: f32, width: f32, height: f32) -> Bounds<ScaledPixels> {
        Bounds::new(
            point(ScaledPixels(x), ScaledPixels(y)),
            size(ScaledPixels(width), ScaledPixels(height)),
        )
    }

    fn source(x: f32) -> ScaledLiquidBox {
        ScaledLiquidBox {
            quad: Quad {
                bounds: bounds(x, 0.0, 20.0, 20.0),
                content_mask: ContentMask {
                    bounds: bounds(-100.0, -100.0, 1000.0, 1000.0),
                    ..Default::default()
                },
                background: rgb(0xff0000).into(),
                ..Default::default()
            },
            group: None,
            blend_radius: ScaledPixels(8.0),
            reach: ScaledPixels(24.0),
        }
    }

    fn paint(scene: &mut Scene, source: ScaledLiquidBox) -> LiquidMemberId {
        let member = scene.register_liquid_box(source).unwrap();
        scene.paint_liquid_phase(member, LiquidPhase::Fill);
        scene.paint_liquid_phase(member, LiquidPhase::Border);

        member
    }

    fn boundary(scene: &mut Scene, is_start: bool, opacity: f32) {
        scene.insert_primitive(FilterBoundary {
            bounds: bounds(-100.0, -100.0, 1000.0, 1000.0),
            content_mask: source(0.0).quad.content_mask,
            corner_radii: Corners::default(),
            corner_smoothing: 0.0,
            filters: Default::default(),
            order: 0,
            opacity,
            is_start,
        });
    }

    #[test]
    fn nested_phases_and_expanded_layers_keep_paint_positions() {
        let mut scene = Scene::default();
        scene.push_layer(bounds(0.0, 0.0, 100.0, 20.0));
        let before = source(60.0).quad;
        scene.insert_primitive(before);
        let outer = scene.register_liquid_box(source(20.0)).unwrap();
        scene.paint_liquid_phase(outer, LiquidPhase::Fill);
        let inner = scene.register_liquid_box(source(25.0)).unwrap();
        scene.paint_liquid_phase(inner, LiquidPhase::Fill);
        scene.insert_primitive(source(25.0).quad);
        scene.paint_liquid_phase(inner, LiquidPhase::Border);
        scene.paint_liquid_phase(outer, LiquidPhase::Border);
        scene.pop_layer();
        scene.finish();

        let draws = &scene.liquid_draws;
        assert!(scene.quads[0].order < draws[0].order);
        assert!(draws[0].order < draws[1].order);
        assert!(draws[1].order < scene.quads[1].order);
        assert!(scene.quads[1].order < draws[2].order);
        assert!(draws[2].order < draws[3].order);
        assert_eq!(scene.liquid_components.len(), 1);
        assert_eq!(
            scene.liquid_records[0].source.quad.bounds,
            source(20.0).quad.bounds
        );
        assert_eq!(draws[0].bounds, source(20.0).coverage());
        assert!(scene.layer_stack.is_empty());
    }

    #[test]
    fn equal_groups_and_masks_preserve_component_and_descriptor_order() {
        let mut scene = Scene::default();
        let mut first = source(0.0);
        first.group = Some(SharedString::from(String::from("toolbar")));
        first.quad.content_mask.fade_out.top = ScaledPixels(f32::NAN);
        let mut second = source(30.0);
        second.group = Some(SharedString::from(String::from("toolbar")));
        second.quad.content_mask.fade_out.top = ScaledPixels(-5.0);
        paint(&mut scene, first);
        paint(&mut scene, source(0.0));
        paint(&mut scene, second);
        scene.finish();

        assert_eq!(scene.liquid_components.len(), 2);
        assert_eq!(scene.liquid_components[0].records, 0..2);
        assert_eq!(scene.liquid_components[1].records, 2..3);
        assert_eq!(
            scene
                .liquid_records
                .iter()
                .map(|record| record.ordinal)
                .collect::<Vec<_>>(),
            vec![0, 2, 1]
        );
        assert_eq!(
            scene
                .liquid_draws
                .iter()
                .map(|draw| draw.record_index)
                .collect::<Vec<_>>(),
            vec![0, 0, 2, 2, 1, 1]
        );

        for draw in &scene.liquid_draws {
            let component = &scene.liquid_components[draw.component_index];
            assert!(component.records.contains(&draw.record_index));
            assert_eq!(
                scene.liquid_records[draw.record_index].source.quad.bounds,
                draw.quad.bounds
            );
        }
    }

    #[test]
    fn clip_targets_and_overlay_epochs_are_distinct_domains() {
        let mut scene = Scene::default();
        paint(&mut scene, source(0.0));
        let mut clipped = source(0.0);
        clipped.quad.content_mask.bounds.size.width = ScaledPixels(500.0);
        paint(&mut scene, clipped);
        let mut faded = source(0.0);
        faded.quad.content_mask.fade_out.top = ScaledPixels(10.0);
        paint(&mut scene, faded);
        boundary(&mut scene, true, 0.5);
        paint(&mut scene, source(0.0));
        boundary(&mut scene, true, 0.5);
        paint(&mut scene, source(0.0));
        boundary(&mut scene, false, 0.5);
        paint(&mut scene, source(0.0));
        boundary(&mut scene, false, 0.5);
        paint(&mut scene, source(0.0));
        boundary(&mut scene, true, 0.5);
        paint(&mut scene, source(0.0));
        boundary(&mut scene, false, 0.5);
        let main_order = scene.liquid_draws.last().unwrap().order;
        scene.raise_order_floor();
        paint(&mut scene, source(0.0));
        scene.finish();

        assert_eq!(scene.liquid_components.len(), 7);
        assert_eq!(scene.liquid_components[0].records.len(), 2);
        assert_eq!(scene.liquid_components[3].records.len(), 2);
        assert!(scene.liquid_draws[scene.liquid_draws.len() - 2].order > main_order);

        let mut replayed = Scene::default();
        replayed.replay(0..scene.len(), &scene);
        replayed.finish();
        assert_eq!(replayed.liquid_components.len(), 7);
        assert_eq!(replayed.render_commands(), scene.render_commands());
        assert!(replayed.liquid.isolation_stack.is_empty());

        let mut another_window = Scene::default();
        let other_member = paint(&mut another_window, source(0.0));
        another_window.finish();
        assert_ne!(other_member.generation, scene.liquid.generation);
        assert_eq!(another_window.liquid_components.len(), 1);
    }

    #[test]
    fn inactive_sources_are_rejected_but_border_and_clip_reach_remain() {
        let mut scene = Scene::default();
        let mut empty = source(0.0);
        empty.quad.bounds.size.width = ScaledPixels(0.0);
        assert!(scene.register_liquid_box(empty).is_none());
        let mut undecorated = source(0.0);
        undecorated.quad.background = rgb(0x000000).into();
        undecorated.quad.background = undecorated.quad.background.opacity(0.0);
        assert!(scene.register_liquid_box(undecorated).is_none());
        boundary(&mut scene, true, 0.0);
        boundary(&mut scene, true, 1.0);
        paint(&mut scene, source(0.0));
        boundary(&mut scene, false, 1.0);
        boundary(&mut scene, false, 0.0);

        let mut border_only = source(0.0);
        border_only.quad.background = border_only.quad.background.opacity(0.0);
        border_only.quad.border_color = rgb(0xffffff).into();
        border_only.quad.border_widths.top = ScaledPixels(0.35);
        border_only.blend_radius = ScaledPixels(f32::NAN);
        paint(&mut scene, border_only);
        let mut outside = source(-30.0);
        outside.quad.content_mask.bounds = bounds(0.0, 0.0, 50.0, 50.0);
        assert!(
            outside
                .quad
                .bounds
                .intersect(&outside.quad.content_mask.bounds)
                .is_empty()
        );
        paint(&mut scene, outside);
        scene.finish();

        assert_eq!(scene.liquid_records.len(), 2);
        assert_eq!(
            scene.liquid_records[0].source.blend_radius,
            ScaledPixels(0.0)
        );
        assert_eq!(
            scene.liquid_records[0].source.quad.border_widths.top,
            ScaledPixels(0.35)
        );
        assert_eq!(scene.liquid_draws.len(), 4);
    }

    #[test]
    fn blend_radius_is_sanitized_after_resolution() {
        for (requested, expected) in [
            (-5.0, 0.0),
            (f32::NEG_INFINITY, 0.0),
            (f32::INFINITY, 0.0),
            (f32::NAN, 0.0),
            (0.0, 0.0),
            (0.35, 0.35),
            (24.0, 24.0),
        ] {
            let mut scene = Scene::default();
            let mut authored = source(0.0);
            authored.blend_radius = ScaledPixels(requested);
            paint(&mut scene, authored);
            scene.finish();

            assert_eq!(
                scene.liquid_records[0].source.blend_radius,
                ScaledPixels(expected)
            );
        }
    }

    #[test]
    fn split_replay_rebuilds_indices_and_joins_fresh_neighbors() {
        let mut previous = Scene::default();
        paint(&mut previous, source(0.0));
        let end_first = previous.len();
        paint(&mut previous, source(200.0));
        previous.finish();

        let mut current = Scene::default();
        paint(&mut current, source(500.0));
        // Replay only a fill marker, with its registration in a different range.
        current.replay(1..2, &previous);
        current.replay(0..1, &previous);
        current.replay(2..end_first, &previous);
        paint(&mut current, source(30.0));
        current.finish();

        assert_eq!(current.liquid_records.len(), 3);
        assert_eq!(current.liquid_components.len(), 2);
        assert_eq!(current.liquid_components[1].records, 1..3);
        assert_eq!(current.liquid_draws[2].record_index, 1);
        assert_eq!(current.liquid_draws[3].record_index, 1);
        assert_ne!(current.liquid.generation, previous.liquid.generation);
        let records_before = current
            .liquid_records
            .iter()
            .map(|record| record.source.quad.bounds)
            .collect::<Vec<_>>();
        current.finish();
        assert_eq!(
            records_before,
            current
                .liquid_records
                .iter()
                .map(|record| record.source.quad.bounds)
                .collect::<Vec<_>>()
        );

        current.clear();
        current.replay(end_first..previous.len(), &previous);
        current.finish();
        assert_eq!(current.liquid_records.len(), 1);
        assert_eq!(current.liquid_draws[0].record_index, 0);
        assert_eq!(
            current.liquid_records[0].source.quad.bounds,
            source(200.0).quad.bounds
        );
    }

    #[test]
    fn invisible_cached_sources_activate_in_the_current_opacity_target() {
        let mut previous = Scene::default();
        boundary(&mut previous, true, 0.0);
        let start = previous.len();
        paint(&mut previous, source(0.0));
        let end = previous.len();
        boundary(&mut previous, false, 0.0);
        previous.finish();
        assert!(previous.liquid_records.is_empty());
        assert!(previous.liquid_draws.is_empty());

        let mut current = Scene::default();
        boundary(&mut current, true, 0.5);
        current.replay(start..end, &previous);
        boundary(&mut current, false, 0.5);
        current.finish();
        assert_eq!(current.liquid_records.len(), 1);
        assert_eq!(current.liquid_draws.len(), 2);
    }

    #[test]
    fn dashed_component_falls_back_without_changing_ordinary_quad_storage() {
        let mut scene = Scene::default();
        let ordinary = source(0.0).quad;
        scene.insert_primitive(ordinary);
        scene.finish();
        assert_eq!(scene.quads.len(), 1);
        assert!(scene.liquid_records.is_empty());
        assert!(scene.liquid_draws.is_empty());
        assert!(matches!(
            scene.batches().collect::<Vec<_>>().as_slice(),
            [PrimitiveBatch::Quads { .. }]
        ));

        let mut dashed = source(0.0);
        dashed.quad.border_style = BorderStyle::Dashed;
        paint(&mut scene, dashed);
        paint(&mut scene, source(30.0));
        scene.finish();
        assert_eq!(scene.quads.len(), 1);
        assert_eq!(scene.liquid_components.len(), 1);
        assert!(scene.liquid_components[0].fallback);
        assert_eq!(scene.liquid_components[0].records, 0..2);
    }

    #[test]
    fn invalid_numeric_geometry_cannot_enter_ordering_or_gpu_records() {
        let mut scene = Scene::default();

        for value in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, f32::MAX] {
            let mut invalid = source(value);
            invalid.quad.bounds.size.width = ScaledPixels(f32::MAX);
            assert!(scene.register_liquid_box(invalid).is_none());
        }

        let mut invalid_style = source(0.0);
        invalid_style.quad.border_widths = Edges::all(ScaledPixels(f32::INFINITY));
        invalid_style.quad.corner_radii = Corners::all(ScaledPixels(f32::NAN));
        invalid_style.quad.corner_smoothing = f32::NAN;
        invalid_style.blend_radius = ScaledPixels(f32::MAX);
        paint(&mut scene, invalid_style);
        scene.finish();
        let source = &scene.liquid_records[0].source;
        assert_eq!(source.quad.border_widths, Edges::default());
        assert_eq!(source.quad.corner_radii, Corners::default());
        assert_eq!(source.quad.corner_smoothing, 0.0);
        assert_eq!(source.blend_radius.0, f32::MAX);
    }
}
