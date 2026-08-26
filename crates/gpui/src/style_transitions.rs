use std::time::Instant;

use crate::{Lerp, MotionInfo};

pub(crate) struct StyleTransitionPropertyState<T> {
    goal_last_updated_at: Option<Instant>,
    start_goal: Option<T>,
    end_goal: Option<T>,
    last_delta: f32,
}

impl<T: Clone> StyleTransitionPropertyState<T> {
    fn new(initial_goal: Option<T>) -> Self {
        Self {
            goal_last_updated_at: None,
            start_goal: initial_goal.clone(),
            end_goal: initial_goal,
            last_delta: 1.0,
        }
    }

    fn jump_to(&mut self, goal: Option<T>) {
        self.start_goal = goal.clone();
        self.end_goal = goal;
        self.goal_last_updated_at = None;
        self.last_delta = 1.0;
    }
}

impl<T> StyleTransitionPropertyState<T>
where
    T: Lerp + Clone + PartialEq,
{
    pub(crate) fn evaluate(
        &mut self,
        goal: Option<T>,
        motion: &MotionInfo,
        now: Instant,
        reduce_motion: bool,
    ) -> (bool, Option<T>) {
        if reduce_motion {
            self.jump_to(goal);
            return (false, self.end_goal.clone());
        }

        self.update_goal(goal, motion, now);
        self.raw_evaluate(motion, now)
    }

    fn update_goal(&mut self, goal: Option<T>, motion: &MotionInfo, now: Instant) {
        if self.end_goal == goal {
            return;
        }

        let (_, current_value) = self.raw_evaluate(motion, now);
        self.start_goal = current_value;
        self.end_goal = goal;

        if motion.duration.is_zero()
            || self.start_goal.is_none()
            || self.end_goal.is_none()
            || self.start_goal == self.end_goal
        {
            self.jump_to(self.end_goal.clone());
        } else {
            self.goal_last_updated_at = Some(now);
            self.last_delta = 0.0;
        }
    }

    fn raw_evaluate(&mut self, motion: &MotionInfo, now: Instant) -> (bool, Option<T>) {
        let Some(goal_last_updated_at) = self.goal_last_updated_at else {
            self.last_delta = 1.0;
            return (false, self.end_goal.clone());
        };

        let duration = motion.duration.as_secs_f32();
        if duration == 0.0 {
            self.jump_to(self.end_goal.clone());
            return (false, self.end_goal.clone());
        }

        let linear_delta = now
            .saturating_duration_since(goal_last_updated_at)
            .as_secs_f32()
            / duration;
        if linear_delta >= 1.0 {
            self.jump_to(self.end_goal.clone());
            return (false, self.end_goal.clone());
        }

        let eased_delta = (motion.easing)(linear_delta.clamp(0.0, 1.0));
        debug_assert!(
            (0.0..=1.0).contains(&eased_delta),
            "style transition easing must return a value between 0 and 1"
        );

        self.last_delta = eased_delta.clamp(0.0, 1.0);

        if let (Some(start_goal), Some(end_goal)) =
            (self.start_goal.as_ref(), self.end_goal.as_ref())
        {
            return (true, Some(start_goal.lerp(end_goal, self.last_delta)));
        }

        self.jump_to(self.end_goal.clone());
        (false, self.end_goal.clone())
    }
}

gpui_macros::style_transitions!();

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AbsoluteLength, DefiniteLength, Length, Style, px};
    use std::time::Duration;

    fn length(value: f32) -> Length {
        Length::Definite(DefiniteLength::Absolute(AbsoluteLength::Pixels(px(value))))
    }

    fn definite_length(value: f32) -> DefiniteLength {
        DefiniteLength::Absolute(AbsoluteLength::Pixels(px(value)))
    }

    #[test]
    fn interrupted_property_motion_continues_from_its_current_value() {
        let motion = MotionInfo::new(1.0);
        let started_at = Instant::now();
        let mut state = StyleTransitionPropertyState::new(Some(0.0_f32));

        assert_eq!(
            state.evaluate(Some(10.0), &motion, started_at, false),
            (true, Some(0.0))
        );
        assert_eq!(
            state.evaluate(
                Some(20.0),
                &motion,
                started_at + Duration::from_millis(500),
                false,
            ),
            (true, Some(5.0))
        );
        assert_eq!(
            state.evaluate(
                Some(20.0),
                &motion,
                started_at + Duration::from_millis(1_000),
                false,
            ),
            (true, Some(12.5))
        );
    }

    #[test]
    fn non_animating_changes_apply_immediately() {
        let now = Instant::now();
        let motion = MotionInfo::new(1.0);
        let mut optional_state = StyleTransitionPropertyState::new(None::<f32>);

        assert_eq!(
            optional_state.evaluate(Some(10.0), &motion, now, false),
            (false, Some(10.0))
        );

        let mut state = StyleTransitionPropertyState::new(Some(0.0_f32));
        assert_eq!(
            state.evaluate(Some(10.0), &MotionInfo::new(Duration::ZERO), now, false),
            (false, Some(10.0))
        );
        assert_eq!(
            state.evaluate(Some(20.0), &motion, now, true),
            (false, Some(20.0))
        );
    }

    #[test]
    fn generated_width_transition_updates_the_resolved_style() {
        let transitions = StyleTransitions::new().w(1.0);
        let mut state = StyleTransitionState::default();
        let started_at = Instant::now();
        let mut style = Style {
            size: crate::size(length(10.0), Length::Auto),
            ..Style::default()
        };

        assert!(!transitions.apply_at(&mut style, &mut state, started_at, false));
        assert_eq!(style.size.width, length(10.0));

        style.size.width = length(20.0);
        assert!(transitions.apply_at(&mut style, &mut state, started_at, false));
        assert_eq!(style.size.width, length(10.0));

        style.size.width = length(20.0);
        assert!(transitions.apply_at(
            &mut style,
            &mut state,
            started_at + Duration::from_millis(500),
            false,
        ));
        assert_eq!(style.size.width, length(15.0));

        style.size.width = length(20.0);
        assert!(!transitions.apply_at(
            &mut style,
            &mut state,
            started_at + Duration::from_secs(1),
            false,
        ));
        assert_eq!(style.size.width, length(20.0));
    }

    #[test]
    fn overlapping_transition_builders_share_state_and_last_builder_wins() {
        let transitions = StyleTransitions::new().size(1.0).w(2.0);
        let mut state = StyleTransitionState::default();
        let started_at = Instant::now();
        let mut style = Style {
            size: crate::size(length(10.0), length(10.0)),
            ..Style::default()
        };

        assert!(!transitions.apply_at(&mut style, &mut state, started_at, false));
        assert_eq!(state.properties.len(), 2);
        assert!(state.properties.contains_key("size.width"));
        assert!(state.properties.contains_key("size.height"));

        style.size = crate::size(length(20.0), length(20.0));
        assert!(transitions.apply_at(&mut style, &mut state, started_at, false));

        style.size = crate::size(length(20.0), length(20.0));
        assert!(transitions.apply_at(
            &mut style,
            &mut state,
            started_at + Duration::from_secs(1),
            false,
        ));
        assert_eq!(style.size.width, length(15.0));
        assert_eq!(style.size.height, length(20.0));
    }

    #[test]
    fn composite_builder_overrides_earlier_single_field_builder() {
        let transitions = StyleTransitions::new().w(2.0).size(1.0);
        let mut state = StyleTransitionState::default();
        let started_at = Instant::now();
        let mut style = Style {
            size: crate::size(length(10.0), length(10.0)),
            ..Style::default()
        };

        assert!(!transitions.apply_at(&mut style, &mut state, started_at, false));

        style.size = crate::size(length(20.0), length(20.0));
        assert!(transitions.apply_at(&mut style, &mut state, started_at, false));

        style.size = crate::size(length(20.0), length(20.0));
        assert!(!transitions.apply_at(
            &mut style,
            &mut state,
            started_at + Duration::from_secs(1),
            false,
        ));
        assert_eq!(style.size.width, length(20.0));
        assert_eq!(style.size.height, length(20.0));
    }

    #[test]
    fn padding_aliases_share_canonical_property_state() {
        let transitions = StyleTransitions::new().p(1.0).pl(2.0);
        let mut state = StyleTransitionState::default();
        let started_at = Instant::now();
        let mut style = Style {
            padding: crate::Edges::all(definite_length(10.0)),
            ..Style::default()
        };

        assert!(!transitions.apply_at(&mut style, &mut state, started_at, false));
        assert_eq!(state.properties.len(), 4);
        assert!(state.properties.contains_key("padding.top"));
        assert!(state.properties.contains_key("padding.right"));
        assert!(state.properties.contains_key("padding.bottom"));
        assert!(state.properties.contains_key("padding.left"));

        style.padding = crate::Edges::all(definite_length(20.0));
        assert!(transitions.apply_at(&mut style, &mut state, started_at, false));

        style.padding = crate::Edges::all(definite_length(20.0));
        assert!(transitions.apply_at(
            &mut style,
            &mut state,
            started_at + Duration::from_secs(1),
            false,
        ));
        assert_eq!(style.padding.top, definite_length(20.0));
        assert_eq!(style.padding.right, definite_length(20.0));
        assert_eq!(style.padding.bottom, definite_length(20.0));
        assert_eq!(style.padding.left, definite_length(15.0));
    }
}
