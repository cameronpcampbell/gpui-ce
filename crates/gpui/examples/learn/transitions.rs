//! Transition Example
//!
//! This example uses a keyed transition to move a button to a random position.

#[path = "../shared/prelude.rs"]
mod example_prelude;

use std::time::Duration;

use gpui::{
    AnyElement, App, AppContext, Bounds, Context, ElementId, Pixels, Point, Window, WindowBounds,
    WindowOptions, actions, div, ease_in_out, point, prelude::*, px, rgb, size,
};
use rand::Rng;
use smallvec::SmallVec;

actions!(app, [Quit]);

const BUTTON_WIDTH: f32 = 120.;
const BUTTON_HEIGHT: f32 = 44.;
const WINDOW_PADDING: f32 = 24.;

fn centered_position(window: &Window) -> Point<Pixels> {
    let viewport = window.viewport_size();

    point(
        px(((f32::from(viewport.width) - BUTTON_WIDTH) / 2.).max(0.)),
        px(((f32::from(viewport.height) - BUTTON_HEIGHT) / 2.).max(0.)),
    )
}

fn random_position(window: &Window) -> Point<Pixels> {
    let viewport = window.viewport_size();
    let max_x = (f32::from(viewport.width) - BUTTON_WIDTH - WINDOW_PADDING).max(WINDOW_PADDING);
    let max_y = (f32::from(viewport.height) - BUTTON_HEIGHT - WINDOW_PADDING).max(WINDOW_PADDING);
    let mut rng = rand::rng();

    point(
        px(rng.random_range(WINDOW_PADDING..=max_x)),
        px(rng.random_range(WINDOW_PADDING..=max_y)),
    )
}

#[derive(IntoElement)]
struct Button {
    id: ElementId,
    children: SmallVec<[AnyElement; 2]>,
}

impl Button {
    fn new(id: impl Into<ElementId>) -> Self {
        Self {
            id: id.into(),
            children: SmallVec::new(),
        }
    }
}

impl ParentElement for Button {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements);
    }
}

impl RenderOnce for Button {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let position_transition = window
            .use_keyed_transition(
                (self.id.clone(), "position"),
                cx,
                Duration::from_millis(400),
                |window, _cx| centered_position(window),
            )
            .with_easing(ease_in_out);
        let position = *position_transition.evaluate(window, cx);

        div()
            .id(self.id)
            .absolute()
            .left(position.x)
            .top(position.y)
            .w(px(BUTTON_WIDTH))
            .h(px(BUTTON_HEIGHT))
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .rounded(px(100.))
            .bg(rgb(0x663399))
            .text_color(rgb(0xffffff))
            .children(self.children)
            .on_click(move |_, window, cx| {
                let target = random_position(window);

                position_transition.update(cx, |position, cx| {
                    *position = target;
                    cx.notify();
                });
            })
    }
}

struct TransitionExample;

impl Render for TransitionExample {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .relative()
            .size_full()
            .overflow_hidden()
            .bg(rgb(0x110F15))
            .child(Button::new("btn").child("Click me!"))
    }
}

fn main() {
    gpui_platform::application().run(|cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(500.), px(650.)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            |_, cx| cx.new(|_| TransitionExample),
        )
        .expect("Failed to open window");

        example_prelude::init_example(cx, "Transition");
    });
}
