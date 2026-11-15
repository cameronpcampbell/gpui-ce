//! Geometry preparation shared by packed liquid uploads and contour tests.

use crate::shaders::{
    common::*,
    corner_smoothing::{PreparedCorners, prepare_corners, prepared_corner_signed_distance},
    liquid::liquid_direct_inset,
    quad::{InsetSegment, Quad, has_inner_contour, inner_contour_value},
};

use gpui::Quad as SceneQuad;
use std::mem::swap;
use wgsl_rs::std::{Vec2f, vec2f};

pub(crate) struct PreparedLiquidGeometry {
    pub(crate) corners: PreparedCorners,
    pub(crate) inset_contour: Vec<InsetSegment>,
    pub(crate) outer_contour: Vec<InsetSegment>,
    pub(crate) direct_inset: u32,
    pub(crate) fill_power: f32,
}

impl PreparedLiquidGeometry {
    pub(crate) fn new(source: &SceneQuad) -> Self {
        let quad = Quad {
            order: 0,
            border_style: BorderStyle::Solid,
            border_dashed_length: 0.0,
            border_dashed_gap: 0.0,
            bounds: Bounds {
                origin: vec2f(0.0, 0.0),
                size: vec2f(source.bounds.size.width.0, source.bounds.size.height.0),
            },
            content_mask: source.content_mask.into(),
            background: source.background.into(),
            border_color: source.border_color.into(),
            corner_radii: source.corner_radii.into(),
            border_widths: source.border_widths.into(),
            corner_smoothing: source.corner_smoothing,
            padding: 0,
        };

        Self::prepare(quad)
    }

    fn prepare(quad: Quad) -> Self {
        let corners = prepare_corners(
            quad.bounds.size,
            quad.corner_radii,
            quad.corner_smoothing,
            Edges::is_zero(quad.border_widths),
        );

        let outer_contour = if corners.superellipse_power > 0.0 {
            prepare_contour(quad.bounds, |position| {
                prepared_corner_signed_distance(
                    position,
                    quad.bounds,
                    quad.corner_radii,
                    quad.corner_smoothing,
                    corners,
                )
            })
        } else {
            Vec::new()
        };

        let direct_inset = liquid_direct_inset(quad, corners);

        let inset_contour = if direct_inset == 0 {
            prepare_inset_contour(quad, corners)
        } else {
            Vec::new()
        };

        Self {
            corners,
            inset_contour,
            outer_contour,
            direct_inset,
            fill_power: prepare_corners(
                quad.bounds.size,
                quad.corner_radii,
                quad.corner_smoothing,
                true,
            )
            .superellipse_power,
        }
    }
}

/// Extract the existing inset at one eighth of a device pixel. Marching squares
/// retains disconnected regions left by wide asymmetric smoothed borders, which
/// cannot always be represented by rays from one interior point.
fn prepare_inset_contour(quad: Quad, corners: PreparedCorners) -> Vec<InsetSegment> {
    if !has_inner_contour(quad) || Edges::is_zero(quad.border_widths) {
        return Vec::new();
    }

    if Corners::is_zero(quad.corner_radii) {
        let lower = quad.bounds.origin + vec2f(quad.border_widths.left, quad.border_widths.top);

        let upper = quad.bounds.origin + quad.bounds.size
            - vec2f(quad.border_widths.right, quad.border_widths.bottom);

        let vertices = [
            lower,
            vec2f(upper.x, lower.y),
            upper,
            vec2f(lower.x, upper.y),
        ];

        return (0..4)
            .map(|idx| InsetSegment {
                start: vertices[idx],
                end: vertices[(idx + 1) % 4],
            })
            .collect();
    }

    prepare_contour(quad.bounds, |position| {
        inner_contour_value(quad, position, corners)
    })
}

fn prepare_contour(bounds: Bounds, value: impl Fn(Vec2f) -> f32) -> Vec<InsetSegment> {
    let step = 0.125;
    let origin = bounds.origin - vec2f(step, step);
    let columns = (bounds.size.x / step).ceil() as usize + 2;
    let rows = (bounds.size.y / step).ceil() as usize + 2;

    let mut previous: Vec<_> = (0..=columns)
        .map(|idx| value(origin + vec2f(idx as f32 * step, 0.0)))
        .collect();

    let mut current = vec![0.0; columns + 1];
    let mut contour = Vec::new();

    for row_idx in 0..rows {
        let bottom = (row_idx + 1) as f32 * step;

        for (idx, sample) in current.iter_mut().enumerate() {
            *sample = value(origin + vec2f(idx as f32 * step, bottom));
        }

        for column_idx in 0..columns {
            let values = [
                previous[column_idx],
                previous[column_idx + 1],
                current[column_idx + 1],
                current[column_idx],
            ];

            let lower = origin + vec2f(column_idx as f32 * step, row_idx as f32 * step);

            let vertices = [
                lower,
                lower + vec2f(step, 0.0),
                lower + vec2f(step, step),
                lower + vec2f(0.0, step),
            ];

            let mut crossings = [vec2f(0.0, 0.0); 4];
            let mut crossing_count = 0;

            for idx in 0..4 {
                let next = (idx + 1) % 4;

                if (values[idx] < 0.0) == (values[next] < 0.0) {
                    continue;
                }

                let ratio = values[idx] / (values[idx] - values[next]);
                crossings[crossing_count] =
                    vertices[idx] + (vertices[next] - vertices[idx]) * ratio;
                crossing_count += 1;
            }

            if crossing_count == 4 {
                let center_inside = value(lower + vec2f(step / 2.0, step / 2.0)) < 0.0;
                crossings.rotate_left(usize::from(center_inside == (values[0] < 0.0)));
            }

            contour.extend(
                crossings[..crossing_count]
                    .chunks_exact(2)
                    .map(|pair| InsetSegment {
                        start: pair[0],
                        end: pair[1],
                    }),
            );
        }

        swap(&mut previous, &mut current);
    }

    contour
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shaders::{
        liquid::{
            LIQUID_CIRCULAR_INSET, LIQUID_PARALLEL_INSET, liquid_circular_inset, liquid_potential,
        },
        quad::{
            InsetDistance, field_safe_inner_border_signed_distance, inset_segment_sample,
            segment_distance,
        },
    };

    use gpui::{Background as SceneBackground, Corners as SceneCorners, ScaledPixels, rgba};
    use std::f32::consts::TAU;

    fn quad(widths: Edges, smoothing: f32) -> Quad {
        let background: SceneBackground = rgba(0).into();

        let bounds = Bounds {
            origin: vec2f(0.0, 0.0),
            size: vec2f(80.0, 60.0),
        };

        Quad {
            order: 0,
            border_style: BorderStyle::Solid,
            border_dashed_length: 0.0,
            border_dashed_gap: 0.0,
            bounds,
            content_mask: ContentMask {
                bounds,
                fade_out: widths_uniform(0.0),
            },
            background: background.into(),
            border_color: background.into(),
            corner_radii: Corners {
                top_left: 20.0,
                top_right: 20.0,
                bottom_right: 20.0,
                bottom_left: 20.0,
            },
            border_widths: widths,
            corner_smoothing: smoothing,
            padding: 0,
        }
    }

    fn widths_uniform(width: f32) -> Edges {
        Edges {
            top: width,
            right: width,
            bottom: width,
            left: width,
        }
    }

    fn contour_distance(contour: &[InsetSegment], position: Vec2f) -> f32 {
        let sample = contour.iter().fold(
            InsetDistance {
                distance: f32::MAX,
                inside: false,
            },
            |sample, segment| inset_segment_sample(sample, position, segment.start, segment.end),
        );

        field_safe_inner_border_signed_distance(sample)
    }

    #[test]
    fn inset_contours_match_independent_ray_distances_inside_and_outside() {
        for smoothing in [0.0, 0.6] {
            let quad = quad(
                Edges {
                    top: 1.0,
                    right: 8.0,
                    bottom: 4.0,
                    left: 3.0,
                },
                smoothing,
            );

            let prepared = PreparedLiquidGeometry::prepare(quad);
            let center = vec2f(37.5, 28.5);

            let contour: Vec<_> = (0..1024)
                .map(|idx| {
                    let angle = idx as f32 * TAU / 1024.0;
                    let direction = vec2f(angle.cos(), angle.sin());
                    let mut lower = 0.0;
                    let mut upper = 120.0;

                    for _iteration in 0..24 {
                        let middle = (lower + upper) / 2.0;

                        if inner_contour_value(quad, center + direction * middle, prepared.corners)
                            < 0.0
                        {
                            lower = middle;
                        } else {
                            upper = middle;
                        }
                    }

                    center + direction * ((lower + upper) / 2.0)
                })
                .collect();

            for y in (-24..85).step_by(5) {
                for x in (-24..105).step_by(5) {
                    let position = vec2f(x as f32, y as f32);

                    let expected = contour
                        .iter()
                        .enumerate()
                        .map(|(idx, start)| {
                            segment_distance(position, *start, contour[(idx + 1) % contour.len()])
                        })
                        .fold(f32::INFINITY, f32::min);

                    let actual = contour_distance(&prepared.inset_contour, position).abs();

                    assert!(
                        (actual - expected).abs() < 0.1,
                        "smoothing {smoothing}, position {position:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn asymmetric_and_consumed_insets_keep_contour_signs() {
        for smoothing in [0.0, 0.6, 1.0] {
            for widths in [
                Edges {
                    top: 0.0,
                    right: 8.0,
                    bottom: 4.0,
                    left: 1.0,
                },
                Edges {
                    top: 20.0,
                    right: 35.0,
                    bottom: 25.0,
                    left: 35.0,
                },
            ] {
                let mut quad = quad(widths, smoothing);
                quad.corner_radii = Corners {
                    top_left: 28.0,
                    top_right: 4.0,
                    bottom_right: 19.0,
                    bottom_left: 0.0,
                };

                let prepared = PreparedLiquidGeometry::prepare(quad);

                for y in (1..60).step_by(3) {
                    for x in (1..80).step_by(3) {
                        let position = vec2f(x as f32, y as f32);
                        let value = inner_contour_value(quad, position, prepared.corners);

                        if value < -0.05 {
                            assert!(
                                !prepared.inset_contour.is_empty(),
                                "lost an inset with positive area"
                            );
                        }

                        if !prepared.inset_contour.is_empty() {
                            let distance = contour_distance(&prepared.inset_contour, position);
                            assert!(distance.is_finite());

                            if value.abs() > 0.05 {
                                assert_eq!(
                                    distance < 0.0,
                                    value < 0.0,
                                    "smoothing {smoothing}, position {position:?}"
                                );
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn analytic_circular_and_smoothed_insets_match_sampled_contours() {
        let smoothed = [0.0, 0.1, 0.6, 1.0].into_iter().flat_map(|smoothing| {
            [1.0, 8.0, 10.0].into_iter().map(move |width| {
                (
                    quad(widths_uniform(width), smoothing),
                    -24..105,
                    -24..85,
                    4,
                    f32::MAX,
                )
            })
        });

        let circular = [(1.0, 1.0), (1.0, 10.0), (8.0, 10.0), (13.0, 10.0)]
            .into_iter()
            .map(|(width, radius)| {
                let mut quad = quad(widths_uniform(width), 0.0);
                quad.bounds.size = vec2f(30.0, 24.0);
                quad.corner_radii = SceneCorners::all(ScaledPixels(radius)).into();

                (quad, -10..40, -10..35, 2, 24.0)
            });

        for (quad, columns, rows, step, maximum) in smoothed.chain(circular) {
            let prepared = PreparedLiquidGeometry::prepare(quad);
            let contour = prepare_inset_contour(quad, prepared.corners);
            let width = quad.border_widths.top;
            let smoothing = quad.corner_smoothing;

            if width > quad.corner_radii.top_left {
                assert_eq!(prepared.direct_inset, 0);
                assert!(prepared.inset_contour.is_empty());
                assert!(contour.is_empty());

                continue;
            }

            let mode = if smoothing == 0.0 {
                LIQUID_CIRCULAR_INSET
            } else {
                LIQUID_PARALLEL_INSET
            };

            assert_eq!(prepared.direct_inset, mode);

            for y in rows.step_by(step) {
                for x in columns.clone().step_by(step) {
                    let position = vec2f(x as f32 + 0.5, y as f32 + 0.5);

                    let actual = if smoothing == 0.0 {
                        liquid_circular_inset(quad, position)
                    } else {
                        prepared_corner_signed_distance(
                            position,
                            quad.bounds,
                            quad.corner_radii,
                            smoothing,
                            prepared.corners,
                        ) + width
                    };

                    let expected = contour_distance(&contour, position);

                    assert!(
                        (actual.min(maximum) - expected.min(maximum)).abs() < 0.125,
                        "size {:?}, smoothing {smoothing}, width {width}, radius {}, position {position:?}",
                        quad.bounds.size,
                        quad.corner_radii.top_left,
                    );
                }
            }
        }
    }

    #[test]
    fn smoothed_outer_support_has_finite_external_distances() {
        let mut quad = quad(widths_uniform(0.0), 0.6);
        quad.bounds.size = vec2f(100.0, 80.0);
        let prepared = PreparedLiquidGeometry::prepare(quad);
        assert!(!prepared.outer_contour.is_empty());

        for y in (-32..113).step_by(4) {
            for x in (-32..133).step_by(4) {
                let position = vec2f(x as f32, y as f32);
                let distance = contour_distance(&prepared.outer_contour, position);
                assert!(distance.is_finite());

                if position.x <= -24.0
                    || position.y <= -24.0
                    || position.x >= 124.0
                    || position.y >= 104.0
                {
                    assert_eq!(liquid_potential(distance, 24.0), 0.0);
                }
            }
        }
    }
}
