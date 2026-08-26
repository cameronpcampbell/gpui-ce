use std::{rc::Rc, time::Duration};

/// Creates motion information from a duration and an easing function.
pub trait DurationWithEasing {
    /// Creates motion information with this duration and the supplied easing function.
    fn with_easing(self, easing: impl Fn(f32) -> f32 + 'static) -> MotionInfo;
}

impl DurationWithEasing for Duration {
    fn with_easing(self, easing: impl Fn(f32) -> f32 + 'static) -> MotionInfo {
        MotionInfo::new(self).with_easing(easing)
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
    pub fn new(duration: Duration) -> Self {
        Self {
            duration,
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
