//! Shared liquid field and material math. Renderer scheduling is separate.

#[wgsl_rs::wgsl]
mod source {
    use super::super::common::*;
    use super::super::corner_smoothing::*;
    use super::super::quad::*;
    use wgsl_rs::std::*;

    // A single packed table holds source headers and variable-length contours.
    // Cells keep the downlevel transport at group 1, binding 0.
    pub const LIQUID_RECORD_CELLS: u32 = 21u32;
    // Reserved counts represent exact insets without uploaded contour segments.
    pub const LIQUID_PARALLEL_INSET: u32 = 0xffffffffu32;
    pub const LIQUID_CIRCULAR_INSET: u32 = 0xfffffffeu32;

    #[repr(C)]
    #[derive(Clone, Copy, Wgsl)]
    pub struct LiquidCell {
        pub words: Vec4u,
    }

    storage!(group(1), binding(0), LIQUID_CELLS: RuntimeArray<LiquidCell>);

    #[repr(C)]
    #[derive(Clone, Copy, Wgsl)]
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

    pub fn liquid_word(base: u32, word: u32) -> u32 {
        let cell_idx = base + word / 4u32;

        return get!(LIQUID_CELLS)[cell_idx as usize].words[word % 4u32];
    }

    pub fn liquid_float(base: u32, word: u32) -> f32 {
        return bitcast_f32(liquid_word(base, word));
    }

    pub fn liquid_vector(base: u32, word: u32) -> Vec4f {
        return vec4f(
            liquid_float(base, word),
            liquid_float(base, word + 1u32),
            liquid_float(base, word + 2u32),
            liquid_float(base, word + 3u32),
        );
    }

    pub fn liquid_hsla(base: u32, word: u32) -> Hsla {
        let value = liquid_vector(base, word);

        return Hsla {
            h: value.x,
            s: value.y,
            l: value.z,
            a: value.w,
        };
    }

    pub fn liquid_background(base: u32, word: u32) -> Background {
        let tag = liquid_word(base, word);
        let mut background_tag = BackgroundTag::Solid;

        if tag == 1u32 {
            background_tag = BackgroundTag::LinearGradient;
        } else if tag == 2u32 {
            background_tag = BackgroundTag::PatternSlash;
        } else if tag == 3u32 {
            background_tag = BackgroundTag::Checkerboard;
        }

        let mut color_space = ColorSpace::Srgb;

        if liquid_word(base, word + 1u32) == 1u32 {
            color_space = ColorSpace::Oklab;
        }

        return Background {
            tag: background_tag,
            color_space,
            solid: liquid_hsla(base, word + 2u32),
            gradient_angle_or_pattern_height: liquid_float(base, word + 6u32),
            colors: [
                LinearColorStop {
                    color: liquid_hsla(base, word + 7u32),
                    percentage: liquid_float(base, word + 11u32),
                },
                LinearColorStop {
                    color: liquid_hsla(base, word + 12u32),
                    percentage: liquid_float(base, word + 16u32),
                },
            ],
            padding: 0u32,
        };
    }

    pub fn liquid_source(source_idx: u32) -> LiquidGpuRecord {
        let base = source_idx * LIQUID_RECORD_CELLS;
        let bounds = liquid_vector(base, 4u32);
        let mask = liquid_vector(base, 8u32);
        let fade = liquid_vector(base, 12u32);
        let radii = liquid_vector(base, 52u32);
        let widths = liquid_vector(base, 56u32);

        return LiquidGpuRecord {
            quad: Quad {
                order: 0u32,
                border_style: BorderStyle::Solid,
                border_dashed_length: 0.0,
                border_dashed_gap: 0.0,
                bounds: Bounds {
                    origin: bounds.xy(),
                    size: bounds.zw(),
                },
                content_mask: ContentMask {
                    bounds: Bounds {
                        origin: mask.xy(),
                        size: mask.zw(),
                    },
                    fade_out: Edges {
                        top: fade.x,
                        right: fade.y,
                        bottom: fade.z,
                        left: fade.w,
                    },
                },
                background: liquid_background(base, 16u32),
                border_color: liquid_background(base, 34u32),
                corner_radii: Corners {
                    top_left: radii.x,
                    top_right: radii.y,
                    bottom_right: radii.z,
                    bottom_left: radii.w,
                },
                border_widths: Edges {
                    top: widths.x,
                    right: widths.y,
                    bottom: widths.z,
                    left: widths.w,
                },
                corner_smoothing: liquid_float(base, 60u32),
                padding: 0u32,
            },
            horizontal_reaches: [
                liquid_float(base, 62u32),
                liquid_float(base, 63u32),
                liquid_float(base, 64u32),
                liquid_float(base, 65u32),
            ],
            vertical_reaches: [
                liquid_float(base, 66u32),
                liquid_float(base, 67u32),
                liquid_float(base, 68u32),
                liquid_float(base, 69u32),
            ],
            smoothing_factors: [
                liquid_float(base, 70u32),
                liquid_float(base, 71u32),
                liquid_float(base, 72u32),
                liquid_float(base, 73u32),
            ],
            superellipse_power: liquid_float(base, 74u32),
            component_start: liquid_word(base, 75u32),
            component_count: liquid_word(base, 76u32),
            reach: liquid_float(base, 77u32),
            radius: liquid_float(base, 78u32),
            inner_start: liquid_word(base, 79u32),
            inner_count: liquid_word(base, 80u32),
            outer_start: liquid_word(base, 81u32),
            outer_count: liquid_word(base, 82u32),
            fill_power: liquid_float(base, 83u32),
        };
    }

    pub fn liquid_corners(source: LiquidGpuRecord) -> PreparedCorners {
        return PreparedCorners {
            horizontal_reaches: vec4f(
                source.horizontal_reaches[0],
                source.horizontal_reaches[1],
                source.horizontal_reaches[2],
                source.horizontal_reaches[3],
            ),
            vertical_reaches: vec4f(
                source.vertical_reaches[0],
                source.vertical_reaches[1],
                source.vertical_reaches[2],
                source.vertical_reaches[3],
            ),
            smoothing_factors: vec4f(
                source.smoothing_factors[0],
                source.smoothing_factors[1],
                source.smoothing_factors[2],
                source.smoothing_factors[3],
            ),
            superellipse_power: source.superellipse_power,
        };
    }

    pub fn liquid_contour(start: u32, count: u32, position: Vec2f) -> f32 {
        let mut sample = InsetDistance {
            distance: 1.0e30,
            inside: false,
        };

        let mut idx = 0u32;

        while idx < count {
            let segment = liquid_vector(start + idx, 0u32);
            sample = inset_segment_sample(sample, position, segment.xy(), segment.zw());
            idx += 1u32;
        }

        return field_safe_inner_border_signed_distance(sample);
    }

    pub fn liquid_direct_inset(quad: Quad, corners: PreparedCorners) -> u32 {
        let widths = quad.border_widths;
        let radii = quad.corner_radii;
        let width = widths.top;
        let maximum_radius = min(quad.bounds.size.x, quad.bounds.size.y) / 2.0;
        let circular = has_inner_contour(quad)
            && width > 0.0
            && width == widths.right
            && width == widths.bottom
            && width == widths.left
            && radii.top_left >= width
            && radii.top_right >= width
            && radii.bottom_right >= width
            && radii.bottom_left >= width
            && radii.top_left <= maximum_radius
            && radii.top_right <= maximum_radius
            && radii.bottom_right <= maximum_radius
            && radii.bottom_left <= maximum_radius;

        if !circular {
            return 0u32;
        }

        if quad.corner_smoothing <= 0.0 {
            return LIQUID_CIRCULAR_INSET;
        }

        if corners.superellipse_power == 0.0
            && width <= radii.top_left * 0.5
            && radii.top_left == radii.top_right
            && radii.top_left == radii.bottom_right
            && radii.top_left == radii.bottom_left
        {
            return LIQUID_PARALLEL_INSET;
        }

        return 0u32;
    }

    pub fn liquid_circular_inset(quad: Quad, position: Vec2f) -> f32 {
        let width = quad.border_widths.top;
        let radii = quad.corner_radii;
        let inset_bounds = Bounds {
            origin: quad.bounds.origin + vec2f(width, width),
            size: quad.bounds.size - vec2f(2.0 * width, 2.0 * width),
        };
        let inset_radii = Corners {
            top_left: radii.top_left - width,
            top_right: radii.top_right - width,
            bottom_right: radii.bottom_right - width,
            bottom_left: radii.bottom_left - width,
        };
        let relative = position - Bounds::center(inset_bounds);
        let radius = pick_corner_radius(relative, inset_radii);
        let delta = abs(relative) - Bounds::half_size(inset_bounds) + radius;

        return length(max(delta, vec2f(0.0, 0.0))) + min(max(delta.x, delta.y), 0.0) - radius;
    }

    pub fn liquid_distances(source: LiquidGpuRecord, position: Vec2f) -> Vec2f {
        // Bounds rejection precedes contour scans and paint preparation.
        let relative = max(
            source.quad.bounds.origin - position,
            position - source.quad.bounds.origin - source.quad.bounds.size,
        );

        if max(relative.x, relative.y) >= source.reach {
            return vec2f(source.reach, source.reach);
        }

        let mut outer = prepared_corner_signed_distance(
            position,
            source.quad.bounds,
            source.quad.corner_radii,
            source.quad.corner_smoothing,
            liquid_corners(source),
        );

        if source.outer_count > 0u32 {
            outer = liquid_contour(source.outer_start, source.outer_count, position);
        }

        if Edges::is_zero(source.quad.border_widths) {
            return vec2f(outer, outer);
        }

        let mut inner = source.reach;

        if source.inner_count == LIQUID_PARALLEL_INSET {
            return vec2f(outer, outer + source.quad.border_widths.top);
        }

        if source.inner_count > 0u32 && outer < source.reach {
            if source.inner_count == LIQUID_CIRCULAR_INSET {
                inner = liquid_circular_inset(source.quad, position);
            } else {
                inner = liquid_contour(source.inner_start, source.inner_count, position);
            }
        }

        return vec2f(outer, inner);
    }

    pub fn liquid_paint(source: LiquidGpuRecord, position: Vec2f, border: bool) -> Vec4f {
        let mut background = source.quad.background;

        if border {
            background = source.quad.border_color;
        }

        let paint = Paint::new(background, source.quad.bounds);

        return liquid_material(paint_color(paint, position, prepare_paint(paint)));
    }

    #[derive(Clone, Copy, Wgsl)]
    pub struct LiquidVarying {
        #[builtin(position)]
        pub position: Vec4f,
        #[location(0)]
        #[interpolate(flat)]
        pub source_idx: u32,
    }

    #[vertex]
    pub fn vertex_liquid(
        #[builtin(vertex_index)] vertex_id: u32,
        #[builtin(instance_index)] instance_id: u32,
    ) -> LiquidVarying {
        let source = liquid_source(instance_id);
        let padding = source.reach + 1.0;
        let bounds = Bounds {
            origin: source.quad.bounds.origin - vec2f(padding, padding),
            size: source.quad.bounds.size + vec2f(2.0 * padding, 2.0 * padding),
        };

        return LiquidVarying {
            position: rectangle_vertex(vertex_id, bounds).clip_position,
            source_idx: instance_id,
        };
    }

    #[derive(Clone, Copy, Wgsl)]
    pub struct LiquidFields {
        pub fields: Vec2f,
        pub owner: u32,
        pub active_count: u32,
        pub minimum_distance: f32,
        pub radius: f32,
        pub shell_idx: u32,
        pub shell_distance: f32,
    }

    pub fn sample_liquid_fields(
        emitter: LiquidGpuRecord,
        source_idx: u32,
        position: Vec2f,
    ) -> LiquidFields {
        let mut fields = vec2f(0.0, 0.0);
        let mut owner = source_idx;
        let mut owner_distance = 1.0e30;
        let mut minimum_distance = 1.0e30;
        let mut radius_sum = 0.0;
        let mut maximum_radius = 0.0;
        let mut active_count = 0u32;
        let mut shell_idx = source_idx;
        let mut shell_distance = 1.0e30;
        let mut local_idx = 0u32;

        while local_idx < emitter.component_count {
            let member_idx = emitter.component_start + local_idx;
            let member = liquid_source(member_idx);
            let distance = liquid_distances(member, position);
            let potential = liquid_potential(distance.x, member.reach);
            let inner = liquid_potential(distance.y, member.reach);
            fields += vec2f(potential, inner);

            if potential > 0.0 {
                active_count += 1u32;

                if prefers_owner(distance.x, member_idx >= owner, owner_distance) {
                    owner = member_idx;
                    owner_distance = distance.x;
                }

                minimum_distance = min(minimum_distance, distance.x);
                radius_sum += (potential / fields.x) * (member.radius - radius_sum);
                maximum_radius = max(maximum_radius, member.radius);

                if shell_potential(potential, inner) > 0.0
                    && (distance.x < shell_distance
                        || (distance.x == shell_distance && member_idx >= shell_idx))
                {
                    shell_idx = member_idx;
                    shell_distance = distance.x;
                }
            }

            local_idx += 1u32;
        }

        return LiquidFields {
            fields,
            owner,
            active_count,
            minimum_distance,
            radius: min(radius_sum, maximum_radius),
            shell_idx,
            shell_distance,
        };
    }

    pub fn liquid_coverage(
        emitter: LiquidGpuRecord,
        position: Vec2f,
        sample: LiquidFields,
        derivative_width: Vec2f,
    ) -> Vec2f {
        let mut coverage = contour_coverages(
            sample.fields.x,
            sample.fields.y,
            derivative_width.x,
            derivative_width.y,
        );

        if sample.active_count == 1u32 {
            let mut fill_corners = liquid_corners(emitter);
            fill_corners.superellipse_power = emitter.fill_power;
            coverage = ordinary_quad_coverage_prepared(
                emitter.quad,
                position,
                liquid_corners(emitter),
                fill_corners,
            );
        }

        return coverage;
    }

    pub fn liquid_component_material(
        emitter: LiquidGpuRecord,
        position: Vec2f,
        sample: LiquidFields,
        border: bool,
    ) -> Vec4f {
        let radius = sample.radius;
        let mut material = transparent();

        if radius <= 0.0 {
            if border && sample.shell_distance < 1.0e30 {
                material = liquid_paint(liquid_source(sample.shell_idx), position, true);
            } else if !border {
                material = liquid_paint(emitter, position, false);
            }
        } else {
            let mut normalization = 0.0;
            let mut maximum_log = -1.0e30;
            let mut local_idx = 0u32;

            while local_idx < emitter.component_count {
                let member = liquid_source(emitter.component_start + local_idx);
                let distance = liquid_distances(member, position);
                let potential = liquid_potential(distance.x, member.reach);
                let shell = shell_potential(potential, liquid_potential(distance.y, member.reach));

                if border && shell > 0.0 {
                    maximum_log = max(
                        maximum_log,
                        border_log_weight(shell, distance.x, sample.shell_distance, radius),
                    );
                } else if !border {
                    normalization +=
                        nearest_fill_weight(potential, distance.x, sample.minimum_distance, radius);
                }

                local_idx += 1u32;
            }

            let mut nearest_material = transparent();
            let mut remaining = 1.0;
            local_idx = emitter.component_count;

            while local_idx > 0u32 {
                local_idx -= 1u32;
                let member = liquid_source(emitter.component_start + local_idx);
                let distance = liquid_distances(member, position);
                let potential = liquid_potential(distance.x, member.reach);

                if potential > 0.0 {
                    let shell =
                        shell_potential(potential, liquid_potential(distance.y, member.reach));

                    if border && shell > 0.0 {
                        let weight = exp(border_log_weight(
                            shell,
                            distance.x,
                            sample.shell_distance,
                            radius,
                        ) - maximum_log);
                        material += liquid_paint(member, position, true) * weight;
                        normalization += weight;
                    } else if !border {
                        let paint = liquid_paint(member, position, false);
                        let strength = interior_strength(distance.x, radius);
                        material += paint * (remaining * strength);
                        remaining *= 1.0 - strength;
                        nearest_material += paint
                            * nearest_fill_weight(
                                potential,
                                distance.x,
                                sample.minimum_distance,
                                radius,
                            );
                    }
                }
            }

            if border && normalization > 0.0 {
                material /= normalization;
            } else if !border && normalization > 0.0 {
                material += nearest_material * (remaining / normalization);
            }
        }

        return material;
    }

    pub fn evaluate_liquid(input: LiquidVarying, border: bool) -> Vec4f {
        let emitter = liquid_source(input.source_idx);
        let position = input.position.xy();
        let sample = sample_liquid_fields(emitter, input.source_idx, position);

        // Derivatives must execute before any ownership or clipping return.
        let derivative_width = vec2f(fwidth(sample.fields.x), fwidth(sample.fields.y));

        if sample.owner != input.source_idx || sample.active_count == 0u32 {
            return transparent();
        }

        let coverage = liquid_coverage(emitter, position, sample, derivative_width);
        let mut alpha = coverage.x;

        if border {
            alpha = coverage.y;
        }

        if alpha <= 0.0 {
            return transparent();
        }

        let mask = emitter.quad.content_mask;

        if is_clipped(clip_distances(position, mask.bounds)) {
            return transparent();
        }

        let material = liquid_component_material(emitter, position, sample, border);

        return liquid_output(material, alpha, ContentMask::alpha(mask, position));
    }

    #[fragment]
    pub fn fragment_liquid_fill(input: LiquidVarying) -> Vec4f {
        return evaluate_liquid(input, false);
    }

    #[fragment]
    pub fn fragment_liquid_border(input: LiquidVarying) -> Vec4f {
        return evaluate_liquid(input, true);
    }

    pub const MIN_FIELD_BAND: f32 = 0.00001;

    pub fn liquid_potential(distance: f32, reach: f32) -> f32 {
        if reach <= 0.0 {
            return 0.0;
        }

        let influence = max(0.0, 1.0 - max(distance, -reach) / reach);

        return influence * influence;
    }

    pub fn field_coverage_with_width(field: f32, derivative_width: f32) -> f32 {
        if field <= 0.0 {
            return 0.0;
        }

        let band = max(derivative_width, MIN_FIELD_BAND);

        return saturate(0.5 + (field - 1.0) / band);
    }

    /// Call in uniform control flow before testing ownership in either phase.
    pub fn field_coverage(field: f32) -> f32 {
        let derivative_width = fwidth(field);

        return field_coverage_with_width(field, derivative_width);
    }

    pub fn contour_coverages(outer: f32, inner: f32, outer_width: f32, inner_width: f32) -> Vec2f {
        let outer_coverage = field_coverage_with_width(outer, outer_width);
        let inner_coverage = min(
            outer_coverage,
            field_coverage_with_width(inner, inner_width),
        );

        return vec2f(outer_coverage, max(0.0, outer_coverage - inner_coverage));
    }

    pub fn prefers_owner(distance: f32, later: bool, previous_distance: f32) -> bool {
        if distance <= 0.0 && previous_distance <= 0.0 {
            return later;
        }

        if distance <= 0.0 || previous_distance <= 0.0 {
            return distance <= 0.0;
        }

        return distance < previous_distance || (distance == previous_distance && later);
    }

    pub fn nearest_fill_weight(
        potential: f32,
        distance: f32,
        minimum_distance: f32,
        radius: f32,
    ) -> f32 {
        if radius <= 0.0 || potential <= 0.0 {
            return 0.0;
        }

        let weight = max(0.0, 1.0 - (distance - minimum_distance) / radius);

        return potential * weight * weight;
    }

    pub fn interior_strength(distance: f32, radius: f32) -> f32 {
        if radius <= 0.0 {
            return 0.0;
        }

        // Normalize first so arbitrarily small positive radii remain well-defined.
        let ratio = saturate(-distance / radius);

        return ratio * ratio * (3.0 - 2.0 * ratio);
    }

    pub fn shell_potential(outer: f32, inner: f32) -> f32 {
        return max(0.0, outer - inner);
    }

    /// Subtracting the nearest shell distance before dividing avoids overflow for
    /// tiny radii. A second shared maximum subtraction precedes exponentiation.
    pub fn border_log_weight(
        shell: f32,
        distance: f32,
        minimum_shell_distance: f32,
        radius: f32,
    ) -> f32 {
        return log(shell) - (distance - minimum_shell_distance) / radius;
    }

    /// Paint samples are already linear. Keep transparent contributors in the sum.
    pub fn liquid_material(color: Vec4f) -> Vec4f {
        return premultiply(color, 1.0);
    }

    /// Convert mixed premultiplied material back to the ordinary output contract.
    pub fn liquid_output(material: Vec4f, coverage: f32, fade: f32) -> Vec4f {
        if material.w <= 0.0 {
            return transparent();
        }

        let straight = vec4f(
            material.x / material.w,
            material.y / material.w,
            material.z / material.w,
            material.w,
        );

        return blend_color(straight, coverage * fade);
    }
}

pub use source::*;
