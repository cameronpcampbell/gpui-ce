#[path = "../shared/prelude.rs"]
mod example_prelude;

use std::time::Duration;

use gpui::{
    App, Background, Bounds, Context, Div, MouseButton, MouseDownEvent, MouseMoveEvent, Pixels,
    Point, Stateful, Window, WindowBounds, WindowOptions, checkerboard, div, ease_in_out, hsla,
    point, prelude::*, px, rgb, rgba, size,
};

struct LiquidExample {
    draggable_positions: [Point<Pixels>; 2],
    drag: Option<(usize, Point<Pixels>, Point<Pixels>)>,
    group: usize,
    blend_radius: f32,
    smoothing: f32,
    isolated: bool,
    separate_child: bool,
    joined: bool,
}

impl Default for LiquidExample {
    fn default() -> Self {
        Self {
            draggable_positions: [point(px(220.), px(35.)), point(px(384.), px(85.))],
            drag: None,
            group: 0,
            blend_radius: 8.,
            smoothing: 0.6,
            isolated: false,
            separate_child: false,
            joined: true,
        }
    }
}

impl LiquidExample {
    fn decoration(&self, element: Div, separate: bool) -> Div {
        let element = element
            .liquid()
            .liquid_blend_radius(px(self.blend_radius))
            .rounded(px(32.))
            .rounded_smoothing(self.smoothing)
            .border_color(rgba(0xffffffdd));

        if separate {
            return element.liquid_group("separate");
        }

        if self.group == 1 {
            return element.liquid_group("named");
        }

        element
    }

    fn animation_offset(source_idx: usize, progress: f32) -> Point<Pixels> {
        let distance = if source_idx == 1 { 1. - progress } else { 0. };

        point(px(116. * distance), px(20. * distance))
    }

    fn move_source(&mut self, event: &MouseMoveEvent, progress: f32) {
        if event.pressed_button != Some(MouseButton::Left) {
            self.drag = None;

            return;
        }

        let Some((source_idx, mouse, origin)) = self.drag else {
            return;
        };
        let offset = Self::animation_offset(source_idx, progress);
        let position = origin + event.position - mouse;

        self.draggable_positions[source_idx] =
            position.clamp(&point(px(0.), px(0.)), &point(px(680.), px(110.))) - offset;
    }

    fn source(&self, source_idx: usize, progress: f32, cx: &mut Context<Self>) -> Stateful<Div> {
        let (width, height, color): (f32, f32, Background) = if source_idx == 0 {
            (155., 125., rgba(0x4488ff99).into())
        } else {
            (
                125. + 30. * progress,
                85. + 40. * progress,
                hsla(0.06 + 0.65 * (1. - progress), 0.8, 0.6, 0.6).into(),
            )
        };

        let separate = source_idx == 1 && self.group == 2;
        let element = self.decoration(div(), separate);
        let offset = Self::animation_offset(source_idx, progress);

        let position = (self.draggable_positions[source_idx] + offset)
            .clamp(&point(px(0.), px(0.)), &point(px(680.), px(110.)));

        element
            .id(("liquid-source", source_idx))
            .absolute()
            .left(position.x)
            .top(position.y)
            .w(px(width))
            .h(px(height))
            .border(px(if source_idx == 0 { 1. } else { 8. }))
            .bg(color)
            .when(source_idx == 1 && self.isolated, |element| {
                element.opacity(0.7)
            })
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseDownEvent, _window, cx| {
                    this.drag = Some((source_idx, event.position, position));
                    cx.notify();
                }),
            )
            .child(if source_idx == 0 {
                "Drag / 1px"
            } else {
                "Drag / 8px"
            })
    }

    fn controls(&self, cx: &mut Context<Self>) -> Div {
        let labels = [
            format!("Group: {}", ["default", "named", "separate"][self.group]),
            format!("Color radius: {}px", self.blend_radius),
            format!("Smoothing: {:.1}", self.smoothing),
            format!("Opacity target: {}", self.isolated),
            format!("Separate child: {}", self.separate_child),
            format!("Animate: {}", if self.joined { "separate" } else { "join" }),
            String::from("Reset"),
        ];

        div()
            .flex()
            .flex_wrap()
            .gap_2()
            .children(labels.into_iter().enumerate().map(|(control_idx, label)| {
                div()
                    .id(("control", control_idx))
                    .px_3()
                    .py_2()
                    .rounded_md()
                    .bg(rgb(0x34343c))
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _event, _window, cx| {
                        match control_idx {
                            0 => this.group = (this.group + 1) % 3,
                            1 => this.blend_radius = (this.blend_radius + 8.) % 40.,
                            2 => {
                                this.smoothing = if this.smoothing < 0.5 {
                                    0.6
                                } else if this.smoothing < 1. {
                                    1.
                                } else {
                                    0.
                                };
                            }
                            3 => this.isolated = !this.isolated,
                            4 => this.separate_child = !this.separate_child,
                            5 => this.joined = !this.joined,
                            _else => *this = Self::default(),
                        }

                        cx.notify();
                    }))
                    .child(label)
            }))
    }
}

impl Render for LiquidExample {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let transition = window
            .use_keyed_transition("liquid-join", cx, Duration::from_millis(600), |_, _| 1.)
            .with_easing(ease_in_out);

        transition.update(cx, |goal, _cx| *goal = if self.joined { 1. } else { 0. });
        let progress = *transition.evaluate(window, cx);

        let nested = self
            .decoration(div(), false)
            .relative()
            .w(px(350.))
            .h(px(130.))
            .border_4()
            .bg(rgba(0x4488ff99))
            .p_4()
            .child("Parent content")
            .child(
                self.decoration(div(), self.separate_child)
                    .absolute()
                    .left(px(210.))
                    .top(px(30.))
                    .w(px(180.))
                    .h(px(80.))
                    .border_8()
                    .bg(rgba(0xff884499))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child("Child content"),
            );

        div()
            .id("liquid-example")
            .size_full()
            .overflow_y_scroll()
            .bg(rgb(0x18181b))
            .text_color(rgb(0xffffff))
            .p_6()
            .flex()
            .flex_col()
            .gap_4()
            .on_mouse_move(cx.listener(move |this, event, _window, cx| {
                if this.drag.is_some() {
                    this.move_source(event, progress);
                    cx.notify();
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _event, _window, cx| {
                    this.drag = None;
                    cx.notify();
                }),
            )
            .child(self.controls(cx))
            .child(
                div()
                    .relative()
                    .w(px(840.))
                    .h(px(250.))
                    .flex_shrink_0()
                    .bg(checkerboard(rgb(0x303038), 12.))
                    .child(self.source(0, progress, cx))
                    .child(self.source(1, progress, cx)),
            )
            .child(
                div()
                    .w(px(840.))
                    .h(px(200.))
                    .flex_shrink_0()
                    .p_6()
                    .bg(checkerboard(rgb(0x303038), 12.))
                    .child(nested),
            )
    }
}

fn main() {
    gpui_platform::application().run(|cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(940.), px(650.)), cx);
        cx.open_window(
            WindowOptions::new().window_bounds(Some(WindowBounds::Windowed(bounds))),
            |_window, cx| cx.new(|_cx| LiquidExample::default()),
        )
        .expect("Failed to open window");

        example_prelude::init_example(cx, "Liquid");
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dragging_preserves_offsets_clamps_independently_and_stops_on_release() {
        for progress in [0., 1.] {
            let mut example = LiquidExample::default();
            let mut expected = example.draggable_positions;

            for source_idx in 0..2 {
                let offset = LiquidExample::animation_offset(source_idx, progress);

                example.drag = Some((
                    source_idx,
                    point(px(400.), px(100.)),
                    expected[source_idx] + offset,
                ));

                let mut event = MouseMoveEvent {
                    position: point(px(410.), px(105.)),
                    pressed_button: Some(MouseButton::Left),
                    ..Default::default()
                };

                example.move_source(&event, progress);
                expected[source_idx] += point(px(10.), px(5.));
                assert_eq!(example.draggable_positions, expected);
                event.position = point(px(2000.), px(-100.));
                example.move_source(&event, progress);
                expected[source_idx] = point(px(680.), px(0.)) - offset;
                assert_eq!(example.draggable_positions, expected);
                event.pressed_button = None;
                example.move_source(&event, progress);
                assert!(example.drag.is_none());
                assert_eq!(example.draggable_positions, expected);
            }
        }
    }
}
