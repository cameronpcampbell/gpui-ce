//! Renders one of every primitive kind through the headless WGPU renderer and checks
//! that each one actually lands on the target. This is the cross-platform smoke test for
//! the shared render plan, instance transport, and generated shaders.
#![cfg(feature = "test-support")]

use gpui::{
    AtlasKey, AtlasTile, BackdropFilter, BorderStyle, Bounds, ColorSpace, ContentMask, Corners,
    DevicePixels, Edges, Hsla, MonochromeSprite, PlatformHeadlessRenderer, Point, PolychromeSprite,
    Quad, RenderImageParams, RenderSvgParams, ScaledFilter, ScaledPixels, Scene, ShaderBool,
    Shadow, Size, Underline, checkerboard, linear_color_stop, linear_gradient, solid_background,
};
use gpui_ce_wgpu::WgpuHeadlessRenderer;
use smallvec::smallvec;
use std::borrow::Cow;

const TARGET: Size<DevicePixels> = Size {
    width: DevicePixels(310),
    height: DevicePixels(100),
};

#[test]
fn filled_path_resolves_without_invalidating_the_frame() {
    let mut renderer = WgpuHeadlessRenderer::new().expect("headless renderer");
    let mut scene = Scene::default();
    scene.insert_primitive(Quad {
        bounds: bounds(0.0, 0.0, 200.0, 100.0),
        content_mask: full_mask(),
        background: solid_background(gpui::black()),
        ..Default::default()
    });
    let mut builder = gpui::PathBuilder::fill();
    builder.move_to(gpui::point(gpui::px(20.0), gpui::px(20.0)));
    builder.line_to(gpui::point(gpui::px(80.0), gpui::px(20.0)));
    builder.line_to(gpui::point(gpui::px(50.0), gpui::px(80.0)));
    builder.close();
    let mut path = builder.build().expect("triangle").scale(1.0);
    path.content_mask = full_mask();
    path.color = solid_background(gpui::white());
    scene.insert_primitive(path);
    scene.finish();

    // The path uses an MSAA attachment when supported. Storing that transient
    // attachment invalidates the command buffer, including the background quad.
    let image = renderer
        .render_scene_to_image(&scene, TARGET)
        .expect("valid path frame");
    assert_eq!(image.get_pixel(50, 40).0, [255, 255, 255, 255]);
    assert_eq!(image.get_pixel(150, 40).0, [0, 0, 0, 255]);
}

fn bounds(x: f32, y: f32, w: f32, h: f32) -> Bounds<ScaledPixels> {
    Bounds {
        origin: Point {
            x: ScaledPixels(x),
            y: ScaledPixels(y),
        },
        size: Size {
            width: ScaledPixels(w),
            height: ScaledPixels(h),
        },
    }
}

fn full_mask() -> ContentMask<ScaledPixels> {
    ContentMask {
        bounds: bounds(0.0, 0.0, 310.0, 100.0),
        ..Default::default()
    }
}

fn mask(width: f32, height: f32) -> ContentMask<ScaledPixels> {
    ContentMask {
        bounds: bounds(0.0, 0.0, width, height),
        ..Default::default()
    }
}

fn tile(renderer: &WgpuHeadlessRenderer, key: AtlasKey, bytes: Vec<u8>) -> AtlasTile {
    renderer
        .sprite_atlas()
        .get_or_insert_with(&key, &mut || {
            Ok(Some((
                Size {
                    width: DevicePixels(8),
                    height: DevicePixels(8),
                },
                Cow::Owned(bytes.clone()),
            )))
        })
        .expect("atlas insert must succeed")
        .expect("atlas insert must produce a tile")
}

#[test]
fn every_primitive_kind_renders() {
    let mut renderer = WgpuHeadlessRenderer::new().expect("headless renderer");
    let mono = tile(
        &renderer,
        AtlasKey::Svg(RenderSvgParams {
            path: "test-mono".into(),
            size: Size {
                width: DevicePixels(8),
                height: DevicePixels(8),
            },
        }),
        vec![255; 64],
    );
    let poly = tile(
        &renderer,
        AtlasKey::Image(RenderImageParams {
            image_id: gpui::ImageId(1),
            frame_index: 0,
        }),
        // BGRA red, opaque.
        (0..64).flat_map(|_| [0u8, 0, 255, 255]).collect(),
    );

    let mut scene = Scene::default();
    let green: Hsla = gpui::rgb_to_hsla(gpui::rgb(0x00ff00));
    let red: Hsla = gpui::rgb_to_hsla(gpui::rgb(0xff0000));
    let white: Hsla = gpui::rgb_to_hsla(gpui::rgb(0xffffff));
    let blue: Hsla = gpui::rgb_to_hsla(gpui::rgb(0x0000ff));

    // 1. Solid quad.
    let quad_bounds = bounds(10.0, 10.0, 30.0, 30.0);
    scene.insert_primitive(Quad {
        order: 0,
        bounds: quad_bounds,
        content_mask: full_mask(),
        background: solid_background(green),
        ..Default::default()
    });
    // 2. Bordered quad (white border, transparent fill).
    let bordered = bounds(50.0, 10.0, 30.0, 30.0);
    scene.insert_primitive(Quad {
        order: 0,
        bounds: bordered,
        content_mask: full_mask(),
        border_color: white.into(),
        border_widths: gpui::Edges::all(ScaledPixels(4.0)),
        ..Default::default()
    });
    // 3. Shadow (blue, no blur so it is a solid block).
    let shadow_bounds = bounds(90.0, 10.0, 30.0, 30.0);
    scene.insert_primitive(Shadow {
        order: 0,
        blur_radius: ScaledPixels(0.0),
        bounds: shadow_bounds,
        corner_radii: Default::default(),
        content_mask: full_mask(),
        color: blue.into(),
        element_bounds: shadow_bounds,
        element_corner_radii: Default::default(),
        inset: ShaderBool::Disabled,
        corner_smoothing: 0.0,
    });
    // 4. Underline (white, solid).
    let underline_bounds = bounds(130.0, 20.0, 30.0, 4.0);
    scene.insert_primitive(Underline {
        order: 0,
        padding: 0,
        bounds: underline_bounds,
        content_mask: full_mask(),
        color: white.into(),
        thickness: ScaledPixels(4.0),
        wavy: ShaderBool::Disabled,
    });
    // 5. Monochrome sprite (white coverage tile tinted green).
    let mono_bounds = bounds(10.0, 60.0, 30.0, 30.0);
    scene.insert_primitive(MonochromeSprite {
        order: 0,
        padding: 0,
        bounds: mono_bounds,
        content_mask: full_mask(),
        color: green.into(),
        tile: mono,
        transformation: Default::default(),
    });
    // 6. Polychrome sprite (red image).
    let poly_bounds = bounds(50.0, 60.0, 30.0, 30.0);
    scene.insert_primitive(PolychromeSprite {
        order: 0,
        grayscale: ShaderBool::Disabled,
        opacity: 1.0,
        corner_smoothing: 0.0,
        bounds: poly_bounds,
        content_mask: full_mask(),
        corner_radii: Default::default(),
        tile: poly,
    });
    // 7. A second quad batch. Overlapping the image pushes this quad above it in draw
    //    order, so it starts past the first quads in the frame's quad buffer: a backend
    //    that loses the batch base draws the wrong quads here.
    let late_quad = bounds(60.0, 70.0, 30.0, 30.0);
    scene.insert_primitive(Quad {
        order: 0,
        bounds: late_quad,
        content_mask: full_mask(),
        background: solid_background(blue),
        ..Default::default()
    });
    // 8–9. Gradient drop and inset shadows. The inset gradient stays in the element's paint
    // coordinate space while its offset hole is smaller.
    let gradient = linear_gradient(
        90.0,
        linear_color_stop(red, 0.0),
        linear_color_stop(blue, 1.0),
    )
    .color_space(ColorSpace::Srgb);
    let gradient_drop_bounds = bounds(210.0, 10.0, 40.0, 40.0);
    scene.insert_primitive(Shadow {
        order: 0,
        blur_radius: ScaledPixels(0.0),
        bounds: gradient_drop_bounds,
        corner_radii: Default::default(),
        content_mask: full_mask(),
        color: gradient,
        element_bounds: gradient_drop_bounds,
        element_corner_radii: Default::default(),
        inset: ShaderBool::Disabled,
        corner_smoothing: 0.0,
    });
    scene.insert_primitive(Shadow {
        order: 0,
        blur_radius: ScaledPixels(0.0),
        bounds: bounds(266.0, 16.0, 28.0, 28.0),
        corner_radii: Default::default(),
        content_mask: full_mask(),
        color: gradient,
        element_bounds: bounds(260.0, 10.0, 40.0, 40.0),
        element_corner_radii: Default::default(),
        inset: ShaderBool::Enabled,
        corner_smoothing: 0.0,
    });
    scene.finish();
    let quad_batches: Vec<_> = scene
        .render_commands()
        .iter()
        .filter_map(|command| match command {
            gpui::RenderCommand::Batch(gpui::PrimitiveBatch::Quads { range, .. }) => {
                Some(range.clone())
            }
            _ => None,
        })
        .collect();
    assert_eq!(
        quad_batches,
        vec![0..2, 2..3],
        "the scene must split quads across batches"
    );

    let image = renderer
        .render_scene_to_image(&scene, TARGET)
        .expect("render must succeed");
    let px = |x: u32, y: u32| {
        let p = image.get_pixel(x, y).0;
        (p[0], p[1], p[2], p[3])
    };
    let mut failures = Vec::new();
    let mut check = |name: &str, x: u32, y: u32, expected: (u8, u8, u8)| {
        let (r, g, b, _) = px(x, y);
        let close = |a: u8, b: u8| (a as i32 - b as i32).abs() <= 8;
        if !(close(r, expected.0) && close(g, expected.1) && close(b, expected.2)) {
            failures.push(format!(
                "{name} at ({x},{y}): got ({r},{g},{b}) expected {expected:?}"
            ));
        }
    };
    check("solid quad", 25, 25, (0, 255, 0));
    check("bordered quad border", 52, 25, (255, 255, 255));
    check("bordered quad interior", 65, 25, (0, 0, 0));
    check("shadow", 105, 25, (0, 0, 255));
    check("underline", 145, 22, (255, 255, 255));
    check("monochrome sprite", 25, 75, (0, 255, 0));
    check("polychrome sprite", 55, 65, (255, 0, 0));
    check("second quad batch", 85, 85, (0, 0, 255));
    check("background", 190, 90, (0, 0, 0));
    check("gradient inset shadow hole", 280, 30, (0, 0, 0));
    let mut check_dominant = |name: &str, x: u32, y: u32, red_dominant: bool| {
        let (r, g, b, _) = px(x, y);
        let matches = if red_dominant {
            r > 180 && g < 80 && b < 80
        } else {
            b > 180 && r < 80 && g < 80
        };
        if !matches {
            failures.push(format!(
                "{name} at ({x},{y}): got ({r},{g},{b}), expected {} dominance",
                if red_dominant { "red" } else { "blue" }
            ));
        }
    };
    check_dominant("gradient drop shadow left edge", 212, 30, true);
    check_dominant("gradient drop shadow right edge", 247, 30, false);
    check_dominant("gradient inset shadow left edge", 262, 30, true);
    check_dominant("gradient inset shadow right edge", 297, 30, false);
    if !failures.is_empty() {
        let path = std::env::temp_dir().join("gpui_headless_primitives.png");
        image.save(&path).ok();
        panic!(
            "{}\n(image saved to {})",
            failures.join("\n"),
            path.display()
        );
    }
}

#[test]
fn smoothed_primitives_share_one_contour() {
    let mut renderer = WgpuHeadlessRenderer::new().expect("headless renderer");
    let image_tile = tile(
        &renderer,
        AtlasKey::Image(RenderImageParams {
            image_id: gpui::ImageId(2),
            frame_index: 0,
        }),
        (0..64).flat_map(|_| [0u8, 0, 255, 255]).collect(),
    );
    let target = Size {
        width: DevicePixels(360),
        height: DevicePixels(170),
    };
    let content_mask = mask(360.0, 170.0);
    let green: Hsla = gpui::rgb_to_hsla(gpui::rgb(0x19d36b));
    let white: Hsla = gpui::rgb_to_hsla(gpui::rgb(0xffffff));
    let blue: Hsla = gpui::rgb_to_hsla(gpui::rgb(0x287cff));
    let black: Hsla = gpui::rgb_to_hsla(gpui::rgb(0x000000));
    let radii = Corners::all(ScaledPixels(18.0));

    let mut scene = Scene::default();

    let fill_bounds = bounds(10.0, 10.0, 50.0, 50.0);
    scene.insert_primitive(Quad {
        bounds: fill_bounds,
        content_mask,
        background: solid_background(green),
        corner_radii: radii,
        corner_smoothing: 1.0,
        ..Default::default()
    });

    let border_bounds = bounds(72.0, 10.0, 50.0, 50.0);
    scene.insert_primitive(Quad {
        bounds: border_bounds,
        content_mask,
        border_color: white.into(),
        border_widths: Edges {
            top: ScaledPixels(2.0),
            right: ScaledPixels(5.0),
            bottom: ScaledPixels(8.0),
            left: ScaledPixels(3.0),
        },
        corner_radii: radii,
        corner_smoothing: 0.6,
        ..Default::default()
    });

    let dashed_bounds = bounds(134.0, 10.0, 58.0, 50.0);
    scene.insert_primitive(Quad {
        bounds: dashed_bounds,
        content_mask,
        border_style: BorderStyle::Dashed,
        border_color: white.into(),
        border_widths: Edges::all(ScaledPixels(3.0)),
        corner_radii: Corners {
            top_left: ScaledPixels(20.0),
            top_right: ScaledPixels(8.0),
            bottom_right: ScaledPixels(16.0),
            bottom_left: ScaledPixels(3.0),
        },
        corner_smoothing: 1.0,
        ..Default::default()
    });

    let image_bounds = bounds(204.0, 10.0, 50.0, 50.0);
    scene.insert_primitive(PolychromeSprite {
        order: 0,
        grayscale: ShaderBool::Disabled,
        opacity: 1.0,
        corner_smoothing: 1.0,
        bounds: image_bounds,
        content_mask,
        corner_radii: radii,
        tile: image_tile,
    });

    let drop_element = bounds(20.0, 96.0, 50.0, 44.0);
    scene.insert_primitive(Shadow {
        order: 0,
        blur_radius: ScaledPixels(5.0),
        bounds: bounds(25.0, 101.0, 50.0, 44.0),
        corner_radii: Corners::all(ScaledPixels(15.0)),
        content_mask,
        color: blue.into(),
        element_bounds: drop_element,
        element_corner_radii: Corners::all(ScaledPixels(15.0)),
        inset: ShaderBool::Disabled,
        corner_smoothing: 0.6,
    });
    scene.insert_primitive(Quad {
        bounds: drop_element,
        content_mask,
        background: solid_background(green),
        corner_radii: Corners::all(ScaledPixels(15.0)),
        corner_smoothing: 0.6,
        ..Default::default()
    });

    let inset_bounds = bounds(100.0, 94.0, 50.0, 48.0);
    scene.insert_primitive(Quad {
        bounds: inset_bounds,
        content_mask,
        background: solid_background(green),
        corner_radii: Corners::all(ScaledPixels(16.0)),
        corner_smoothing: 1.0,
        ..Default::default()
    });
    scene.insert_primitive(Shadow {
        order: 0,
        blur_radius: ScaledPixels(4.0),
        bounds: bounds(104.0, 98.0, 42.0, 40.0),
        corner_radii: Corners::all(ScaledPixels(12.0)),
        content_mask,
        color: black.into(),
        element_bounds: inset_bounds,
        element_corner_radii: Corners::all(ScaledPixels(16.0)),
        inset: ShaderBool::Enabled,
        corner_smoothing: 1.0,
    });

    let filter_bounds = bounds(190.0, 92.0, 70.0, 52.0);
    let backdrop = Quad {
        bounds: filter_bounds,
        content_mask,
        background: checkerboard(white, 2.0),
        ..Default::default()
    };
    scene.insert_primitive(backdrop);
    scene.insert_primitive(BackdropFilter {
        order: 0,
        bounds: filter_bounds,
        content_mask,
        corner_radii: Corners::all(ScaledPixels(18.0)),
        corner_smoothing: 1.0,
        filters: smallvec![ScaledFilter::Blur(ScaledPixels(5.0))],
        opacity: 1.0,
    });

    scene.finish();
    let image = renderer
        .render_scene_to_image(&scene, target)
        .expect("render must succeed");
    let mut unfiltered_scene = Scene::default();
    unfiltered_scene.insert_primitive(backdrop);
    unfiltered_scene.finish();
    let unfiltered = renderer
        .render_scene_to_image(&unfiltered_scene, target)
        .expect("reference render must succeed");
    let pixel = |x: u32, y: u32| image.get_pixel(x, y).0;
    let reference_pixel = |x: u32, y: u32| unfiltered.get_pixel(x, y).0;
    let is_black = |x, y| pixel(x, y)[0..3].iter().all(|channel| *channel <= 12);
    let is_white = |x, y| pixel(x, y)[0..3].iter().all(|channel| *channel >= 235);

    for (name, left, center, channel, minimum) in
        [("fill", 10, 35, 1, 180), ("image", 204, 229, 0, 220)]
    {
        assert!(
            is_black(left, 10),
            "smoothed {name} must exclude its corner"
        );
        assert!(
            pixel(center, 10)[channel] > minimum,
            "{name} shoulder must reach the top edge",
        );
        assert!(pixel(center, 35)[channel] > minimum, "{name} center");
    }

    for (side, outer, inner) in [
        ("top", (96, 10), (96, 14)),
        ("right", (120, 35), (115, 35)),
        ("bottom", (96, 56), (96, 50)),
    ] {
        assert!(is_white(outer.0, outer.1), "{side} border");
        assert!(
            is_black(inner.0, inner.1),
            "{side} border interior boundary"
        );
    }

    let dashed_pixels = (8..62)
        .flat_map(|y| (132..194).map(move |x| (x, y)))
        .filter(|&(x, y)| is_white(x, y))
        .count();
    assert!(
        (150..420).contains(&dashed_pixels),
        "dashed contour coverage: {dashed_pixels}"
    );
    assert!(is_black(163, 35), "dashed border interior");

    assert!(
        pixel(73, 132)[2] > 35,
        "drop shadow must extend beyond the element"
    );
    assert!(
        pixel(125, 96)[1] < pixel(125, 118)[1],
        "inset shadow must darken the edge"
    );
    assert!(
        pixel(125, 118)[1] > 120,
        "inset shadow must preserve the center"
    );

    let blurred_midtones = (192..258)
        .flat_map(|x| (94..142).map(move |y| pixel(x, y)[0]))
        .filter(|channel| (30..225).contains(channel))
        .count();
    assert!(
        blurred_midtones > 800,
        "backdrop blur did not composite: {blurred_midtones}"
    );
    assert!(
        pixel(190, 92)
            .iter()
            .zip(reference_pixel(190, 92))
            .all(|(&actual, expected)| actual.abs_diff(expected) <= 1),
        "the smoothed mask must exclude the backdrop corner"
    );
    assert!(
        pixel(225, 92)[0].abs_diff(reference_pixel(225, 92)[0]) > 20,
        "the smoothed mask must include the top shoulder"
    );

    if std::env::var_os("GPUI_SAVE_HEADLESS_TESTS").is_some() {
        image
            .save(std::env::temp_dir().join("gpui_smoothed_primitives.png"))
            .expect("save diagnostic image");
    }
}

#[test]
fn content_mask_fades_quad_edges() {
    let mut renderer = WgpuHeadlessRenderer::new().expect("headless renderer");
    let mut scene = Scene::default();
    let red: Hsla = gpui::rgb_to_hsla(gpui::rgb(0xff0000));
    scene.insert_primitive(Quad {
        order: 0,
        bounds: bounds(0.0, 0.0, 310.0, 100.0),
        content_mask: ContentMask {
            bounds: bounds(0.0, 0.0, 100.0, 100.0),
            fade_out: Edges {
                top: ScaledPixels(10.0),
                right: ScaledPixels(0.0),
                bottom: ScaledPixels(0.0),
                left: ScaledPixels(20.0),
            },
        },
        background: solid_background(red),
        ..Default::default()
    });
    scene.finish();
    let image = renderer
        .render_scene_to_image(&scene, TARGET)
        .expect("render must succeed");
    let pixel = |x: u32, y: u32| image.get_pixel(x, y).0;
    let close = |x: u32, y: u32, red: u8| {
        let actual = pixel(x, y)[0];
        assert!(
            actual.abs_diff(red) <= 3,
            "at ({x},{y}) got {actual}, expected {red}"
        );
    };

    // Fully inside both fades.
    close(60, 50, 255);
    // The ramp is evaluated at pixel centers (x + 0.5), so 10.5px into the
    // 20px left fade is 52.5% coverage.
    close(10, 50, 134);
    // 2.5px into the 10px top fade is 25% coverage.
    close(60, 2, 64);
    // The unfaded right edge still clips hard at the mask boundary.
    assert_eq!(pixel(101, 50)[0], 0);
}

#[cfg(test)]
mod liquid_tests {
    use super::*;
    use gpui::{FilterBoundary, LiquidMemberId, LiquidPhase, ScaledLiquidBox, pattern_slash};
    use gpui_render::liquid::testing::PreparedLiquidComponent;
    use gpui_render::shaders::liquid::contour_coverages;
    use wgsl_rs::std::{abs, vec2f, vec4f};

    fn review_image(image: &image::RgbaImage, case_idx: usize) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!("liquid-material-{case_idx}.ppm"));
        let mut bytes = format!("P6\n{} {}\n255\n", image.width(), image.height()).into_bytes();

        for pixel in image.pixels() {
            bytes.extend_from_slice(&pixel.0[..3]);
        }

        std::fs::write(&path, bytes).unwrap();

        path
    }

    fn register(scene: &mut Scene, quad: Quad, group: Option<&str>, radius: f32) -> LiquidMemberId {
        scene
            .register_liquid_box(ScaledLiquidBox {
                quad,
                group: group.map(Into::into),
                blend_radius: ScaledPixels(radius),
                reach: ScaledPixels(24.0),
            })
            .unwrap()
    }

    fn paint(scene: &mut Scene, quad: Quad, group: Option<&str>) {
        let member = register(scene, quad, group, 8.0);
        scene.paint_liquid_phase(member, LiquidPhase::Fill);
        scene.paint_liquid_phase(member, LiquidPhase::Border);
    }

    fn render(renderer: &mut WgpuHeadlessRenderer, scene: &mut Scene) -> image::RgbaImage {
        scene.finish();

        renderer.render_scene_to_image(scene, TARGET).unwrap()
    }

    fn ordinary_scene(liquid: &Scene) -> Scene {
        let mut scene = Scene::default();

        for draw in &liquid.liquid_draws {
            scene.insert_primitive(draw.quad);
        }

        scene
    }

    fn source(x: f32) -> Quad {
        Quad {
            bounds: bounds(x, 20.0, 30.0, 40.0),
            content_mask: full_mask(),
            background: gpui::rgba(0xff000080).into(),
            ..Default::default()
        }
    }

    #[test]
    fn bridges_and_overlaps_emit_one_translucent_material_per_domain() {
        let mut renderer = WgpuHeadlessRenderer::new().unwrap();
        let mut scene = Scene::default();
        paint(&mut scene, source(20.0), None);
        // Interleave another domain so source indices differ from draw positions.
        paint(&mut scene, source(200.0), Some("separate"));
        paint(&mut scene, source(60.0), None);
        paint(&mut scene, source(25.0), None);
        let image = render(&mut renderer, &mut scene);
        let inside = image.get_pixel(35, 40).0;
        let bridge = image.get_pixel(55, 40).0;
        assert!((inside[0] as i32 - 128).abs() <= 2, "overlap {inside:?}");
        assert!((bridge[0] as i32 - 128).abs() <= 2, "bridge {bridge:?}");
        assert_eq!(image.get_pixel(100, 40).0, [0, 0, 0, 255]);

        let mut separate = Scene::default();
        paint(&mut separate, source(20.0), None);
        paint(&mut separate, source(25.0), Some("other"));
        let image = render(&mut renderer, &mut separate);
        assert!(image.get_pixel(35, 40).0[0] > inside[0] + 40);
    }

    #[test]
    fn isolated_liquid_uses_the_active_opacity_target() {
        let mut renderer = WgpuHeadlessRenderer::new().unwrap();
        let mut scene = Scene::default();
        let boundary = FilterBoundary {
            order: 0,
            bounds: full_mask().bounds,
            content_mask: full_mask(),
            corner_radii: Default::default(),
            corner_smoothing: 0.0,
            filters: Default::default(),
            opacity: 0.5,
            is_start: true,
        };

        scene.insert_primitive(boundary.clone());
        paint(&mut scene, source(20.0), None);
        paint(&mut scene, source(60.0), None);
        scene.insert_primitive(FilterBoundary {
            is_start: false,
            ..boundary
        });

        let image = render(&mut renderer, &mut scene);
        let bridge = image.get_pixel(55, 40).0;
        assert!(
            (bridge[0] as i32 - 64).abs() <= 3,
            "isolated bridge {bridge:?}"
        );
    }

    #[test]
    fn singletons_match_ordinary_two_phase_corner_and_border_coverage() {
        let mut renderer = WgpuHeadlessRenderer::new().unwrap();

        for smoothing in [0.0, 0.6] {
            let quad = Quad {
                bounds: bounds(20.0, 20.0, 42.0, 36.0),
                corner_smoothing: smoothing,
                corner_radii: Corners::all(ScaledPixels(10.0)),
                border_widths: Edges {
                    top: ScaledPixels(0.4),
                    right: ScaledPixels(4.0),
                    bottom: ScaledPixels(2.0),
                    left: ScaledPixels(0.0),
                },
                border_color: gpui::rgba(0x0080ff80).into(),
                ..source(20.0)
            };

            let mut liquid = Scene::default();
            paint(&mut liquid, quad, None);
            let actual = render(&mut renderer, &mut liquid);
            let expected = render(&mut renderer, &mut ordinary_scene(&liquid));

            for y in 20..56 {
                for x in 20..62 {
                    let actual_pixel = actual.get_pixel(x, y).0;
                    let expected_pixel = expected.get_pixel(x, y).0;

                    for channel_idx in 0..4 {
                        assert!(
                            (actual_pixel[channel_idx] as i32 - expected_pixel[channel_idx] as i32)
                                .abs()
                                <= 3,
                            "smoothing {smoothing}, ({x}, {y}): {actual_pixel:?} versus {expected_pixel:?}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn joined_materials_and_borders_match_reference_samples() {
        let mut renderer = WgpuHeadlessRenderer::new().unwrap();
        let mut oklab = None;

        for case_idx in 0..13 {
            let mut scene = Scene::default();
            let mut first = source(20.0);
            first.corner_radii = Corners {
                top_left: ScaledPixels(12.0),
                top_right: ScaledPixels(0.0),
                bottom_right: ScaledPixels(8.0),
                bottom_left: ScaledPixels(3.0),
            };
            first.border_widths = Edges::all(ScaledPixels(1.0));
            first.border_color = gpui::rgba(0x00ff00b0).into();
            first.corner_smoothing = [0.0, 0.6, 1.0][case_idx % 3];
            let mut second = Quad {
                bounds: bounds(58.0, 24.0, 36.0, 40.0),
                background: gpui::rgba(0x0088ff80).into(),
                border_widths: Edges::all(ScaledPixels(8.0)),
                corner_radii: Corners::all(ScaledPixels(10.0)),
                ..first
            };

            match case_idx {
                1 => first.border_widths = Default::default(),
                2 => second.border_color = second.border_color.opacity(0.0),
                3 => first.background = first.background.opacity(0.0),
                4 | 12 => {
                    first.background = checkerboard(gpui::rgba(0xff000080), 5.0);
                    second.background = pattern_slash(gpui::rgba(0x0088ff80), 2.0, 6.0);

                    if case_idx == 12 {
                        first.border_color = first.background;
                        second.border_color = second.background;
                    }
                }
                5 => {
                    second.bounds = bounds(28.0, 28.0, 12.0, 20.0);
                    second.corner_radii = Corners::all(ScaledPixels(4.0));
                    second.border_widths = Edges::all(ScaledPixels(1.0));
                }
                6 | 11 => {
                    first.corner_smoothing = 0.0;
                    second.corner_smoothing = 0.0;
                    first.border_widths = Edges {
                        top: ScaledPixels(0.0),
                        right: ScaledPixels(8.0),
                        bottom: ScaledPixels(2.0),
                        left: ScaledPixels(1.0),
                    };
                    second.background = linear_gradient(
                        90.0,
                        linear_color_stop(gpui::rgba(0x0000ff80), 0.0),
                        linear_color_stop(gpui::rgba(0x00ff0000), 1.0),
                    )
                    .color_space(if case_idx == 6 {
                        ColorSpace::Oklab
                    } else {
                        ColorSpace::Srgb
                    });
                }
                7 => second.border_widths = Edges::all(ScaledPixels(40.0)),
                8 => {
                    first.corner_smoothing = 0.0;
                    second.corner_smoothing = 0.0;
                    first.corner_radii = Corners::all(ScaledPixels(10.0));
                }
                9 | 10 => {
                    first.bounds = bounds(20.0, 20.0, 40.0, 40.0);
                    second.bounds = bounds(56.0, 20.0, 40.0, 40.0);
                    first.corner_radii = Corners::all(ScaledPixels(16.0));
                    second.corner_radii = first.corner_radii;
                    first.corner_smoothing = if case_idx == 9 { 0.6 } else { 1.0 };
                    second.corner_smoothing = first.corner_smoothing;
                }
                _else => {}
            }

            for (source_idx, quad) in [first, second].into_iter().enumerate() {
                let radius = if case_idx == 0 {
                    0.0
                } else {
                    source_idx as f32 * 16.0
                };

                let member = register(&mut scene, quad, None, radius);
                scene.paint_liquid_phase(member, LiquidPhase::Fill);
                scene.paint_liquid_phase(member, LiquidPhase::Border);
            }

            if case_idx == 0 {
                paint(
                    &mut scene,
                    Quad {
                        bounds: bounds(40.0, 64.0, 30.0, 24.0),
                        ..first
                    },
                    None,
                );
            }

            let image = render(&mut renderer, &mut scene);
            let prepared = PreparedLiquidComponent::new(&scene);

            if case_idx == 6 {
                oklab = Some(image.get_pixel(80, 40).0);
            }

            if case_idx == 11 {
                assert_ne!(Some(image.get_pixel(80, 40).0), oklab);
            }

            if std::env::var_os("GPUI_LIQUID_REVIEW").is_some() {
                review_image(&image, case_idx);
            }

            for y in (6..94).step_by(3) {
                for x in (6..118).step_by(3) {
                    let position = vec2f(x as f32 + 0.5, y as f32 + 0.5);
                    let mut sample = prepared.sample(0, position);

                    for channel_idx in 0usize..4 {
                        assert!(sample.fill[channel_idx].is_finite());
                        assert!(sample.border[channel_idx].is_finite());
                    }

                    if sample.active_count > 1 {
                        let horizontal = prepared.fields(
                            0,
                            position + vec2f(if x % 2 == 0 { 1.0 } else { -1.0 }, 0.0),
                        );
                        let vertical = prepared.fields(
                            0,
                            position + vec2f(0.0, if y % 2 == 0 { 1.0 } else { -1.0 }),
                        );
                        let derivative =
                            abs(horizontal - sample.fields) + abs(vertical - sample.fields);
                        sample.coverage = contour_coverages(
                            sample.fields.x,
                            sample.fields.y,
                            derivative.x,
                            derivative.y,
                        );
                    }

                    let mut expected = vec4f(0.0, 0.0, 0.0, 1.0);

                    for draw in &scene.liquid_draws {
                        let record = &scene.liquid_records[draw.record_index];
                        let above = sample.phase(record, position, draw.phase);
                        expected = above + expected * (1.0 - above.w);
                    }

                    let expected = [expected.x, expected.y, expected.z, expected.w]
                        .map(|value| (value.clamp(0.0, 1.0) * 255.0).round() as u8);
                    let actual = image.get_pixel(x, y).0;
                    let tolerance = 4;

                    if actual
                        .iter()
                        .zip(expected)
                        .any(|(actual, expected)| actual.abs_diff(expected) > tolerance)
                    {
                        let path = review_image(&image, case_idx);
                        panic!(
                            "case {case_idx} at ({x}, {y}): {actual:?} versus {expected:?}, fields {:?}, coverage {:?}, border {:?}, review {}",
                            sample.fields,
                            sample.coverage,
                            sample.border,
                            path.display()
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn moving_cached_sources_join_separate_and_cross_ownership_without_alpha_popping() {
        let mut renderer = WgpuHeadlessRenderer::new().unwrap();
        let mut cached = Scene::default();
        paint(&mut cached, source(30.0), None);
        cached.finish();
        let mut previous: Option<u8> = None;
        let mut maximum = 0;
        let mut minimum = 255;

        for frame_idx in 0..2401 {
            let x = 92.0 - frame_idx as f32 * 0.02;
            let mut scene = Scene::default();
            scene.replay(0..cached.len(), &cached);
            paint(&mut scene, source(x), None);
            let image = render(&mut renderer, &mut scene);
            let pixel = image.get_pixel(69, 40).0;
            maximum = maximum.max(pixel[0]);
            minimum = minimum.min(pixel[0]);

            if let Some(previous) = previous {
                assert!(
                    pixel[0].abs_diff(previous) <= 20,
                    "frame {frame_idx}: {previous} -> {pixel:?}"
                );
            }

            assert!(
                pixel[0] <= 130,
                "repeated alpha at frame {frame_idx}: {pixel:?}"
            );
            previous = Some(pixel[0]);
        }

        assert_eq!(minimum, 0);
        assert!((maximum as i32 - 128).abs() <= 2);
    }

    #[test]
    fn clip_fades_overlay_domains_and_dashed_fallback_preserve_ordinary_output() {
        let mut renderer = WgpuHeadlessRenderer::new().unwrap();
        let mut clipped = Scene::default();
        let mut quad = source(-24.0);
        quad.content_mask.bounds = bounds(10.0, 0.0, 90.0, 100.0);
        quad.content_mask.fade_out.left = ScaledPixels(8.0);
        paint(&mut clipped, quad, None);
        paint(
            &mut clipped,
            Quad {
                bounds: bounds(14.0, 20.0, 30.0, 40.0),
                ..quad
            },
            None,
        );
        let image = render(&mut renderer, &mut clipped);
        assert_eq!(image.get_pixel(9, 40).0, [0, 0, 0, 255]);
        assert!(image.get_pixel(11, 40).0[0] > 0);
        assert!(image.get_pixel(11, 40).0[0] < image.get_pixel(18, 40).0[0]);

        let mut overlay = Scene::default();
        paint(&mut overlay, source(20.0), None);
        overlay.advance_liquid_overlay_epoch();
        paint(&mut overlay, source(25.0), None);
        let image = render(&mut renderer, &mut overlay);
        assert!((image.get_pixel(35, 40).0[0] as i32 - 192).abs() <= 2);

        let mut fallback = Scene::default();
        let dashed = Quad {
            border_style: BorderStyle::Dashed,
            border_widths: Edges::all(ScaledPixels(2.0)),
            border_color: gpui::white().into(),
            border_dashed_length: 3.0,
            border_dashed_gap: 2.0,
            ..source(20.0)
        };
        paint(&mut fallback, dashed, None);
        paint(&mut fallback, source(60.0), None);
        fallback.finish();
        assert!(fallback.liquid_components[0].fallback);
        let expected = render(&mut renderer, &mut ordinary_scene(&fallback));
        let actual = renderer.render_scene_to_image(&fallback, TARGET).unwrap();
        assert_eq!(actual, expected);
        assert_eq!(actual.get_pixel(55, 40).0, [0, 0, 0, 255]);
    }

    #[test]
    fn nested_fill_and_content_survive_while_contained_borders_disappear() {
        let mut renderer = WgpuHeadlessRenderer::new().unwrap();
        let mut scene = Scene::default();
        let parent = register(
            &mut scene,
            Quad {
                bounds: bounds(20.0, 10.0, 90.0, 75.0),
                background: gpui::rgb(0xff0000).into(),
                border_widths: Edges::all(ScaledPixels(2.0)),
                border_color: gpui::white().into(),
                ..source(20.0)
            },
            None,
            0.0,
        );

        scene.paint_liquid_phase(parent, LiquidPhase::Fill);

        let child = register(
            &mut scene,
            Quad {
                bounds: bounds(40.0, 25.0, 45.0, 40.0),
                background: gpui::rgb(0x0000ff).into(),
                border_widths: Edges::all(ScaledPixels(8.0)),
                border_color: gpui::rgb(0x00ff00).into(),
                ..source(20.0)
            },
            None,
            0.0,
        );

        scene.paint_liquid_phase(child, LiquidPhase::Fill);
        scene.insert_primitive(Quad {
            bounds: bounds(52.0, 36.0, 10.0, 10.0),
            background: gpui::white().into(),
            ..source(20.0)
        });
        scene.paint_liquid_phase(child, LiquidPhase::Border);
        scene.paint_liquid_phase(parent, LiquidPhase::Border);
        let image = render(&mut renderer, &mut scene);
        assert_eq!(image.get_pixel(57, 41).0, [255, 255, 255, 255]);
        assert_eq!(image.get_pixel(42, 40).0, [0, 0, 255, 255]);
        assert_eq!(image.get_pixel(30, 40).0, [255, 0, 0, 255]);
    }

    #[test]
    fn ordinary_frames_stay_identical_after_liquid_clear_and_reuse() {
        let mut renderer = WgpuHeadlessRenderer::new().unwrap();
        let mut ordinary = Scene::default();
        ordinary.insert_primitive(Quad {
            bounds: full_mask().bounds,
            content_mask: full_mask(),
            background: checkerboard(gpui::rgb(0x445588), 8.0),
            ..Default::default()
        });
        ordinary.insert_primitive(source(20.0));
        let expected = render(&mut renderer, &mut ordinary);
        let mut reused = Scene::default();
        paint(&mut reused, source(20.0), None);
        paint(&mut reused, source(60.0), None);
        render(&mut renderer, &mut reused);
        reused.clear();
        reused.replay(0..ordinary.len(), &ordinary);
        reused.finish();
        assert_eq!(reused.render_plan().requirements().liquid_record_count, 0);
        let actual = renderer.render_scene_to_image(&reused, TARGET).unwrap();
        assert_eq!(actual, expected);
    }
}
