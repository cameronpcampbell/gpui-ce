use std::time::Instant;

use crate::{Animated, Bounds, Lerp, Motion, Pixels};

#[derive(Clone, Copy)]
pub(crate) struct StyleTransitionContext {
    pub(crate) rem_size: Pixels,
    pub(crate) max_corner_radius: Pixels,
}

impl StyleTransitionContext {
    pub(crate) fn new(bounds: Bounds<Pixels>, rem_size: Pixels) -> Self {
        Self {
            rem_size,
            max_corner_radius: std::cmp::min(bounds.size.width, bounds.size.height) / 2.0,
        }
    }
}

pub(crate) struct StyleTransitionPropertyState<T: Lerp + Clone + PartialEq> {
    animated: Option<Animated<T, Instant>>,
}

impl<T> StyleTransitionPropertyState<T>
where
    T: Lerp + Clone + PartialEq,
{
    fn new(initial_goal: Option<T>) -> Self {
        Self {
            animated: initial_goal.map(Animated::new),
        }
    }

    fn jump_to(&mut self, goal: Option<T>) {
        match (self.animated.as_mut(), goal) {
            (Some(animated), Some(goal)) => animated.jump_to(goal),
            (_, Some(goal)) => {
                self.animated = Some(Animated::new(goal));
            }
            (_, None) => self.animated = None,
        }
    }

    pub(crate) fn evaluate(
        &mut self,
        goal: Option<T>,
        motion: &Motion,
        now: Instant,
        reduce_motion: bool,
    ) -> (bool, Option<T>) {
        if reduce_motion {
            self.jump_to(goal);
            return (
                false,
                self.animated
                    .as_ref()
                    .map(|animated| animated.value().clone()),
            );
        }

        let Some(goal) = goal else {
            self.animated = None;
            return (false, None);
        };

        let Some(animated) = self.animated.as_mut() else {
            self.animated = Some(Animated::new(goal.clone()));
            return (false, Some(goal));
        };

        animated.set(goal, motion, now);
        let sample = animated.sample(motion, now);
        (sample.is_active, Some(sample.value))
    }
}

gpui_macros::style_transitions!();

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AbsoluteLength, Bounds, DefiniteLength, Length, Style, px, rems, size};
    use std::time::Duration;

    fn length(value: f32) -> Length {
        Length::Definite(DefiniteLength::Absolute(AbsoluteLength::Pixels(px(value))))
    }

    fn size_transition_after(
        transitions: StyleTransitions,
        elapsed: Duration,
    ) -> (bool, Style, StyleTransitionState) {
        let started_at = Instant::now();
        let mut state = StyleTransitionState::default();
        let mut style = Style {
            size: size(length(10.0), length(10.0)),
            ..Style::default()
        };

        assert!(!transitions.apply_at(&mut style, &mut state, None, started_at, false));

        style.size = size(length(20.0), length(20.0));
        assert!(transitions.apply_at(&mut style, &mut state, None, started_at, false));

        style.size = size(length(20.0), length(20.0));
        let in_progress =
            transitions.apply_at(&mut style, &mut state, None, started_at + elapsed, false);

        (in_progress, style, state)
    }

    #[test]
    fn generated_transitions_exercise_properties_and_builder_precedence() {
        let (in_progress, style, _) = size_transition_after(
            StyleTransitions::new().w(Duration::from_secs(1)),
            Duration::from_millis(500),
        );
        assert!(in_progress);
        assert_eq!(style.size, size(length(15.0), length(20.0)));

        let (in_progress, style, state) = size_transition_after(
            StyleTransitions::new()
                .size(Duration::from_secs(1))
                .w(Duration::from_secs(2)),
            Duration::from_secs(1),
        );
        assert!(in_progress);
        assert_eq!(style.size, size(length(15.0), length(20.0)));
        let mut property_keys = state.properties.keys().copied().collect::<Vec<_>>();
        property_keys.sort_unstable();
        assert_eq!(property_keys, ["size.height", "size.width"]);

        let (in_progress, style, _) = size_transition_after(
            StyleTransitions::new()
                .w(Duration::from_secs(2))
                .size(Duration::from_secs(1)),
            Duration::from_secs(1),
        );
        assert!(!in_progress);
        assert_eq!(style.size, size(length(20.0), length(20.0)));

        let started_at = Instant::now();
        let mut state = StyleTransitionState::default();
        let mut style = Style {
            opacity: None,
            ..Style::default()
        };
        let opacity = StyleTransitions::new().opacity(Duration::from_secs(1));

        assert!(!opacity.apply_at(&mut style, &mut state, None, started_at, false));
        assert!(state.properties.contains_key("opacity"));
        style.opacity = Some(0.5);
        assert!(!opacity.apply_at(&mut style, &mut state, None, started_at, false));
        assert_eq!(style.opacity, Some(0.5));
        style.opacity = None;
        assert!(!opacity.apply_at(&mut style, &mut state, None, started_at, false));
        assert_eq!(style.opacity, None);

        style.opacity = Some(0.25);
        assert!(!opacity.apply_at(&mut style, &mut state, None, started_at, false));
        style.opacity = Some(0.75);
        assert!(!opacity.apply_at(&mut style, &mut state, None, started_at, true));
        assert_eq!(style.opacity, Some(0.75));
        style.opacity = Some(1.0);
        assert!(opacity.apply_at(&mut style, &mut state, None, started_at, false));
        assert_eq!(style.opacity, Some(0.75));

        StyleTransitions::new().apply_at(&mut style, &mut state, None, started_at, false);
        assert!(!state.properties.contains_key("opacity"));

        let mut corner_style = Style::default();
        corner_style.corner_radii.top_left = AbsoluteLength::Rems(rems(2.0));
        let context = StyleTransitionContext::new(
            Bounds {
                origin: Default::default(),
                size: size(px(40.0), px(20.0)),
            },
            px(16.0),
        );
        let mut corner_state = StyleTransitionState::default();
        let corners = StyleTransitions::new().rounded_tl(Duration::from_secs(1));

        assert!(!corners.apply_at(
            &mut corner_style,
            &mut corner_state,
            Some(context),
            started_at,
            false,
        ));
        assert_eq!(
            corner_style.corner_radii.top_left,
            AbsoluteLength::Pixels(px(10.0))
        );
        assert!(
            corner_state
                .properties
                .contains_key("corner_radii.top_left")
        );
    }
}
