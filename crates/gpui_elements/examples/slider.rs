//! Run with `cargo run -p gpui_ce_elements --example slider`.

use gpui::{
    App, Bounds, Context, FocusHandle, KeyBinding, SharedString, Window, WindowBounds,
    WindowOptions, actions, div, prelude::*, px, rgb, size,
};
use gpui_ce_elements::slider::{
    BaseSlider, BaseSliderControl, BaseSliderIndicator, BaseSliderLabel, BaseSliderThumb,
    BaseSliderTrack, BaseSliderValue, SliderChange, ThumbCollisionBehavior,
};

actions!(slider_example, [FocusNext, FocusPrevious]);

struct SliderExample {
    focus_handle: FocusHandle,
    volume: f32,
    range: [f32; 2],
    last_committed: SharedString,
}

impl Render for SliderExample {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("slider-example")
            .track_focus(&self.focus_handle)
            .tab_group()
            .on_action(|_: &FocusNext, window, cx| window.focus_next(cx))
            .on_action(|_: &FocusPrevious, window, cx| window.focus_prev(cx))
            .size_full()
            .flex()
            .flex_col()
            .gap_8()
            .p_6()
            .bg(rgb(0x18181b))
            .text_color(rgb(0xfafafa))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child("Click the track or drag a thumb to change its value.")
                    .child("Tab / Shift+Tab to focus. Arrow keys, Home, End, and Page Up / Down to adjust."),
            )
            .child(
                slider("volume", "Volume", &[self.volume])
                    .on_value_change(cx.listener(
                        |this, change: &SliderChange, _window, cx| {
                            this.volume = change.value.as_slice()[0];
                            cx.notify();
                        },
                    ))
                    .on_value_committed(cx.listener(
                        |this, change: &SliderChange, _window, cx| {
                            this.last_committed =
                                format!("Volume {:.0}%", change.value.as_slice()[0]).into();
                            cx.notify();
                        },
                    )),
            )
            .child(
                slider("range", "Selected range", &self.range)
                    .step(5.)
                    .min_steps_between_values(2)
                    .thumb_collision_behavior(ThumbCollisionBehavior::Clamp)
                    .on_value_change(cx.listener(
                        |this, change: &SliderChange, _window, cx| {
                            this.range.copy_from_slice(change.value.as_slice());
                            cx.notify();
                        },
                    ))
                    .on_value_committed(cx.listener(
                        |this, change: &SliderChange, _window, cx| {
                            let values = change.value.as_slice();
                            this.last_committed =
                                format!("Range {:.0}% to {:.0}%", values[0], values[1]).into();
                            cx.notify();
                        },
                    )),
            )
            .child(
                div()
                    .text_color(rgb(0xa1a1aa))
                    .child(format!("Last committed: {}", self.last_committed)),
            )
    }
}

fn slider(element_id: &'static str, label: &'static str, values: &[f32]) -> BaseSlider {
    let thumb_size = px(20.);
    let track_height = px(6.);
    let track_inset = (thumb_size - track_height) / 2.;

    // Match the fill edges to the centers of the edge-aligned thumbs.
    let start_margin = if values.len() == 1 {
        px(0.)
    } else {
        thumb_size * (0.5 - values[0] / 100.)
    };
    let end_margin = thumb_size * (values[values.len() - 1] / 100. - 0.5);

    let start_radius = if values.len() == 1 {
        track_height / 2.
    } else {
        px(0.)
    };

    let mut track = BaseSliderTrack::new(format!("{element_id}-track"))
        .size_full()
        .h(track_height)
        .top(track_inset)
        .rounded_full()
        .bg(rgb(0x3f3f46))
        .indicator(
            BaseSliderIndicator::new(format!("{element_id}-indicator"))
                .ml(start_margin)
                .mr(end_margin)
                .rounded_l(start_radius)
                .bg(rgb(0x3b82f6)),
        );

    for thumb_idx in 0..values.len() {
        let thumb_label = if values.len() == 1 {
            label.to_string()
        } else if thumb_idx == 0 {
            format!("{label}, lower limit")
        } else {
            format!("{label}, upper limit")
        };

        track = track.thumb(
            BaseSliderThumb::new(format!("{element_id}-thumb-{thumb_idx}"))
                .aria_label(thumb_label)
                .size(thumb_size)
                .mt(-track_inset)
                .rounded_full()
                .bg(rgb(0x3b82f6))
                .p(px(2.))
                .child(div().size_full().rounded_full().bg(rgb(0xfafafa)))
                .cursor_pointer()
                .focus_visible(|style| style.bg(rgb(0xfbbf24))),
        );
    }

    BaseSlider::new(element_id)
        .values(values.iter().copied())
        .min(0.)
        .max(100.)
        .step(1.)
        .large_step(10.)
        .value_text_formatter(|value| format!("{value:.0}%"))
        .w_full()
        .flex()
        .flex_col()
        .gap_3()
        .label(BaseSliderLabel::new(format!("{element_id}-label")).text(label))
        .value_display(
            BaseSliderValue::new(format!("{element_id}-value")).text_color(rgb(0xa1a1aa)),
        )
        .control(
            BaseSliderControl::new(format!("{element_id}-control"))
                .w_full()
                .h(thumb_size)
                .cursor_pointer()
                .track(track),
        )
}

fn main() {
    gpui_platform::application().run(|cx: &mut App| {
        cx.bind_keys([
            KeyBinding::new("tab", FocusNext, None),
            KeyBinding::new("shift-tab", FocusPrevious, None),
        ]);

        let bounds = Bounds::centered(None, size(px(660.), px(420.)), cx);

        cx.open_window(
            WindowOptions::new().window_bounds(Some(WindowBounds::Windowed(bounds))),
            |window, cx| {
                cx.new(|cx| {
                    let focus_handle = cx.focus_handle();
                    focus_handle.focus(window, cx);

                    SliderExample {
                        focus_handle,
                        volume: 35.,
                        range: [25., 75.],
                        last_committed: "No changes yet".into(),
                    }
                })
            },
        )
        .expect("Failed to open the slider example window");
        cx.activate(true);
    });
}
