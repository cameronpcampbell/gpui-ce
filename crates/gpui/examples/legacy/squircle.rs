use std::{fs, path::PathBuf};

use gpui::{
    App, AssetSource, Bounds, Context, Div, Fill, FontWeight, ObjectFit, SharedString, Window,
    WindowBounds, WindowOptions, div, img, prelude::*, px, rgb, size,
};

const IMAGE: &str = "image/black-cat-typing.gif";

struct Assets {
    base: PathBuf,
}

impl AssetSource for Assets {
    fn load(&self, path: &str) -> anyhow::Result<Option<std::borrow::Cow<'static, [u8]>>> {
        fs::read(self.base.join(path))
            .map(|data| Some(std::borrow::Cow::Owned(data)))
            .map_err(Into::into)
    }

    fn list(&self, path: &str) -> anyhow::Result<Vec<SharedString>> {
        fs::read_dir(self.base.join(path))
            .map(|entries| {
                entries
                    .filter_map(|entry| {
                        entry
                            .ok()
                            .and_then(|entry| entry.file_name().into_string().ok())
                            .map(SharedString::from)
                    })
                    .collect()
            })
            .map_err(Into::into)
    }
}

struct SquircleDemo;

impl Render for SquircleDemo {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .gap_4()
            .p_8()
            .bg(rgb(0xf0f0f0))
            .size_full()
            .children([
                div()
                    .text_xl()
                    .font_weight(FontWeight::BOLD)
                    .child("Figma Corner Smoothing"),
                div().flex().gap_4().children([
                    example_card("0", ".rounded_smoothing(0.0)", 0.0, rgb(0x3b82f6)),
                    example_card("0.3", ".rounded_smoothing(0.3)", 0.3, rgb(0x06b6d4)),
                    example_card("0.6", ".rounded_smoothing(0.6)", 0.6, rgb(0x8b5cf6)),
                    example_card("1.0", ".rounded_smoothing(1.0)", 1.0, rgb(0xec4899)),
                ]),
                div()
                    .text_lg()
                    .font_weight(FontWeight::BOLD)
                    .child("Smoothing Gradient"),
                div().flex().gap_2().children(
                    (0..=10)
                        .map(|i| {
                            let amount = i as f32 / 10.0;
                            gradient_example(amount, rgb(0x10b981))
                        })
                        .collect::<Vec<_>>(),
                ),
                div()
                    .text_lg()
                    .font_weight(FontWeight::BOLD)
                    .child("Images with Smoothed Corners"),
                div().flex().gap_4().children([
                    image_example("0", 0.0),
                    image_example("0.3", 0.3),
                    image_example("0.6", 0.6),
                    image_example("1.0", 1.0),
                ]),
                div()
                    .mt_4()
                    .p_4()
                    .rounded(px(8.0))
                    .bg(rgb(0x1f2937))
                    .text_sm()
                    .text_color(rgb(0xe5e7eb))
                    .font_family("monospace")
                    .child(
                        r#"// Usage Examples
div()
    .rounded_smoothing(0.6)  // Figma's documented iOS smoothing
    .rounded(px(40.0))
    .bg(blue())

div()
    .rounded_smoothing(1.0)  // Maximum smoothing
    .rounded(px(40.0))
    .bg(green())"#,
                    ),
            ])
    }
}

fn example_card(
    title: &'static str,
    description: &'static str,
    amount: f32,
    color: impl Clone + Into<Fill>,
) -> Div {
    div().flex().flex_col().gap_2().items_center().children([
        div()
            .w(px(150.0))
            .h(px(150.0))
            .rounded(px(40.0))
            .rounded_smoothing(amount)
            .bg(color)
            .shadow_lg(),
        div().font_weight(FontWeight::BOLD).child(title),
        div().text_sm().text_color(rgb(0x6b7280)).child(description),
    ])
}

fn gradient_example(amount: f32, color: impl Clone + Into<Fill>) -> Div {
    div()
        .w(px(60.0))
        .h(px(60.0))
        .rounded(px(15.0))
        .rounded_smoothing(amount)
        .bg(color)
        .flex()
        .items_center()
        .justify_center()
        .text_xs()
        .text_color(gpui::white())
        .child(format!("{:.1}", amount))
}

fn image_example(label: &'static str, amount: f32) -> Div {
    div()
        .flex()
        .flex_col()
        .gap_2()
        .items_center()
        .child(
            img(IMAGE)
                .w(px(150.0))
                .h(px(150.0))
                .rounded(px(40.0))
                .rounded_smoothing(amount)
                .object_fit(ObjectFit::Cover),
        )
        .child(div().text_sm().text_color(rgb(0x6b7280)).child(label))
}

fn main() {
    gpui_platform::application()
        .with_assets(Assets {
            base: PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("examples/legacy"),
        })
        .run(|cx: &mut App| {
            let bounds = Bounds::centered(None, size(px(800.0), px(600.0)), cx);
            cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    ..Default::default()
                },
                |_, cx| cx.new(|_| SquircleDemo),
            )
            .unwrap();
            cx.activate(true);
        });
}
