use std::rc::Rc;

use gpui::{
    AccessibleAction, AnyElement, App, Axis, Bounds, ClickEvent, DispatchPhase, Div, ElementId,
    Entity, FocusHandle, InteractiveElement, Interactivity, IntoElement, KeyDownEvent, Length,
    MouseButton, MouseMoveEvent, ParentElement, Pixels, Point, RenderOnce, Role, SharedString,
    Size, Stateful, StatefulInteractiveElement, StyleRefinement, Styled, Window, accesskit, canvas,
    div, prelude::FluentBuilder, px, relative,
};
use smallvec::SmallVec;

pub type OnValueChange = Rc<dyn Fn(&SliderChange, &mut Window, &mut App)>;
pub type OnValueCommitted = Rc<dyn Fn(&SliderChange, &mut Window, &mut App)>;
type ValueTextFormatter = Rc<dyn Fn(f32) -> SharedString>;

/// One or more values controlled by the application that renders a slider.
#[derive(Clone, Debug, PartialEq)]
pub struct SliderValue(SmallVec<[f32; 2]>);

impl SliderValue {
    /// Creates a slider value and panics if it is empty or contains a non-finite
    /// number.
    pub fn new(values: impl IntoIterator<Item = f32>) -> Self {
        let values = values.into_iter().collect::<SmallVec<[f32; 2]>>();

        assert!(!values.is_empty(), "a slider requires at least one value");
        assert!(
            values.iter().all(|value| value.is_finite()),
            "slider values must be finite"
        );

        Self(values)
    }

    pub fn as_slice(&self) -> &[f32] {
        &self.0
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl AsRef<[f32]> for SliderValue {
    fn as_ref(&self) -> &[f32] {
        self.as_slice()
    }
}

impl From<f32> for SliderValue {
    fn from(value: f32) -> Self {
        Self::new([value])
    }
}

impl<const COUNT: usize> From<[f32; COUNT]> for SliderValue {
    fn from(values: [f32; COUNT]) -> Self {
        Self::new(values)
    }
}

impl FromIterator<f32> for SliderValue {
    fn from_iter<Values: IntoIterator<Item = f32>>(values: Values) -> Self {
        Self::new(values)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ThumbCollisionBehavior {
    Push,
    Swap,
    Clamp,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ThumbAlignment {
    Center,
    Edge,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SliderChangeReason {
    TrackPress,
    Drag,
    Keyboard,
    Accessibility,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SliderChange {
    pub value: SliderValue,
    pub active_thumb_index: usize,
    pub reason: SliderChangeReason,
}

/// The root of an unstyled, controlled slider.
#[derive(IntoElement, Styled, ParentElement, StatefulInteractiveElement)]
pub struct BaseSlider {
    element_id: ElementId,
    base: Stateful<Div>,
    #[style]
    style: StyleRefinement,
    values: SliderValue,
    minimum: f32,
    maximum: f32,
    step: f32,
    large_step: f32,
    min_steps_between_values: usize,
    collision_behavior: ThumbCollisionBehavior,
    thumb_alignment: ThumbAlignment,
    orientation: Axis,
    disabled: bool,
    label: Option<BaseSliderLabel>,
    value_display: Option<BaseSliderValue>,
    control: Option<BaseSliderControl>,
    #[children]
    children: Vec<AnyElement>,
    on_value_change: Option<OnValueChange>,
    on_value_committed: Option<OnValueCommitted>,
    value_text_formatter: Option<ValueTextFormatter>,
}

impl InteractiveElement for BaseSlider {
    fn interactivity(&mut self) -> &mut Interactivity {
        self.base.interactivity()
    }
}

impl BaseSlider {
    pub fn new(element_id: impl Into<ElementId>) -> Self {
        let element_id = element_id.into();

        Self {
            element_id: element_id.clone(),
            base: div().id(element_id),
            style: StyleRefinement::default(),
            values: SliderValue::from(0.),
            minimum: 0.,
            maximum: 100.,
            step: 1.,
            large_step: 10.,
            min_steps_between_values: 0,
            collision_behavior: ThumbCollisionBehavior::Push,
            thumb_alignment: ThumbAlignment::Edge,
            orientation: Axis::Horizontal,
            disabled: false,
            label: None,
            value_display: None,
            control: None,
            children: Vec::new(),
            on_value_change: None,
            on_value_committed: None,
            value_text_formatter: None,
        }
    }

    /// Sets the application-controlled value of a single-thumb slider.
    pub fn value(mut self, value: f32) -> Self {
        self.values = SliderValue::from(value);

        self
    }

    /// Sets the application-controlled values. At least one finite value is
    /// required.
    pub fn values(mut self, values: impl IntoIterator<Item = f32>) -> Self {
        self.values = SliderValue::new(values);

        self
    }

    pub fn min(mut self, minimum: f32) -> Self {
        assert!(minimum.is_finite(), "the slider minimum must be finite");
        self.minimum = minimum;

        self
    }

    pub fn max(mut self, maximum: f32) -> Self {
        assert!(maximum.is_finite(), "the slider maximum must be finite");
        self.maximum = maximum;

        self
    }

    pub fn step(mut self, step: f32) -> Self {
        assert!(
            step.is_finite() && step > 0.,
            "the slider step must be finite and positive"
        );
        self.step = step;

        self
    }

    pub fn large_step(mut self, large_step: f32) -> Self {
        assert!(
            large_step.is_finite() && large_step > 0.,
            "the slider large step must be finite and positive"
        );
        self.large_step = large_step;

        self
    }

    pub fn min_steps_between_values(mut self, steps: usize) -> Self {
        self.min_steps_between_values = steps;

        self
    }

    pub fn thumb_collision_behavior(mut self, behavior: ThumbCollisionBehavior) -> Self {
        self.collision_behavior = behavior;

        self
    }

    pub fn thumb_alignment(mut self, alignment: ThumbAlignment) -> Self {
        self.thumb_alignment = alignment;

        self
    }

    pub fn orientation(mut self, orientation: Axis) -> Self {
        self.orientation = orientation;

        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;

        self
    }

    /// Adds the visible label and accessible-name fallback for the thumbs.
    pub fn label(mut self, label: BaseSliderLabel) -> Self {
        self.label = Some(label);

        self
    }

    /// Adds an element that displays the effective slider values.
    pub fn value_display(mut self, value_display: BaseSliderValue) -> Self {
        self.value_display = Some(value_display);

        self
    }

    /// Adds the clickable control and its track.
    pub fn control(mut self, control: BaseSliderControl) -> Self {
        self.control = Some(control);

        self
    }

    pub fn value_text_formatter<Text>(mut self, formatter: impl Fn(f32) -> Text + 'static) -> Self
    where
        Text: Into<SharedString>,
    {
        self.value_text_formatter = Some(Rc::new(move |value| formatter(value).into()));

        self
    }

    pub fn on_value_change(
        mut self,
        handler: impl Fn(&SliderChange, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_value_change = Some(Rc::new(handler));

        self
    }

    pub fn on_value_committed(
        mut self,
        handler: impl Fn(&SliderChange, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_value_committed = Some(Rc::new(handler));

        self
    }

    fn model(&self, value_count: usize) -> SliderModel {
        SliderModel {
            minimum: self.minimum,
            maximum: self.maximum,
            step: self.step,
            large_step: self.large_step,
            minimum_spacing: SliderModel::minimum_spacing(
                self.minimum,
                self.maximum,
                self.step,
                self.min_steps_between_values,
                value_count,
            ),
            orientation: self.orientation,
            alignment: self.thumb_alignment,
            collision_behavior: self.collision_behavior,
        }
    }
}

impl RenderOnce for BaseSlider {
    fn render(mut self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        assert!(
            self.maximum > self.minimum,
            "the slider maximum must be greater than its minimum"
        );

        let model = self.model(self.values.len());
        let values = model.normalized_values(&self.values);
        let interactive = !self.disabled && self.on_value_change.is_some();
        let interaction = SliderInteraction::new(
            &self.element_id,
            values.clone(),
            interactive,
            self.on_value_change.clone(),
            self.on_value_committed.clone(),
            window,
            cx,
        );
        let rendered_values = interaction.rendered_values(&values, cx);
        let context = SliderRenderContext {
            values: rendered_values,
            model,
            interaction,
            interactive,
            label: self
                .label
                .as_ref()
                .and_then(BaseSliderLabel::accessible_name),
            value_text_formatter: self.value_text_formatter.clone(),
        };
        let label = self.label.take().map(|label| label.render());
        let value_display = self
            .value_display
            .take()
            .map(|value_display| value_display.render(&context));
        let control = self
            .control
            .take()
            .expect("a slider requires a control")
            .render(&context, window, cx);

        self.base
            .relative()
            .refine_style(&self.style)
            .when_some(label, |this, label| this.child(label))
            .when_some(value_display, |this, value_display| {
                this.child(value_display)
            })
            .children(self.children)
            .child(control)
    }
}

#[derive(Clone, Copy)]
struct SliderModel {
    minimum: f32,
    maximum: f32,
    step: f32,
    large_step: f32,
    minimum_spacing: f32,
    orientation: Axis,
    alignment: ThumbAlignment,
    collision_behavior: ThumbCollisionBehavior,
}

impl SliderModel {
    fn minimum_spacing(
        minimum: f32,
        maximum: f32,
        step: f32,
        steps: usize,
        value_count: usize,
    ) -> f32 {
        if value_count <= 1 {
            return 0.;
        }

        (step * steps as f32).min((maximum - minimum) / (value_count - 1) as f32)
    }

    fn normalized_values(self, values: &SliderValue) -> SliderValue {
        let mut values = values
            .0
            .iter()
            .map(|value| value.clamp(self.minimum, self.maximum))
            .collect::<SmallVec<_>>();
        values.sort_by(f32::total_cmp);

        SliderValue(values)
    }

    fn snap(self, value: f32) -> f32 {
        let step_idx =
            ((f64::from(value) - f64::from(self.minimum)) / f64::from(self.step)).round();
        let snapped = f64::from(self.minimum) + step_idx * f64::from(self.step);

        (snapped as f32).clamp(self.minimum, self.maximum)
    }

    fn fraction(self, value: f32) -> f32 {
        ((value.clamp(self.minimum, self.maximum) - self.minimum) / (self.maximum - self.minimum))
            .clamp(0., 1.)
    }

    fn value_at_pointer(
        self,
        position: Point<Pixels>,
        control_bounds: Bounds<Pixels>,
        pointer_offset: Pixels,
        thumb_size: Size<Pixels>,
    ) -> f32 {
        let pointer = self.axis_position(position) - pointer_offset;
        let control_start = self.bounds_start(control_bounds);
        let control_length = self.size_along(control_bounds.size);
        let thumb_length = self.size_along(thumb_size);
        let (start, length) = match self.alignment {
            ThumbAlignment::Center => (control_start, control_length),
            ThumbAlignment::Edge => (
                control_start + thumb_length / 2.,
                (control_length - thumb_length).max(px(0.)),
            ),
        };
        let mut fraction = if length > px(0.) {
            (pointer - start) / length
        } else {
            0.
        };

        if self.orientation == Axis::Vertical {
            fraction = 1. - fraction;
        }

        let value = self.minimum + fraction.clamp(0., 1.) * (self.maximum - self.minimum);

        self.snap(value)
    }

    fn effective_bounds(self, values: &SliderValue, thumb_idx: usize) -> (f32, f32) {
        let minimum = if thumb_idx == 0 {
            self.minimum
        } else {
            values.0[thumb_idx - 1] + self.minimum_spacing
        };
        let maximum = if thumb_idx + 1 == values.len() {
            self.maximum
        } else {
            values.0[thumb_idx + 1] - self.minimum_spacing
        };

        (minimum.min(maximum), maximum.max(minimum))
    }

    fn keyboard_target(
        self,
        event: &KeyDownEvent,
        value: f32,
        minimum: f32,
        maximum: f32,
    ) -> Option<f32> {
        let increment = if event.keystroke.modifiers.shift {
            self.large_step
        } else {
            self.step
        };

        match event.keystroke.key.as_str() {
            "right" | "up" => Some((value + increment).min(maximum)),
            "left" | "down" => Some((value - increment).max(minimum)),
            "pageup" => Some((value + self.large_step).min(maximum)),
            "pagedown" => Some((value - self.large_step).max(minimum)),
            "home" => Some(minimum),
            "end" => Some(maximum),
            _ => None,
        }
    }

    fn thumb_position(
        self,
        control_size: Size<Pixels>,
        thumb_size: Size<Pixels>,
        value: f32,
    ) -> Point<Length> {
        let fraction = self.fraction(value);
        let (horizontal_fraction, vertical_fraction) = match self.orientation {
            Axis::Horizontal => (fraction, 0.5),
            Axis::Vertical => (0.5, 1. - fraction),
        };
        let unmeasured = control_size == Size::default() || thumb_size == Size::default();

        let offset = |axis: Axis, fraction: f32| -> Length {
            if unmeasured {
                return relative(fraction).into();
            }

            let (control_length, thumb_length) = match axis {
                Axis::Horizontal => (control_size.width, thumb_size.width),
                Axis::Vertical => (control_size.height, thumb_size.height),
            };
            let alignment = if axis == self.orientation {
                self.alignment
            } else {
                ThumbAlignment::Center
            };

            Self::aligned_offset(control_length, thumb_length, fraction, alignment).into()
        };

        Point {
            x: offset(Axis::Horizontal, horizontal_fraction),
            y: offset(Axis::Vertical, vertical_fraction),
        }
    }

    fn aligned_offset(
        control_length: Pixels,
        thumb_length: Pixels,
        fraction: f32,
        alignment: ThumbAlignment,
    ) -> Pixels {
        match alignment {
            ThumbAlignment::Center => control_length * fraction - thumb_length / 2.,
            ThumbAlignment::Edge => (control_length - thumb_length).max(px(0.)) * fraction,
        }
    }

    fn axis_position(self, position: Point<Pixels>) -> Pixels {
        match self.orientation {
            Axis::Horizontal => position.x,
            Axis::Vertical => position.y,
        }
    }

    fn bounds_center_axis(self, bounds: Bounds<Pixels>) -> Pixels {
        self.axis_position(bounds.center())
    }

    fn bounds_start(self, bounds: Bounds<Pixels>) -> Pixels {
        match self.orientation {
            Axis::Horizontal => bounds.left(),
            Axis::Vertical => bounds.top(),
        }
    }

    fn size_along(self, size: Size<Pixels>) -> Pixels {
        match self.orientation {
            Axis::Horizontal => size.width,
            Axis::Vertical => size.height,
        }
    }
}

impl ThumbCollisionBehavior {
    fn resolve(
        self,
        values: &SliderValue,
        thumb_idx: usize,
        target: f32,
        model: SliderModel,
    ) -> (SliderValue, usize) {
        let mut next = values.clone();
        let mut active_idx = thumb_idx;

        match self {
            Self::Push => {
                let room_before = model.minimum_spacing * thumb_idx as f32;
                let room_after = model.minimum_spacing * (values.len() - thumb_idx - 1) as f32;
                next.0[thumb_idx] =
                    target.clamp(model.minimum + room_before, model.maximum - room_after);

                for current_idx in (0..thumb_idx).rev() {
                    let maximum = next.0[current_idx + 1] - model.minimum_spacing;
                    next.0[current_idx] = next.0[current_idx].min(maximum);
                }

                for current_idx in thumb_idx + 1..next.len() {
                    let minimum = next.0[current_idx - 1] + model.minimum_spacing;
                    next.0[current_idx] = next.0[current_idx].max(minimum);
                }
            }
            Self::Clamp | Self::Swap => {
                if self == Self::Swap {
                    while active_idx > 0 && target < next.0[active_idx - 1] {
                        next.0.swap(active_idx, active_idx - 1);
                        active_idx -= 1;
                    }

                    while active_idx + 1 < next.len() && target > next.0[active_idx + 1] {
                        next.0.swap(active_idx, active_idx + 1);
                        active_idx += 1;
                    }
                }

                let (minimum, maximum) = model.effective_bounds(&next, active_idx);
                next.0[active_idx] = target.clamp(minimum, maximum);
            }
        }

        (next, active_idx)
    }
}

impl SliderChange {
    fn between(
        previous: &SliderValue,
        value: SliderValue,
        active_thumb_index: usize,
        reason: SliderChangeReason,
    ) -> Option<Self> {
        if value == *previous {
            return None;
        }

        Some(Self {
            value,
            active_thumb_index,
            reason,
        })
    }
}

#[derive(Clone)]
struct SliderRenderContext {
    values: SliderValue,
    model: SliderModel,
    interaction: SliderInteraction,
    interactive: bool,
    label: Option<SharedString>,
    value_text_formatter: Option<ValueTextFormatter>,
}

impl SliderRenderContext {
    fn format_value(&self, value: f32) -> SharedString {
        self.value_text_formatter
            .as_ref()
            .map(|formatter| formatter(value))
            .unwrap_or_else(|| format!("{value}").into())
    }
}

/// A visible label associated with the slider thumbs.
#[derive(Styled, ParentElement, StatefulInteractiveElement)]
pub struct BaseSliderLabel {
    base: Stateful<Div>,
    #[style]
    style: StyleRefinement,
    text: Option<SharedString>,
    #[children]
    children: Vec<AnyElement>,
}

impl InteractiveElement for BaseSliderLabel {
    fn interactivity(&mut self) -> &mut Interactivity {
        self.base.interactivity()
    }
}

impl BaseSliderLabel {
    pub fn new(element_id: impl Into<ElementId>) -> Self {
        Self {
            base: div().id(element_id),
            style: StyleRefinement::default(),
            text: None,
            children: Vec::new(),
        }
    }

    pub fn text(mut self, text: impl Into<SharedString>) -> Self {
        self.text = Some(text.into());

        self
    }

    fn accessible_name(&self) -> Option<SharedString> {
        self.text.clone()
    }

    fn render(self) -> AnyElement {
        self.base
            .role(Role::Label)
            .refine_style(&self.style)
            .when_some(self.text, |this, text| this.child(text))
            .children(self.children)
            .into_any_element()
    }
}

type SliderValuesFormatter = Rc<dyn Fn(&SliderValue) -> SharedString>;

/// Displays the current effective slider values.
#[derive(Styled, StatefulInteractiveElement)]
pub struct BaseSliderValue {
    base: Stateful<Div>,
    #[style]
    style: StyleRefinement,
    formatter: Option<SliderValuesFormatter>,
}

impl InteractiveElement for BaseSliderValue {
    fn interactivity(&mut self) -> &mut Interactivity {
        self.base.interactivity()
    }
}

impl BaseSliderValue {
    pub fn new(element_id: impl Into<ElementId>) -> Self {
        Self {
            base: div().id(element_id),
            style: StyleRefinement::default(),
            formatter: None,
        }
    }

    pub fn formatter<Text>(mut self, formatter: impl Fn(&SliderValue) -> Text + 'static) -> Self
    where
        Text: Into<SharedString>,
    {
        self.formatter = Some(Rc::new(move |values| formatter(values).into()));

        self
    }

    fn render(self, context: &SliderRenderContext) -> AnyElement {
        let text = self
            .formatter
            .map(|formatter| formatter(&context.values))
            .unwrap_or_else(|| {
                context
                    .values
                    .as_slice()
                    .iter()
                    .map(|value| context.format_value(*value).to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
                    .into()
            });

        self.base
            .refine_style(&self.style)
            .child(text)
            .into_any_element()
    }
}

/// The clickable area of a slider.
#[derive(Styled, ParentElement, StatefulInteractiveElement)]
pub struct BaseSliderControl {
    base: Stateful<Div>,
    #[style]
    style: StyleRefinement,
    track: Option<BaseSliderTrack>,
    #[children]
    children: Vec<AnyElement>,
}

impl InteractiveElement for BaseSliderControl {
    fn interactivity(&mut self) -> &mut Interactivity {
        self.base.interactivity()
    }
}

impl BaseSliderControl {
    pub fn new(element_id: impl Into<ElementId>) -> Self {
        Self {
            base: div().id(element_id),
            style: StyleRefinement::default(),
            track: None,
            children: Vec::new(),
        }
    }

    pub fn track(mut self, track: BaseSliderTrack) -> Self {
        self.track = Some(track);

        self
    }

    fn render(
        self,
        context: &SliderRenderContext,
        window: &mut Window,
        cx: &mut App,
    ) -> AnyElement {
        let track = self
            .track
            .expect("a slider control requires a track")
            .render(context, window, cx);
        let base = self
            .base
            .relative()
            .size_full()
            .refine_style(&self.style)
            .child(track.element)
            .children(self.children)
            .child(context.interaction.measure_bounds(None));

        if context.interactive {
            return context
                .interaction
                .install_pointer(
                    base,
                    context.values.clone(),
                    context.model,
                    track.focus_handles,
                )
                .into_any_element();
        }

        base.on_mouse_down(MouseButton::Left, |_event, _window, cx| {
            cx.stop_propagation();
        })
        .into_any_element()
    }
}

struct RenderedSliderTrack {
    element: AnyElement,
    focus_handles: SmallVec<[FocusHandle; 2]>,
}

/// Contains the indicator, custom marks, and slider thumbs.
#[derive(Styled, ParentElement, StatefulInteractiveElement)]
pub struct BaseSliderTrack {
    base: Stateful<Div>,
    #[style]
    style: StyleRefinement,
    indicator: Option<BaseSliderIndicator>,
    thumbs: Vec<BaseSliderThumb>,
    #[children]
    children: Vec<AnyElement>,
}

impl InteractiveElement for BaseSliderTrack {
    fn interactivity(&mut self) -> &mut Interactivity {
        self.base.interactivity()
    }
}

impl BaseSliderTrack {
    pub fn new(element_id: impl Into<ElementId>) -> Self {
        Self {
            base: div().id(element_id),
            style: StyleRefinement::default(),
            indicator: None,
            thumbs: Vec::new(),
            children: Vec::new(),
        }
    }

    pub fn indicator(mut self, indicator: BaseSliderIndicator) -> Self {
        self.indicator = Some(indicator);

        self
    }

    pub fn thumb(mut self, thumb: BaseSliderThumb) -> Self {
        self.thumbs.push(thumb);

        self
    }

    fn render(
        self,
        context: &SliderRenderContext,
        window: &mut Window,
        cx: &mut App,
    ) -> RenderedSliderTrack {
        assert_eq!(
            self.thumbs.len(),
            context.values.len(),
            "a slider requires one thumb for each value"
        );

        let mut element = self
            .base
            .relative()
            .size_full()
            .refine_style(&self.style)
            .when_some(self.indicator, |this, indicator| {
                this.child(indicator.render(context))
            })
            .children(self.children);
        let mut focus_handles = SmallVec::new();

        for (thumb_idx, thumb) in self.thumbs.into_iter().enumerate() {
            let (thumb, focus_handle) = thumb.render(context, thumb_idx, window, cx);

            element = element.child(thumb);
            focus_handles.push(focus_handle);
        }

        RenderedSliderTrack {
            element: element.into_any_element(),
            focus_handles,
        }
    }
}

/// Shows the selected portion of the slider track.
#[derive(Styled, ParentElement, StatefulInteractiveElement)]
pub struct BaseSliderIndicator {
    base: Stateful<Div>,
    #[style]
    style: StyleRefinement,
    #[children]
    children: Vec<AnyElement>,
}

impl InteractiveElement for BaseSliderIndicator {
    fn interactivity(&mut self) -> &mut Interactivity {
        self.base.interactivity()
    }
}

impl BaseSliderIndicator {
    pub fn new(element_id: impl Into<ElementId>) -> Self {
        Self {
            base: div().id(element_id),
            style: StyleRefinement::default(),
            children: Vec::new(),
        }
    }

    fn render(self, context: &SliderRenderContext) -> AnyElement {
        let values = &context.values;
        let start_value = if values.len() == 1 {
            context.model.minimum
        } else {
            values.0[0]
        };
        let end_value = *values.0.last().expect("slider values are non-empty");
        let start = context.model.fraction(start_value);
        let end = context.model.fraction(end_value);

        self.base
            .absolute()
            .when(context.model.orientation == Axis::Horizontal, |this| {
                this.top(px(0.))
                    .bottom(px(0.))
                    .left(relative(start))
                    .right(relative(1. - end))
            })
            .when(context.model.orientation == Axis::Vertical, |this| {
                this.left(px(0.))
                    .right(px(0.))
                    .bottom(relative(start))
                    .top(relative(1. - end))
            })
            .refine_style(&self.style)
            .children(self.children)
            .into_any_element()
    }
}

/// A focusable thumb that controls one slider value.
#[derive(Styled, ParentElement, StatefulInteractiveElement)]
pub struct BaseSliderThumb {
    element_id: ElementId,
    base: Stateful<Div>,
    #[style]
    style: StyleRefinement,
    #[children]
    children: Vec<AnyElement>,
    aria_label: Option<SharedString>,
    value_text_formatter: Option<ValueTextFormatter>,
}

impl InteractiveElement for BaseSliderThumb {
    fn interactivity(&mut self) -> &mut Interactivity {
        self.base.interactivity()
    }
}

impl BaseSliderThumb {
    pub fn new(element_id: impl Into<ElementId>) -> Self {
        let element_id = element_id.into();

        Self {
            element_id: element_id.clone(),
            base: div().id(element_id),
            style: StyleRefinement::default(),
            children: Vec::new(),
            aria_label: None,
            value_text_formatter: None,
        }
    }

    pub fn aria_label(mut self, label: impl Into<SharedString>) -> Self {
        self.aria_label = Some(label.into());

        self
    }

    pub fn value_text_formatter<Text>(mut self, formatter: impl Fn(f32) -> Text + 'static) -> Self
    where
        Text: Into<SharedString>,
    {
        self.value_text_formatter = Some(Rc::new(move |value| formatter(value).into()));

        self
    }

    fn render(
        self,
        context: &SliderRenderContext,
        thumb_idx: usize,
        window: &mut Window,
        cx: &mut App,
    ) -> (AnyElement, FocusHandle) {
        let values = &context.values;
        let model = context.model;
        let interaction = &context.interaction;
        let value = values.0[thumb_idx];

        let bounds = interaction.state.read(cx).control_bounds;
        let thumb_size = interaction.thumb_size(thumb_idx, cx);
        let position = model.thumb_position(bounds.size, thumb_size, value);

        let (effective_minimum, effective_maximum) = model.effective_bounds(values, thumb_idx);
        let focus_handle = use_focus_handle(self.element_id.clone(), window, cx, None);

        let keyboard_values = values.clone();
        let keyboard_interaction = interaction.clone();

        let accessibility_values = values.clone();
        let increment_interaction = interaction.clone();
        let decrement_values = values.clone();
        let decrement_interaction = interaction.clone();
        let set_value_values = values.clone();
        let set_value_interaction = interaction.clone();

        let aria_label = self.aria_label.or_else(|| context.label.clone());
        let value_text_formatter = self
            .value_text_formatter
            .or_else(|| context.value_text_formatter.clone());

        let thumb = self
            .base
            .absolute()
            .role(Role::Slider)
            .aria_numeric_value(f64::from(value))
            .aria_min_numeric_value(f64::from(effective_minimum))
            .aria_max_numeric_value(f64::from(effective_maximum))
            .aria_numeric_value_step(f64::from(model.step))
            .aria_orientation(match model.orientation {
                Axis::Horizontal => accesskit::Orientation::Horizontal,
                Axis::Vertical => accesskit::Orientation::Vertical,
            })
            .when_some(aria_label, |this, label| this.aria_label(label))
            .when_some(value_text_formatter, |this, formatter| {
                this.aria_value(formatter(value))
            })
            .left(position.x)
            .top(position.y)
            .refine_style(&self.style)
            .children(self.children)
            .child(interaction.measure_bounds(Some(thumb_idx)))
            .when(context.interactive, |this| {
                this.track_focus(&focus_handle)
                    .on_key_down(move |event, window, cx| {
                        let Some(target) = model.keyboard_target(
                            event,
                            keyboard_values.0[thumb_idx],
                            effective_minimum,
                            effective_maximum,
                        ) else {
                            return;
                        };

                        cx.stop_propagation();
                        keyboard_interaction.propose_discrete(
                            &keyboard_values,
                            thumb_idx,
                            target,
                            SliderChangeReason::Keyboard,
                            model,
                            window,
                            cx,
                        );
                    })
                    .on_a11y_action(AccessibleAction::Increment, move |_data, window, cx| {
                        increment_interaction.propose_discrete(
                            &accessibility_values,
                            thumb_idx,
                            accessibility_values.0[thumb_idx] + model.step,
                            SliderChangeReason::Accessibility,
                            model,
                            window,
                            cx,
                        );
                    })
                    .on_a11y_action(AccessibleAction::Decrement, move |_data, window, cx| {
                        decrement_interaction.propose_discrete(
                            &decrement_values,
                            thumb_idx,
                            decrement_values.0[thumb_idx] - model.step,
                            SliderChangeReason::Accessibility,
                            model,
                            window,
                            cx,
                        );
                    })
                    .on_a11y_action(AccessibleAction::SetValue, move |data, window, cx| {
                        let Some(accesskit::ActionData::NumericValue(target)) = data else {
                            return;
                        };

                        let target = *target as f32;

                        if !target.is_finite() {
                            return;
                        }

                        set_value_interaction.propose_discrete(
                            &set_value_values,
                            thumb_idx,
                            target,
                            SliderChangeReason::Accessibility,
                            model,
                            window,
                            cx,
                        );
                    })
            });

        (thumb.into_any_element(), focus_handle)
    }
}

#[derive(Clone)]
struct SliderInteraction {
    state: Entity<SliderInteractionState>,
    on_value_change: Option<OnValueChange>,
    on_value_committed: Option<OnValueCommitted>,
}

impl SliderInteraction {
    fn new(
        element_id: &ElementId,
        values: SliderValue,
        interactive: bool,
        on_value_change: Option<OnValueChange>,
        on_value_committed: Option<OnValueCommitted>,
        window: &mut Window,
        cx: &mut App,
    ) -> Self {
        let state =
            window.use_keyed_state((element_id.clone(), "state:slider"), cx, |_window, _cx| {
                SliderInteractionState::default()
            });
        let interaction = Self {
            state,
            on_value_change,
            on_value_committed,
        };

        let current = interaction.state.read(cx);
        let value_count_changed =
            current.active_thumb_index.is_some() && current.pressed_values.len() != values.len();

        if !interactive || value_count_changed {
            interaction.cancel(cx);
        }

        interaction
    }

    fn rendered_values(&self, fallback: &SliderValue, cx: &App) -> SliderValue {
        self.state
            .read(cx)
            .preview_values
            .clone()
            .unwrap_or_else(|| fallback.clone())
    }

    fn thumb_size(&self, thumb_idx: usize, cx: &App) -> Size<Pixels> {
        self.state.read(cx).thumb_size(thumb_idx)
    }

    fn cancel(&self, cx: &mut App) {
        if self.state.read(cx).active_thumb_index.is_none() {
            return;
        }

        self.state.update(cx, |state, _cx| state.cancel());
    }

    fn measure_bounds(&self, thumb_idx: Option<usize>) -> impl IntoElement {
        let state = self.state.clone();

        canvas(
            |bounds, _window, _cx| bounds,
            move |_bounds, measured, _window, cx| {
                state.update(cx, |state, cx| {
                    let bounds = if let Some(thumb_idx) = thumb_idx {
                        if state.thumb_bounds.len() <= thumb_idx {
                            state.thumb_bounds.resize(thumb_idx + 1, Bounds::default());
                        }

                        &mut state.thumb_bounds[thumb_idx]
                    } else {
                        &mut state.control_bounds
                    };

                    if *bounds == measured {
                        return;
                    }

                    *bounds = measured;
                    cx.notify();
                });
            },
        )
        .absolute()
        .inset_0()
    }

    fn install_pointer<Element>(
        &self,
        control: Element,
        values: SliderValue,
        model: SliderModel,
        focus_handles: SmallVec<[FocusHandle; 2]>,
    ) -> Element
    where
        Element: ParentElement + StatefulInteractiveElement,
    {
        let down_interaction = self.clone();
        let up_interaction = self.clone();
        let touch_interaction = self.clone();
        let touch_values = values.clone();

        control
            .child(self.track_mouse(model, focus_handles.clone()))
            .on_mouse_down_all(move |event, phase, hitbox, window, cx| {
                if phase != DispatchPhase::Bubble
                    || event.button != MouseButton::Left
                    || !hitbox.is_hovered(window)
                {
                    return;
                }

                cx.stop_propagation();
                let change = down_interaction.state.update(cx, |state, cx| {
                    state.control_bounds = hitbox.bounds;
                    let thumb_idx = state.closest_thumb(event.position, &values, model);
                    let pressed_thumb = state
                        .thumb_bounds
                        .get(thumb_idx)
                        .is_some_and(|bounds| bounds.contains(&event.position));
                    let pointer_offset = if pressed_thumb {
                        model.axis_position(event.position)
                            - model.bounds_center_axis(state.thumb_bounds[thumb_idx])
                    } else {
                        px(0.)
                    };

                    state.start(thumb_idx, values.clone(), pointer_offset);
                    cx.notify();

                    if pressed_thumb {
                        return None;
                    }

                    state.update_from_pointer(event.position, SliderChangeReason::TrackPress, model)
                });

                let thumb_idx = down_interaction.state.read(cx).active_thumb_index.unwrap();
                focus_handles[thumb_idx].focus(window, cx);

                down_interaction.emit(change.as_ref(), None, window, cx);
            })
            .on_mouse_up_all(move |event, phase, _hitbox, window, cx| {
                if phase != DispatchPhase::Capture
                    || event.button != MouseButton::Left
                    || up_interaction.state.read(cx).active_thumb_index.is_none()
                {
                    return;
                }

                cx.stop_propagation();
                let (live, committed) = up_interaction.state.update(cx, |state, cx| {
                    let live =
                        state.update_from_pointer(event.position, SliderChangeReason::Drag, model);
                    let committed = state.finish();
                    cx.notify();

                    (live, committed)
                });

                up_interaction.emit(live.as_ref(), committed.as_ref(), window, cx);
            })
            .on_click(move |event, window, cx| {
                let ClickEvent::Touch(touch) = event else {
                    return;
                };

                cx.stop_propagation();
                touch_interaction.propose_touch_tap(
                    touch.position,
                    &touch_values,
                    model,
                    window,
                    cx,
                );
            })
    }

    fn track_mouse(
        &self,
        model: SliderModel,
        focus_handles: SmallVec<[FocusHandle; 2]>,
    ) -> impl IntoElement {
        let interaction = self.clone();

        canvas(
            |_bounds, _window, _cx| (),
            move |_bounds, _prepaint, window, _cx| {
                let interaction = interaction.clone();
                let focus_handles = focus_handles.clone();

                window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
                    if phase != DispatchPhase::Capture
                        || !event.dragging()
                        || interaction.state.read(cx).active_thumb_index.is_none()
                    {
                        return;
                    }

                    let previous_idx = interaction.state.read(cx).active_thumb_index;
                    let change = interaction.state.update(cx, |state, cx| {
                        let change = state.update_from_pointer(
                            event.position,
                            SliderChangeReason::Drag,
                            model,
                        );

                        if change.is_some() {
                            cx.notify();
                        }

                        change
                    });
                    let active_idx = interaction.state.read(cx).active_thumb_index;

                    if active_idx != previous_idx
                        && let Some(active_idx) = active_idx
                    {
                        focus_handles[active_idx].focus(window, cx);
                    }

                    interaction.emit(change.as_ref(), None, window, cx);
                });
            },
        )
        .absolute()
        .inset_0()
    }

    fn propose_touch_tap(
        &self,
        position: Point<Pixels>,
        values: &SliderValue,
        model: SliderModel,
        window: &mut Window,
        cx: &mut App,
    ) {
        let change = self.state.update(cx, |state, _cx| {
            let thumb_idx = state.closest_thumb(position, values, model);
            let target = model.value_at_pointer(
                position,
                state.control_bounds,
                px(0.),
                state.thumb_size(thumb_idx),
            );
            let (value, active_thumb_index) = model
                .collision_behavior
                .resolve(values, thumb_idx, target, model);

            SliderChange::between(
                values,
                value,
                active_thumb_index,
                SliderChangeReason::TrackPress,
            )
        });

        self.emit(change.as_ref(), change.as_ref(), window, cx);
    }

    #[allow(clippy::too_many_arguments)]
    fn propose_discrete(
        &self,
        values: &SliderValue,
        thumb_idx: usize,
        target: f32,
        reason: SliderChangeReason,
        model: SliderModel,
        window: &mut Window,
        cx: &mut App,
    ) {
        let target = target.clamp(model.minimum, model.maximum);
        let target = if matches!(target, value if value == model.minimum || value == model.maximum)
        {
            target
        } else {
            model.snap(target)
        };

        let (value, active_thumb_index) =
            ThumbCollisionBehavior::Clamp.resolve(values, thumb_idx, target, model);
        let change = SliderChange::between(values, value, active_thumb_index, reason);

        self.emit(change.as_ref(), change.as_ref(), window, cx);
    }

    fn emit(
        &self,
        live: Option<&SliderChange>,
        committed: Option<&SliderChange>,
        window: &mut Window,
        cx: &mut App,
    ) {
        for (handler, change) in [
            (&self.on_value_change, live),
            (&self.on_value_committed, committed),
        ] {
            if let (Some(handler), Some(change)) = (handler, change) {
                handler(change, window, cx);
            }
        }
    }
}

#[derive(Clone, Default)]
struct SliderInteractionState {
    control_bounds: Bounds<Pixels>,
    thumb_bounds: SmallVec<[Bounds<Pixels>; 2]>,
    active_thumb_index: Option<usize>,
    pressed_values: SmallVec<[f32; 2]>,
    preview_values: Option<SliderValue>,
    pointer_offset: Pixels,
    last_reason: Option<SliderChangeReason>,
}

impl SliderInteractionState {
    fn start(&mut self, thumb_idx: usize, values: SliderValue, pointer_offset: Pixels) {
        self.active_thumb_index = Some(thumb_idx);
        self.pressed_values = values.0.clone();
        self.preview_values = Some(values);
        self.pointer_offset = pointer_offset;
        self.last_reason = None;
    }

    fn cancel(&mut self) {
        self.active_thumb_index = None;
        self.pressed_values.clear();
        self.preview_values = None;
        self.pointer_offset = px(0.);
        self.last_reason = None;
    }

    fn finish(&mut self) -> Option<SliderChange> {
        let active_thumb_index = self.active_thumb_index?;
        let value = self.preview_values.take()?;
        let changed = value.as_slice() != self.pressed_values.as_slice();
        let reason = self.last_reason;

        self.cancel();

        if !changed {
            return None;
        }

        Some(SliderChange {
            value,
            active_thumb_index,
            reason: reason.expect("a changed slider has a change reason"),
        })
    }

    fn update_from_pointer(
        &mut self,
        position: Point<Pixels>,
        reason: SliderChangeReason,
        model: SliderModel,
    ) -> Option<SliderChange> {
        let thumb_idx = self.active_thumb_index?;
        let values = self.preview_values.as_ref()?;
        let target = model.value_at_pointer(
            position,
            self.control_bounds,
            self.pointer_offset,
            self.thumb_size(thumb_idx),
        );
        let (value, active_thumb_index) = model
            .collision_behavior
            .resolve(values, thumb_idx, target, model);
        let change = SliderChange::between(values, value, active_thumb_index, reason)?;

        self.active_thumb_index = Some(active_thumb_index);
        self.preview_values = Some(change.value.clone());
        self.last_reason = Some(reason);

        Some(change)
    }

    fn closest_thumb(
        &self,
        position: Point<Pixels>,
        values: &SliderValue,
        model: SliderModel,
    ) -> usize {
        let pointer = model.axis_position(position);

        values
            .0
            .iter()
            .enumerate()
            .min_by(|(left_idx, left), (right_idx, right)| {
                let left_center = self.thumb_center(*left_idx, **left, model);
                let right_center = self.thumb_center(*right_idx, **right, model);
                let left_distance = f32::from((pointer - left_center).abs());
                let right_distance = f32::from((pointer - right_center).abs());

                left_distance
                    .total_cmp(&right_distance)
                    .then_with(|| right_idx.cmp(left_idx))
            })
            .map(|(thumb_idx, _value)| thumb_idx)
            .expect("slider values are non-empty")
    }

    fn thumb_center(&self, thumb_idx: usize, value: f32, model: SliderModel) -> Pixels {
        let fraction = model.fraction(value);
        let size = self.thumb_size(thumb_idx);
        let axis_size = model.size_along(size);
        let control_start = model.bounds_start(self.control_bounds);
        let control_length = model.size_along(self.control_bounds.size);
        let usable_length = match model.alignment {
            ThumbAlignment::Center => control_length,
            ThumbAlignment::Edge => (control_length - axis_size).max(px(0.)),
        };
        let start_inset = match model.alignment {
            ThumbAlignment::Center => px(0.),
            ThumbAlignment::Edge => axis_size / 2.,
        };
        let directional_fraction = match model.orientation {
            Axis::Horizontal => fraction,
            Axis::Vertical => 1. - fraction,
        };

        control_start + start_inset + usable_length * directional_fraction
    }

    fn thumb_size(&self, thumb_idx: usize) -> Size<Pixels> {
        self.thumb_bounds
            .get(thumb_idx)
            .map(|bounds| bounds.size)
            .unwrap_or_default()
    }
}

fn use_focus_handle(
    base_id: impl Into<ElementId>,
    window: &mut Window,
    cx: &mut App,
    focus_handle: Option<FocusHandle>,
) -> FocusHandle {
    focus_handle.unwrap_or_else(|| {
        window
            .use_keyed_state((base_id.into(), "state:focus_handle"), cx, |_window, cx| {
                cx.focus_handle().tab_stop(true)
            })
            .read(cx)
            .clone()
    })
}

#[cfg(test)]
mod tests {
    use super::{
        BaseSlider, BaseSliderControl, BaseSliderIndicator, BaseSliderLabel, BaseSliderThumb,
        BaseSliderTrack, BaseSliderValue, SliderChange, SliderChangeReason, SliderInteraction,
        SliderInteractionState, SliderModel, SliderRenderContext, SliderValue, ThumbAlignment,
        ThumbCollisionBehavior,
    };

    use std::rc::Rc;
    use std::sync::{Arc, Mutex};

    use gpui::{
        AppContext, Axis, Bounds, Context, Div, Element as _, Entity, InteractiveElement,
        IntoElement, KeyDownEvent, KeyUpEvent, Keystroke, Modifiers, MouseButton, ParentElement,
        Render, Size, StatefulInteractiveElement, Styled, TestAppContext, VisualTestContext,
        Window, accesskit, canvas, div, point, prelude::FluentBuilder, px, relative, size,
    };

    const SLIDER: &str = "base-slider";
    const CONTROL: &str = "slider-under-test-control";
    const TRACK: &str = "slider-under-test-track";
    const INDICATOR: &str = "slider-under-test-indicator";
    const FIRST_THUMB: &str = "slider-under-test-thumb-0";
    const SECOND_THUMB: &str = "slider-under-test-thumb-1";
    const LABEL: &str = "slider-under-test-label";
    const VALUE: &str = "slider-under-test-value";
    const ROOT_CHILD: &str = "slider-under-test-root-child";
    const TRACK_MARK: &str = "slider-under-test-track-mark";

    #[derive(Clone)]
    struct SliderHarness {
        values: SliderValue,
        disabled: bool,
        has_handler: bool,
        accept_changes: bool,
        orientation: Axis,
        alignment: ThumbAlignment,
        collision_behavior: ThumbCollisionBehavior,
        live: Vec<SliderChange>,
        committed: Vec<SliderChange>,
        parent_key_downs: usize,
        parent_clicks: usize,
    }

    impl Render for SliderHarness {
        fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let track = BaseSliderTrack::new("slider-track")
                .indicator(
                    BaseSliderIndicator::new("slider-indicator")
                        .debug_selector(|| INDICATOR.to_owned()),
                )
                .child(
                    div()
                        .id("track-mark")
                        .absolute()
                        .w(px(13.))
                        .h(px(1.))
                        .debug_selector(|| TRACK_MARK.to_owned()),
                )
                .debug_selector(|| TRACK.to_owned());
            let track = self.values.as_slice().iter().enumerate().fold(
                track,
                |track, (thumb_idx, _value)| {
                    let selector = format!("slider-under-test-thumb-{thumb_idx}");
                    let label = if thumb_idx == 0 { "Minimum" } else { "Maximum" };

                    track.thumb(
                        BaseSliderThumb::new(("slider-thumb", thumb_idx))
                            .aria_label(label)
                            .size(px(20.))
                            .debug_selector(move || selector),
                    )
                },
            );
            let control = BaseSliderControl::new("slider-control")
                .track(track)
                .debug_selector(|| CONTROL.to_owned());

            div()
                .id("slider-parent")
                .tab_group()
                .size_full()
                .on_key_down(cx.listener(|this, _event, _window, _cx| this.parent_key_downs += 1))
                .on_click(cx.listener(|this, _event, _window, _cx| this.parent_clicks += 1))
                .child(
                    BaseSlider::new("slider-under-test")
                        .values(self.values.as_slice().iter().copied())
                        .min(0.)
                        .max(100.)
                        .step(5.)
                        .large_step(20.)
                        .min_steps_between_values(2)
                        .thumb_collision_behavior(self.collision_behavior)
                        .thumb_alignment(self.alignment)
                        .orientation(self.orientation)
                        .disabled(self.disabled)
                        .label(
                            BaseSliderLabel::new("slider-label")
                                .text("Range")
                                .absolute()
                                .debug_selector(|| LABEL.to_owned()),
                        )
                        .value_display(
                            BaseSliderValue::new("slider-value")
                                .formatter(|values| format!("{} values", values.len()))
                                .absolute()
                                .debug_selector(|| VALUE.to_owned()),
                        )
                        .control(control)
                        .child(
                            div()
                                .id("root-child")
                                .absolute()
                                .w(px(11.))
                                .h(px(1.))
                                .debug_selector(|| ROOT_CHILD.to_owned()),
                        )
                        .absolute()
                        .left(px(40.))
                        .top(px(40.))
                        .w(px(200.))
                        .h(px(20.))
                        .when(self.orientation == Axis::Vertical, |this| {
                            this.w(px(20.)).h(px(200.))
                        })
                        .debug_selector(|| SLIDER.to_owned())
                        .when(self.has_handler, |this| {
                            this.on_value_change(cx.listener(
                                |this, change: &SliderChange, _window, cx| {
                                    this.live.push(change.clone());

                                    if this.accept_changes {
                                        this.values = change.value.clone();
                                    }

                                    cx.notify();
                                },
                            ))
                        })
                        .on_value_committed(cx.listener(
                            |this, change: &SliderChange, _window, _cx| {
                                this.committed.push(change.clone());
                            },
                        )),
                )
        }
    }

    fn setup(
        cx: &mut TestAppContext,
        values: impl IntoIterator<Item = f32>,
    ) -> (Entity<SliderHarness>, &mut VisualTestContext) {
        let values = SliderValue::new(values);

        cx.add_window_view(move |_window, _cx| SliderHarness {
            values,
            disabled: false,
            has_handler: true,
            accept_changes: true,
            orientation: Axis::Horizontal,
            alignment: ThumbAlignment::Center,
            collision_behavior: ThumbCollisionBehavior::Push,
            live: Vec::new(),
            committed: Vec::new(),
            parent_key_downs: 0,
            parent_clicks: 0,
        })
    }

    fn model(
        orientation: Axis,
        behavior: ThumbCollisionBehavior,
        minimum_spacing: f32,
    ) -> SliderModel {
        SliderModel {
            minimum: 0.,
            maximum: 100.,
            step: 5.,
            large_step: 20.,
            minimum_spacing,
            orientation,
            alignment: ThumbAlignment::Center,
            collision_behavior: behavior,
        }
    }

    fn bounds(cx: &mut VisualTestContext, selector: &'static str) -> Bounds<gpui::Pixels> {
        cx.debug_bounds(selector)
            .unwrap_or_else(|| panic!("{selector} should be rendered"))
    }

    fn activate_key(cx: &mut VisualTestContext, key: &str) {
        let keystroke = Keystroke::parse(key).unwrap();
        cx.simulate_event(KeyDownEvent {
            keystroke: keystroke.clone(),
            is_held: false,
            prefer_character_input: false,
        });

        cx.simulate_event(KeyUpEvent { keystroke });
    }

    fn snapshot(view: &Entity<SliderHarness>, cx: &VisualTestContext) -> SliderHarness {
        cx.read_entity(view, |view, _cx| view.clone())
    }

    #[test]
    fn value_math_clamps_snaps_and_maps_both_orientations() {
        let snap_cases = [
            (-10., 10., 20., 2.5, 10.),
            (21., 10., 20., 2.5, 20.),
            (13.7, 10., 20., 0.5, 13.5),
            (0.29, 0.1, 0.5, 0.1, 0.3),
        ];

        for (value, minimum, maximum, step, expected) in snap_cases {
            let model = SliderModel {
                minimum,
                maximum,
                step,
                ..model(Axis::Horizontal, ThumbCollisionBehavior::Clamp, 0.)
            };

            assert_eq!(model.snap(value), expected);
        }

        let fraction_model = SliderModel {
            minimum: 10.,
            maximum: 20.,
            ..model(Axis::Horizontal, ThumbCollisionBehavior::Clamp, 0.)
        };

        assert_eq!(fraction_model.fraction(-5.), 0.);
        assert_eq!(fraction_model.fraction(15.), 0.5);
        assert_eq!(fraction_model.fraction(25.), 1.);
        assert_eq!(SliderModel::minimum_spacing(0., 10., 4., 2, 3), 5.);

        for (values, expected) in [([120., -5.], [0., 100.]), ([78., 22.], [22., 78.])] {
            let model = model(Axis::Horizontal, ThumbCollisionBehavior::Push, 0.);

            assert_eq!(
                model
                    .normalized_values(&SliderValue::from(values))
                    .as_slice(),
                &expected
            );
        }

        let control = Bounds {
            origin: point(px(10.), px(20.)),
            size: size(px(100.), px(100.)),
        };
        let thumb_size = size(px(20.), px(20.));

        for (orientation, pointer, expected) in [
            (Axis::Horizontal, point(px(60.), px(70.)), 50.),
            (Axis::Vertical, point(px(60.), px(45.)), 75.),
        ] {
            let model = model(orientation, ThumbCollisionBehavior::Clamp, 0.);

            assert_eq!(
                model.value_at_pointer(pointer, control, px(0.), thumb_size),
                expected,
                "{orientation:?}"
            );
        }

        assert_eq!(
            SliderModel::aligned_offset(px(100.), px(20.), 0., ThumbAlignment::Center,),
            px(-10.)
        );
        assert_eq!(
            SliderModel::aligned_offset(px(100.), px(20.), 1., ThumbAlignment::Edge,),
            px(80.)
        );

        for (orientation, x, y) in [(Axis::Horizontal, 0.25, 0.5), (Axis::Vertical, 0.5, 0.75)] {
            let model = model(orientation, ThumbCollisionBehavior::Clamp, 0.);
            let expected = point(relative(x).into(), relative(y).into());

            for (control_size, thumb_size) in [
                (Size::default(), thumb_size),
                (control.size, Size::default()),
            ] {
                assert_eq!(
                    model.thumb_position(control_size, thumb_size, 25.),
                    expected,
                    "{orientation:?}"
                );
            }
        }
    }

    #[test]
    fn range_collision_behaviors_preserve_valid_values() {
        let values = SliderValue::from([20., 50., 80.]);
        let cases = [
            (ThumbCollisionBehavior::Clamp, 1, 100., [20., 70., 80.], 1),
            (ThumbCollisionBehavior::Push, 1, 95., [20., 90., 100.], 1),
            (ThumbCollisionBehavior::Push, 1, 5., [0., 10., 80.], 1),
            (ThumbCollisionBehavior::Swap, 0, 55., [50., 60., 80.], 1),
            (ThumbCollisionBehavior::Swap, 2, 45., [20., 40., 50.], 1),
        ];

        for (behavior, thumb_idx, target, expected, expected_idx) in cases {
            let model = model(Axis::Horizontal, behavior, 10.);
            let (actual, active_idx) = behavior.resolve(&values, thumb_idx, target, model);

            assert_eq!(
                actual.as_slice(),
                &expected,
                "{behavior:?}, thumb {thumb_idx}"
            );
            assert_eq!(active_idx, expected_idx, "{behavior:?}, thumb {thumb_idx}");
        }

        let push_model = model(Axis::Horizontal, ThumbCollisionBehavior::Push, 10.);
        let pushed = ThumbCollisionBehavior::Push
            .resolve(&values, 0, 70., push_model)
            .0;
        let restored = ThumbCollisionBehavior::Push
            .resolve(&pushed, 0, 20., push_model)
            .0;

        assert_eq!(pushed, SliderValue::from([70., 80., 90.]));
        assert_eq!(restored, SliderValue::from([20., 80., 90.]));
    }

    #[gpui::test]
    fn pointer_and_touch_interactions_report_live_and_committed_values(cx: &mut TestAppContext) {
        let (view, cx) = setup(cx, [25., 75.]);
        let control = bounds(cx, CONTROL);
        let press = point(control.left() + px(130.), control.center().y);
        let drag = point(control.left() + px(170.), control.center().y);
        let outside = point(control.right() + px(30.), control.center().y);

        cx.simulate_mouse_down(press, MouseButton::Left, Modifiers::none());

        let state = snapshot(&view, cx);
        assert_eq!(state.values, SliderValue::from([25., 65.]));
        assert_eq!(state.live.len(), 1);
        assert_eq!(state.live[0].reason, SliderChangeReason::TrackPress);
        assert_eq!(state.live[0].active_thumb_index, 1);

        cx.simulate_mouse_move(drag, MouseButton::Left, Modifiers::none());
        cx.simulate_mouse_up(outside, MouseButton::Left, Modifiers::none());

        let state = snapshot(&view, cx);
        assert_eq!(state.values, SliderValue::from([25., 100.]));
        assert_eq!(
            state
                .live
                .iter()
                .map(|change| change.reason)
                .collect::<Vec<_>>(),
            vec![
                SliderChangeReason::TrackPress,
                SliderChangeReason::Drag,
                SliderChangeReason::Drag,
            ]
        );
        assert_eq!(state.committed.len(), 1);
        assert_eq!(state.committed[0].value, SliderValue::from([25., 100.]));
        assert_eq!(state.committed[0].reason, SliderChangeReason::Drag);
        assert_eq!(state.parent_clicks, 0);

        let thumb = bounds(cx, FIRST_THUMB);
        let unchanged = point(thumb.right() - px(1.), thumb.center().y);

        for (drag, live_count) in [
            (unchanged, 3),
            (point(control.left() + px(80.), control.center().y), 5),
        ] {
            cx.simulate_mouse_down(unchanged, MouseButton::Left, Modifiers::none());
            cx.simulate_mouse_move(drag, MouseButton::Left, Modifiers::none());
            cx.simulate_mouse_move(unchanged, MouseButton::Left, Modifiers::none());
            cx.simulate_mouse_up(unchanged, MouseButton::Left, Modifiers::none());

            let state = snapshot(&view, cx);
            assert_eq!(state.values, SliderValue::from([25., 100.]));
            assert_eq!(state.live.len(), live_count);
            assert_eq!(state.committed.len(), 1);
        }

        let touch_events = Arc::new(Mutex::new(Vec::new()));
        let captured_live = touch_events.clone();
        let captured_committed = touch_events.clone();
        cx.update(|window, cx| {
            let state = cx.new(|_cx| SliderInteractionState {
                control_bounds: control,
                ..Default::default()
            });
            let interaction = SliderInteraction {
                state,
                on_value_change: Some(Rc::new(move |change, _window, _cx| {
                    captured_live.lock().unwrap().push(("live", change.clone()));
                })),
                on_value_committed: Some(Rc::new(move |change, _window, _cx| {
                    captured_committed
                        .lock()
                        .unwrap()
                        .push(("committed", change.clone()));
                })),
            };

            interaction.propose_touch_tap(
                control.center(),
                &SliderValue::from([25., 75.]),
                model(Axis::Horizontal, ThumbCollisionBehavior::Push, 10.),
                window,
                cx,
            );
        });

        let expected = SliderChange {
            value: SliderValue::from([25., 50.]),
            active_thumb_index: 1,
            reason: SliderChangeReason::TrackPress,
        };

        assert_eq!(
            touch_events.lock().unwrap().as_slice(),
            &[("live", expected.clone()), ("committed", expected)]
        );
    }

    #[gpui::test]
    fn keyboard_input_updates_the_focused_thumb(cx: &mut TestAppContext) {
        let (view, cx) = setup(cx, [20., 80.]);
        let first_thumb = bounds(cx, FIRST_THUMB);

        cx.simulate_mouse_down(first_thumb.center(), MouseButton::Left, Modifiers::none());
        cx.simulate_mouse_up(first_thumb.center(), MouseButton::Left, Modifiers::none());

        for key in ["right", "shift-right", "pageup", "pagedown", "home", "end"] {
            activate_key(cx, key);
        }

        let state = snapshot(&view, cx);
        let expected = [25., 45., 65., 45., 0., 70.]
            .into_iter()
            .map(|value| SliderChange {
                value: SliderValue::from([value, 80.]),
                active_thumb_index: 0,
                reason: SliderChangeReason::Keyboard,
            })
            .collect::<Vec<_>>();

        assert_eq!(state.live, expected);
        assert_eq!(state.committed, state.live);
        assert_eq!(state.parent_key_downs, 0);
    }

    #[gpui::test]
    fn controlled_and_disabled_states_remain_authoritative(cx: &mut TestAppContext) {
        let (view, cx) = setup(cx, [25.]);
        view.update(cx, |view, cx| {
            view.accept_changes = false;
            cx.notify();
        });

        let control = bounds(cx, CONTROL);
        let proposed = point(control.left() + px(150.), control.center().y);
        cx.simulate_mouse_down(proposed, MouseButton::Left, Modifiers::none());
        assert_eq!(bounds(cx, FIRST_THUMB).center().x, proposed.x);
        cx.simulate_mouse_up(proposed, MouseButton::Left, Modifiers::none());

        let rejected = snapshot(&view, cx);
        assert_eq!(rejected.values, SliderValue::from(25.));
        assert_eq!(bounds(cx, FIRST_THUMB).center().x, control.left() + px(50.));

        for (disabled, has_handler, value_count, drag_offset, release_offset) in [
            (true, true, 1, 100., 220.),
            (false, false, 1, 120., 120.),
            (false, true, 2, 140., 140.),
        ] {
            view.update(cx, |view, cx| {
                view.accept_changes = true;
                view.disabled = false;
                view.has_handler = true;
                view.values = SliderValue::from(25.);
                view.live.clear();
                view.committed.clear();
                cx.notify();
            });

            let thumb = bounds(cx, FIRST_THUMB);
            let drag = point(control.left() + px(drag_offset), control.center().y);
            let release = point(control.left() + px(release_offset), control.center().y);

            cx.simulate_mouse_down(thumb.center(), MouseButton::Left, Modifiers::none());
            cx.simulate_mouse_move(drag, MouseButton::Left, Modifiers::none());

            view.update(cx, |view, cx| {
                view.disabled = disabled;
                view.has_handler = has_handler;

                if value_count == 2 {
                    view.values = SliderValue::from([25., 75.]);
                }

                cx.notify();
            });

            cx.simulate_mouse_up(release, MouseButton::Left, Modifiers::none());
            assert!(
                snapshot(&view, cx).committed.is_empty(),
                "disabled={disabled}, has_handler={has_handler}, value_count={value_count}"
            );
        }

        view.update(cx, |view, cx| {
            view.has_handler = true;
            view.values = SliderValue::from(25.);
            cx.notify();
        });

        let restored_control = bounds(cx, CONTROL);
        let restored_press = point(
            restored_control.left() + px(160.),
            restored_control.center().y,
        );
        cx.simulate_mouse_down(restored_press, MouseButton::Left, Modifiers::none());
        cx.simulate_mouse_up(restored_press, MouseButton::Left, Modifiers::none());

        assert_eq!(snapshot(&view, cx).committed.len(), 1);
    }

    #[gpui::test]
    fn anatomy_and_accessibility_reflect_runtime_state(cx: &mut TestAppContext) {
        type Captured = Arc<Mutex<Option<[accesskit::Node; 2]>>>;

        struct AccessibilityProbe {
            captured: Captured,
        }

        impl Render for AccessibilityProbe {
            fn render(
                &mut self,
                _window: &mut Window,
                _cx: &mut Context<Self>,
            ) -> impl IntoElement {
                let captured = self.captured.clone();

                canvas(
                    move |_bounds, window, cx| {
                        let values = SliderValue::from([25., 75.]);
                        let model = SliderModel {
                            alignment: ThumbAlignment::Edge,
                            ..model(Axis::Vertical, ThumbCollisionBehavior::Push, 10.)
                        };
                        let nodes = [(0, true), (1, false)].map(|(thumb_idx, interactive)| {
                            let interaction = SliderInteraction::new(
                                &("probe-slider", thumb_idx).into(),
                                values.clone(),
                                interactive,
                                if interactive {
                                    Some(Rc::new(|_change, _window, _cx| {}))
                                } else {
                                    None
                                },
                                None,
                                window,
                                cx,
                            );
                            let context = SliderRenderContext {
                                values: values.clone(),
                                model,
                                interaction,
                                interactive,
                                label: Some("Price range".into()),
                                value_text_formatter: None,
                            };
                            let thumb = BaseSliderThumb::new(("probe-thumb", thumb_idx));
                            let thumb = if interactive {
                                thumb.value_text_formatter(|value| format!("{value} percent"))
                            } else {
                                thumb.aria_label("Maximum price")
                            };

                            let mut node = accesskit::Node::new(accesskit::Role::Slider);

                            thumb
                                .render(&context, thumb_idx, window, cx)
                                .0
                                .downcast_mut::<Div>()
                                .expect("slider thumb should render a div")
                                .write_a11y_info(&mut node);

                            node
                        });

                        *captured.lock().unwrap() = Some(nodes);
                    },
                    |_bounds, _prepaint, _window, _cx| {},
                )
            }
        }

        let (view, visual_cx) = setup(cx, [75., 25.]);

        assert_eq!(bounds(visual_cx, SLIDER).size, size(px(200.), px(20.)));
        assert_eq!(bounds(visual_cx, CONTROL).size, size(px(200.), px(20.)));
        assert_eq!(bounds(visual_cx, TRACK).size, size(px(200.), px(20.)));
        assert_eq!(bounds(visual_cx, INDICATOR).size.width, px(100.));
        assert_eq!(bounds(visual_cx, FIRST_THUMB).center().x, px(90.));
        assert_eq!(bounds(visual_cx, SECOND_THUMB).center().x, px(190.));
        assert!(bounds(visual_cx, LABEL).size.width > px(0.));
        assert!(bounds(visual_cx, VALUE).size.width > px(0.));
        assert_eq!(bounds(visual_cx, ROOT_CHILD).size.width, px(11.));
        assert_eq!(bounds(visual_cx, TRACK_MARK).size.width, px(13.));

        view.update(visual_cx, |view, cx| {
            view.orientation = Axis::Vertical;
            view.alignment = ThumbAlignment::Edge;
            cx.notify();
        });
        visual_cx.update(|window, cx| window.draw(cx).clear(cx));
        visual_cx.update(|window, cx| window.draw(cx).clear(cx));

        assert_eq!(bounds(visual_cx, SLIDER).size, size(px(20.), px(200.)));
        assert_eq!(bounds(visual_cx, FIRST_THUMB).center().y, px(185.));
        assert_eq!(bounds(visual_cx, SECOND_THUMB).center().y, px(95.));

        let captured: Captured = Arc::new(Mutex::new(None));
        let result = captured.clone();
        let (_probe, probe_cx) =
            cx.add_window_view(move |_window, _cx| AccessibilityProbe { captured });

        probe_cx.update(|window, cx| window.draw(cx).clear(cx));

        let [enabled, disabled] = result.lock().unwrap().take().unwrap();
        assert_eq!(enabled.role(), accesskit::Role::Slider);
        assert_eq!(enabled.label(), Some("Price range"));
        assert_eq!(enabled.numeric_value(), Some(25.));
        assert_eq!(enabled.min_numeric_value(), Some(0.));
        assert_eq!(enabled.max_numeric_value(), Some(65.));
        assert_eq!(enabled.numeric_value_step(), Some(5.));
        assert_eq!(enabled.value(), Some("25 percent"));
        assert_eq!(
            enabled.orientation(),
            Some(accesskit::Orientation::Vertical)
        );
        assert!(enabled.supports_action(accesskit::Action::Increment));
        assert!(enabled.supports_action(accesskit::Action::Decrement));
        assert!(enabled.supports_action(accesskit::Action::SetValue));
        assert!(enabled.supports_action(accesskit::Action::Focus));

        assert_eq!(disabled.role(), accesskit::Role::Slider);
        assert_eq!(disabled.label(), Some("Maximum price"));
        assert_eq!(disabled.numeric_value(), Some(75.));
        assert_eq!(disabled.min_numeric_value(), Some(35.));
        assert_eq!(disabled.max_numeric_value(), Some(100.));
        assert!(!disabled.supports_action(accesskit::Action::Increment));
        assert!(!disabled.supports_action(accesskit::Action::Decrement));
        assert!(!disabled.supports_action(accesskit::Action::SetValue));
        assert!(!disabled.supports_action(accesskit::Action::Focus));
    }
}
