use std::{
    rc::Rc,
    time::{Duration, Instant},
};

use crate::Lerp;

/// Converts a motion duration into a [`Duration`].
pub trait IntoMotionDuration {
    /// Converts this value into a duration.
    fn into_motion_duration(self) -> Duration;
}

impl IntoMotionDuration for Duration {
    fn into_motion_duration(self) -> Duration {
        self
    }
}

impl IntoMotionDuration for f32 {
    fn into_motion_duration(self) -> Duration {
        assert!(
            self.is_finite() && self >= 0.0,
            "motion duration must be a finite, non-negative number of seconds"
        );
        Duration::from_secs_f32(self)
    }
}

/// Timing and easing configuration for a declarative style transition.
#[derive(Clone)]
pub struct MotionInfo {
    duration: Duration,
    easing: Rc<dyn Fn(f32) -> f32>,
}

impl MotionInfo {
    /// Creates a linear motion with the supplied duration.
    ///
    /// A `Duration` is used as-is. An `f32` is interpreted as seconds.
    pub fn new(duration: impl IntoMotionDuration) -> Self {
        Self {
            duration: duration.into_motion_duration(),
            easing: Rc::new(crate::linear),
        }
    }

    /// Replaces the linear easing function.
    pub fn with_easing(mut self, easing: impl Fn(f32) -> f32 + 'static) -> Self {
        self.easing = Rc::new(easing);
        self
    }

    /// Returns the duration of this motion.
    pub fn duration(&self) -> Duration {
        self.duration
    }
}

impl From<Duration> for MotionInfo {
    fn from(duration: Duration) -> Self {
        Self::new(duration)
    }
}

impl From<f32> for MotionInfo {
    fn from(seconds: f32) -> Self {
        Self::new(seconds)
    }
}

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
    use crate::{
        AbsoluteLength, DefiniteLength, InteractiveElement as _, Length, Style, div, ease_in_out,
        px,
    };

    #[test]
    fn f32_motion_duration_is_seconds() {
        assert!((MotionInfo::new(0.2).duration().as_secs_f32() - 0.2).abs() < f32::EPSILON);
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
    fn generated_api_accepts_the_documented_motion_forms() {
        let transitions = StyleTransitions::new()
            .w(MotionInfo::new(Duration::from_millis(200)).with_easing(ease_in_out))
            .bg(0.5)
            .border_color(MotionInfo::new(0.6));

        assert_eq!(
            transitions.w.unwrap().duration(),
            Duration::from_millis(200)
        );
        assert!((transitions.bg.unwrap().duration().as_secs_f32() - 0.5).abs() < f32::EPSILON);
        assert!(
            (transitions.border_color.unwrap().duration().as_secs_f32() - 0.6).abs() < f32::EPSILON
        );

        let mut built = false;
        let _ = div().transitions(|transitions| {
            built = true;
            transitions.opacity(0.1)
        });
        assert!(built);
    }

    #[test]
    fn generated_width_transition_updates_the_resolved_style() {
        fn width(value: f32) -> Length {
            Length::Definite(DefiniteLength::Absolute(AbsoluteLength::Pixels(px(value))))
        }

        let transitions = StyleTransitions::new().w(1.0);
        let mut state = StyleTransitionState::default();
        let started_at = Instant::now();
        let mut style = Style {
            size: crate::size(width(10.0), Length::Auto),
            ..Style::default()
        };

        assert!(!transitions.apply_at(&mut style, &mut state, started_at, false));
        assert_eq!(style.size.width, width(10.0));

        style.size.width = width(20.0);
        assert!(transitions.apply_at(&mut style, &mut state, started_at, false));
        assert_eq!(style.size.width, width(10.0));

        style.size.width = width(20.0);
        assert!(transitions.apply_at(
            &mut style,
            &mut state,
            started_at + Duration::from_millis(500),
            false,
        ));
        assert_eq!(style.size.width, width(15.0));

        style.size.width = width(20.0);
        assert!(!transitions.apply_at(
            &mut style,
            &mut state,
            started_at + Duration::from_secs(1),
            false,
        ));
        assert_eq!(style.size.width, width(20.0));
    }
}
