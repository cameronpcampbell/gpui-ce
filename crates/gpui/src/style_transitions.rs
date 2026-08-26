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
    initialized: bool,
    start: Option<T>,
    target: Option<T>,
    started_at: Option<Instant>,
}

impl<T> Default for StyleTransitionPropertyState<T> {
    fn default() -> Self {
        Self {
            initialized: false,
            start: None,
            target: None,
            started_at: None,
        }
    }
}

impl<T> StyleTransitionPropertyState<T>
where
    T: Lerp + Clone + PartialEq,
{
    pub(crate) fn evaluate(
        &mut self,
        target: Option<T>,
        motion: &MotionInfo,
        now: Instant,
        reduce_motion: bool,
    ) -> (Option<T>, bool) {
        if !self.initialized {
            self.initialized = true;
            self.start = target.clone();
            self.target = target.clone();
            return (target, false);
        }

        if reduce_motion {
            self.start = target.clone();
            self.target = target.clone();
            self.started_at = None;
            return (target, false);
        }

        if self.target != target {
            let current = self.value_at(motion, now).0;
            self.start = current;
            self.target = target;

            if motion.duration.is_zero()
                || self.start.is_none()
                || self.target.is_none()
                || self.start == self.target
            {
                self.start = self.target.clone();
                self.started_at = None;
            } else {
                self.started_at = Some(now);
            }
        }

        self.value_at(motion, now)
    }

    fn value_at(&mut self, motion: &MotionInfo, now: Instant) -> (Option<T>, bool) {
        let Some(started_at) = self.started_at else {
            return (self.target.clone(), false);
        };

        let duration = motion.duration.as_secs_f32();
        if duration == 0.0 {
            self.start = self.target.clone();
            self.started_at = None;
            return (self.target.clone(), false);
        }

        let linear_delta = now.saturating_duration_since(started_at).as_secs_f32() / duration;
        if linear_delta >= 1.0 {
            self.start = self.target.clone();
            self.started_at = None;
            return (self.target.clone(), false);
        }

        let eased_delta = (motion.easing)(linear_delta.clamp(0.0, 1.0));
        debug_assert!(
            (0.0..=1.0).contains(&eased_delta),
            "style transition easing must return a value between 0 and 1"
        );

        match (self.start.as_ref(), self.target.as_ref()) {
            (Some(start), Some(target)) => {
                (Some(start.lerp(target, eased_delta.clamp(0.0, 1.0))), true)
            }
            _ => (self.target.clone(), false),
        }
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
        let mut state = StyleTransitionPropertyState::default();

        assert_eq!(
            state.evaluate(Some(0.0_f32), &motion, started_at, false),
            (Some(0.0), false)
        );
        assert_eq!(
            state.evaluate(Some(10.0), &motion, started_at, false),
            (Some(0.0), true)
        );
        assert_eq!(
            state.evaluate(
                Some(20.0),
                &motion,
                started_at + Duration::from_millis(500),
                false,
            ),
            (Some(5.0), true)
        );
        assert_eq!(
            state.evaluate(
                Some(20.0),
                &motion,
                started_at + Duration::from_millis(1_000),
                false,
            ),
            (Some(12.5), true)
        );
    }

    #[test]
    fn reduced_motion_applies_the_target_immediately() {
        let motion = MotionInfo::new(1.0);
        let started_at = Instant::now();
        let mut state = StyleTransitionPropertyState::default();

        state.evaluate(Some(0.0_f32), &motion, started_at, false);
        assert_eq!(
            state.evaluate(Some(10.0), &motion, started_at, true),
            (Some(10.0), false)
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
