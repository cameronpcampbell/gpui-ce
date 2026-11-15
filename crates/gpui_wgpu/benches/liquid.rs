use gpui::{
    Bounds, ContentMask, Corners, DevicePixels, Edges, LiquidPhase, Quad, ScaledLiquidBox,
    ScaledPixels, Scene, checkerboard, pattern_slash, point, rgba, size,
};
use gpui_ce_wgpu::{WgpuContext, WgpuDeviceRequirements, WgpuHeadlessRenderer, wgpu};
use gpui_render::liquid::LiquidTable;
use std::{
    env,
    hint::black_box,
    time::{Duration, Instant},
};

#[derive(Clone, Copy, Debug)]
enum Layout {
    Nearby,
    Distant,
    Chain,
    Dense,
    Cached,
    Example,
}

fn sources(count: usize, scale: f32, layout: Layout) -> Vec<Quad> {
    (0..count)
        .map(|source_idx| {
            let (x, y) = match layout {
                Layout::Distant => (
                    (source_idx % 16) as f32 * 78.,
                    (source_idx / 16) as f32 * 78.,
                ),
                Layout::Chain => (source_idx as f32 * 9., 140.),
                Layout::Dense => ((source_idx % 8) as f32 * 2., (source_idx / 8) as f32 * 2.),
                _else => (
                    (source_idx % 16) as f32 * 24.,
                    (source_idx / 16) as f32 * 24.,
                ),
            };
            let (x, y, width, height, radius, border) = if matches!(layout, Layout::Example) {
                [
                    (180., 55., 155., 125., 32., 1.),
                    (344., 65., 155., 125., 32., 8.),
                    (24., 350., 350., 130., 32., 4.),
                    (234., 380., 180., 80., 32., 8.),
                ][source_idx]
            } else {
                (x, y, 20., 18., 4., (1 + source_idx % 3) as f32)
            };
            let background = if matches!(layout, Layout::Dense) {
                if source_idx % 2 == 0 {
                    checkerboard(rgba(0x4488ff80), 4. * scale)
                } else {
                    pattern_slash(rgba(0xff884480), scale, 4. * scale)
                }
            } else {
                rgba(0x4488ff80).into()
            };

            Quad {
                bounds: Bounds::new(
                    point(
                        ScaledPixels((x + 30.) * scale),
                        ScaledPixels((y + 30.) * scale),
                    ),
                    size(ScaledPixels(width * scale), ScaledPixels(height * scale)),
                ),
                content_mask: ContentMask {
                    bounds: Bounds::new(
                        point(ScaledPixels(0.), ScaledPixels(0.)),
                        size(ScaledPixels(1280. * scale), ScaledPixels(720. * scale)),
                    ),
                    ..Default::default()
                },
                background,
                border_color: rgba(0xffffffb0).into(),
                border_widths: Edges::all(ScaledPixels(border * scale)),
                corner_radii: Corners::all(ScaledPixels(radius * scale)),
                corner_smoothing: if matches!(layout, Layout::Dense | Layout::Example) {
                    0.6
                } else {
                    0.
                },
                ..Default::default()
            }
        })
        .collect()
}

fn paint(scene: &mut Scene, quad: Quad, scale: f32, liquid: bool) {
    if !liquid {
        scene.insert_primitive(Quad {
            border_widths: Default::default(),
            border_color: quad.border_color.opacity(0.),
            ..quad
        });
        scene.insert_primitive(Quad {
            background: quad.background.opacity(0.),
            ..quad
        });

        return;
    }

    let member = scene
        .register_liquid_box(ScaledLiquidBox {
            quad,
            group: None,
            blend_radius: ScaledPixels(8. * scale),
            reach: ScaledPixels(24. * scale),
        })
        .unwrap();
    scene.paint_liquid_phase(member, LiquidPhase::Fill);
    scene.paint_liquid_phase(member, LiquidPhase::Border);
}

fn main() -> anyhow::Result<()> {
    let smoke = env::args().any(|argument| argument == "--test");
    let samples = if smoke {
        1
    } else {
        env::var("GPUI_LIQUID_SAMPLES")
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or(5usize)
            .max(1)
    };
    let context = WgpuContext::new_headless(Some(&WgpuDeviceRequirements {
        features: wgpu::Features::TIMESTAMP_QUERY,
        ..Default::default()
    }))?;
    let adapter = context.adapter.get_info();
    eprintln!(
        "machine={} arch={} os={} adapter={} backend={:?} driver={} {} samples={samples}",
        env::var("HOSTNAME").unwrap_or_default(),
        env::consts::ARCH,
        env::consts::OS,
        adapter.name,
        adapter.backend,
        adapter.driver,
        adapter.driver_info
    );
    let mut renderer = WgpuHeadlessRenderer::with_context(&context)?;
    println!(
        "layout,count,scale,liquid,components,draws,source_bytes,raster_pixels,component_visits,record_us,compile_us,cold_prepare_us,prepare_us,frame_wait_us,gpu_us"
    );
    let workloads = [
        (Layout::Nearby, 0),
        (Layout::Nearby, 2),
        (Layout::Nearby, 8),
        (Layout::Nearby, 32),
        (Layout::Nearby, 128),
        (Layout::Distant, 128),
        (Layout::Chain, 128),
        (Layout::Dense, 32),
        (Layout::Cached, 32),
        (Layout::Example, 4),
    ];

    for scale in [1., 2.] {
        for (layout, count) in workloads {
            if env::var("GPUI_LIQUID_LAYOUT")
                .is_ok_and(|selected| selected != format!("{layout:?}"))
            {
                continue;
            }

            let quads = sources(count, scale, layout);
            let target = size(
                DevicePixels((1280. * scale) as i32),
                DevicePixels((720. * scale) as i32),
            );

            for liquid in [false, true] {
                let mut cached = Scene::default();
                let static_count = if matches!(layout, Layout::Cached) {
                    count.saturating_sub(1)
                } else {
                    0
                };

                for quad in &quads[..static_count] {
                    paint(&mut cached, *quad, scale, liquid);
                }

                cached.finish();
                let mut scene = Scene::default();
                let mut table = LiquidTable::default();

                let mut totals = [0.; 5];
                let mut cold_prepare = Duration::ZERO;

                // The first frame warms pipelines, contour preparation, and retained buffers.
                for sample_idx in 0..=samples {
                    scene.clear();
                    let start = Instant::now();

                    if static_count > 0 {
                        scene.replay(0..cached.len(), &cached);
                    }

                    for quad in &quads[static_count..] {
                        let mut quad = *quad;

                        if static_count > 0 {
                            quad.bounds.origin.x += ScaledPixels(sample_idx as f32 * 0.25 * scale);
                        }

                        paint(&mut scene, quad, scale, liquid);
                    }

                    let record = start.elapsed();
                    let start = Instant::now();
                    scene.finish();
                    let compile = start.elapsed();
                    let start = Instant::now();
                    table.prepare(&scene, 128 * 1024 * 1024);
                    let prepare = start.elapsed();

                    if sample_idx == 0 {
                        cold_prepare = prepare;
                    }

                    let start = Instant::now();
                    renderer.render_scene_and_wait(&scene, target)?;
                    let wait = start.elapsed();
                    let gpu = if count > 0 {
                        renderer.measure_gpu_time(&scene, target)?
                    } else {
                        Duration::ZERO
                    };

                    if sample_idx > 0 {
                        for (total, elapsed) in
                            totals.iter_mut().zip([record, compile, prepare, wait, gpu])
                        {
                            *total += elapsed.as_secs_f64() * 1e6 / samples as f64;
                        }
                    }
                }

                if liquid {
                    assert_eq!(scene.liquid_records.len(), count);
                    assert_eq!(scene.liquid_draws.len(), count * 2);
                    assert!((0..count).all(|source_idx| !table.falls_back(source_idx)));

                    if matches!(layout, Layout::Distant) {
                        assert_eq!(scene.liquid_components.len(), count);
                    }
                } else {
                    assert!(table.cells.is_empty());
                }

                let (mut raster_pixels, mut visits) = (0., 0.);

                for draw in &scene.liquid_draws {
                    let area =
                        f64::from(draw.bounds.size.width.0) * f64::from(draw.bounds.size.height.0);

                    raster_pixels += area;
                    visits +=
                        area * scene.liquid_components[draw.component_index].records.len() as f64;
                }

                let gpu_time = if count > 0 {
                    format!("{:.2}", totals[4])
                } else {
                    String::from("NA")
                };
                println!(
                    "{layout:?},{count},{scale},{liquid},{},{},{},{raster_pixels:.0},{visits:.0},{:.2},{:.2},{:.2},{:.2},{:.2},{gpu_time}",
                    scene.liquid_components.len(),
                    scene.render_plan().requirements().instance_batch_count,
                    table.byte_len(),
                    totals[0],
                    totals[1],
                    cold_prepare.as_secs_f64() * 1e6,
                    totals[2],
                    totals[3]
                );
                black_box(&table);
            }
        }
    }

    Ok(())
}
