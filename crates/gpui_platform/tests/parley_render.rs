#[cfg(target_os = "macos")]
fn main() {
    use gpui::{
        AppContext as _, Context, IntoElement, ParentElement as _, Render, Styled as _,
        VisualTestAppContext, Window, div, px, rgb, white,
    };

    if std::env::var_os("GPUI_RUN_RENDERING_TESTS").is_none() {
        return;
    }

    struct ParleyRenderingFixture;

    impl Render for ParleyRenderingFixture {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .size_full()
                .relative()
                .bg(rgb(0x101418))
                .text_color(white())
                .text_size(px(28.0))
                .p(px(32.0))
                .child(div().child("Parley office cafe\u{301} العربية אבג"))
                .child(div().child("日本語 ไทย 👩🏽‍💻 🇬🇧 1️⃣"))
                .child(
                    div()
                        .w(px(260.0))
                        .child("wrapped text one two three four five six seven eight"),
                )
                .child(
                    div()
                        .absolute()
                        .left(px(32.0))
                        .bottom(px(-20.0))
                        .h(px(40.0))
                        .line_height(px(40.0))
                        .child("MMMM clipped at the bottom edge"),
                )
        }
    }

    let mut cx = VisualTestAppContext::new(gpui_ce_platform::current_platform(false));
    let window = cx
        .open_offscreen_window_default(|_, cx| cx.new(|_| ParleyRenderingFixture))
        .expect("failed to create offscreen text window");
    let window = window.into();
    cx.run_until_parked();

    let primitive_counts = cx
        .update_window(window, |_, window, _| window.rendered_primitive_counts())
        .expect("failed to inspect rendered text");
    let (_, monochrome, subpixel, polychrome) = primitive_counts;
    assert!(
        monochrome + subpixel > 0,
        "Parley scene contained no text glyph sprites"
    );
    assert!(
        polychrome > 0,
        "Parley scene contained no color emoji sprites"
    );

    // This is a Parley rendering invariant, not a pixel comparison with Cosmic.
    let image = cx
        .capture_screenshot(window)
        .expect("failed to capture rendered text");
    let background = *image.get_pixel(0, 0);
    let changed_pixels = image.pixels().filter(|pixel| **pixel != background).count();
    assert!(
        changed_pixels > 500,
        "Parley scene had primitive counts {primitive_counts:?}, but painted only {changed_pixels} non-background pixels in a {}x{} image with background {background:?}",
        image.width(),
        image.height(),
    );
    let edge_top = image.height().saturating_sub(20);
    let clipped_line_pixels = image
        .enumerate_pixels()
        .filter(|(_, y, pixel)| *y >= edge_top && **pixel != background)
        .count();
    assert!(
        clipped_line_pixels > 20,
        "Parley painted no visible glyph pixels for the line clipped by the bottom window edge"
    );
}

#[cfg(not(target_os = "macos"))]
fn main() {}
