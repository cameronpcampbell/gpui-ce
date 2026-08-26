use std::{rc::Rc, time::Duration};

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
        debug_assert!(
            self.is_finite() && self >= 0.0,
            "motion duration must be a finite, non-negative number of seconds"
        );
        Duration::from_secs_f32(self)
    }
}

/// Timing and easing configuration for motion.
#[derive(Clone)]
pub struct MotionInfo {
    /// How long the motion runs.
    pub duration: Duration,
    /// Maps linear progress to eased progress.
    pub easing: Rc<dyn Fn(f32) -> f32>,
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
