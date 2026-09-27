use std::{
    ops::{Deref, DerefMut, Sub},
    rc::Rc,
    time::Duration,
};

use crate::spring::DEFAULT_SPRING_EPSILON;
use crate::{SpringAnimation, SpringConfig, SpringState, SpringTarget};

/// Creates a duration from a number of whole seconds.
pub const fn secs(seconds: u64) -> Duration {
    Duration::from_secs(seconds)
}

/// Creates a duration from a number of whole milliseconds.
pub const fn millis(milliseconds: u64) -> Duration {
    Duration::from_millis(milliseconds)
}

/// Animation progress is normalized before easing and may overshoot afterward.
#[derive(Clone, Copy, Debug, Default, PartialEq, PartialOrd)]
pub struct Progress(f32);

impl Progress {
    /// The beginning of an animation.
    pub const START: Self = Self(0.0);

    /// The end of an animation.
    pub const END: Self = Self(1.0);

    /// Returns progress clamped to the normalized range.
    pub fn clamped(value: f32) -> Self {
        assert!(!value.is_nan(), "progress must not be NaN");
        Self(value.clamp(Self::START.0, Self::END.0))
    }

    /// Returns the underlying progress value.
    pub const fn get(self) -> f32 {
        self.0
    }

    /// Returns whether the value is at least one; use [`MotionSample::is_active`]
    /// to check if motion has finished.
    pub const fn is_complete(self) -> bool {
        self.0 >= Self::END.0
    }

    fn eased(value: f32) -> Self {
        assert!(value.is_finite(), "easing must return a finite value");
        Self(value)
    }
}

/// Creates duration-based motion with easing or a sampled spring.
pub trait MotionDurationExt {
    /// Creates motion with this duration and the supplied easing function.
    fn with_easing(self, easing: impl Fn(f32) -> f32 + 'static) -> Motion;

    /// Samples a spring over this duration, ending when the duration expires.
    fn with_spring(self, config: SpringConfig) -> Motion;
}

impl MotionDurationExt for Duration {
    fn with_easing(self, easing: impl Fn(f32) -> f32 + 'static) -> Motion {
        Motion::new(self).with_easing(easing)
    }

    fn with_spring(self, config: SpringConfig) -> Motion {
        Motion::new(self).with_spring(config)
    }
}

/// Maps linear progress to eased progress.
#[derive(Clone)]
pub struct Easing(Rc<dyn Fn(f32) -> f32>);

impl Easing {
    /// Creates an easing function.
    pub fn new(easing: impl Fn(f32) -> f32 + 'static) -> Self {
        Self(Rc::new(easing))
    }

    /// Evaluates normalized progress without clamping the eased result.
    pub fn sample(&self, progress: Progress) -> Progress {
        Progress::eased((self.0)(progress.get()))
    }
}

impl Default for Easing {
    fn default() -> Self {
        Self::new(crate::linear)
    }
}

/// Whether motion runs once or repeats indefinitely.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Repeat {
    /// Run once.
    #[default]
    Once,

    /// Repeat and remain active until the owner removes the animation.
    Forever,
}

/// The result of evaluating motion at a point in time.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MotionSample {
    /// Eased progress, which may overshoot zero through one.
    pub progress: Progress,

    /// Whether another sample may produce a different value.
    pub is_active: bool,
}

/// Configuration for motion driven by a fixed duration.
#[derive(Clone)]
pub struct DurationDescription {
    /// How long this motion takes.
    pub duration: Duration,

    /// Maps linear progress to eased progress.
    pub easing: Easing,

    /// Whether this motion runs once or forever.
    pub repeat: Repeat,
}

/// Configuration for motion driven by a settling spring.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SpringDescription {
    /// The spring's physical parameters.
    pub(crate) config: SpringConfig,

    /// The distance and velocity threshold for settling.
    pub(crate) epsilon: f32,

    settle_after: Duration,
}

/// A motion with methods determined by its description type.
#[derive(Clone, Debug)]
pub struct Motion<Description = DurationDescription> {
    description: Description,
}

impl<Description> Deref for Motion<Description> {
    type Target = Description;

    fn deref(&self) -> &Self::Target {
        &self.description
    }
}

impl<Description> DerefMut for Motion<Description> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.description
    }
}

impl Motion<DurationDescription> {
    /// Creates one linear motion pass with the supplied duration.
    pub fn new(duration: Duration) -> Self {
        Self {
            description: DurationDescription {
                duration,
                easing: Easing::default(),
                repeat: Repeat::Once,
            },
        }
    }

    /// Replaces the linear easing function.
    pub fn with_easing(mut self, easing: impl Fn(f32) -> f32 + 'static) -> Self {
        self.easing = Easing::new(easing);
        self
    }

    /// Replaces easing with a spring sampled over this motion's duration.
    /// Use [`Motion::spring`] for a spring that settles and preserves velocity across retargets.
    pub fn with_spring(self, config: SpringConfig) -> Self {
        let duration = self.duration.as_secs_f32();
        let initial_state = SpringState {
            position: 0.0,
            velocity: 0.0,
        };

        self.with_easing(move |progress| {
            if progress <= 0.0 {
                0.0
            } else if progress >= 1.0 {
                1.0
            } else {
                config
                    .step(initial_state, 1.0, progress * duration)
                    .position
            }
        })
    }

    /// Evaluates this motion after the supplied elapsed time.
    pub fn sample(&self, elapsed: Duration) -> MotionSample {
        if self.duration.is_zero() {
            return MotionSample {
                progress: self.resting_progress(),
                is_active: false,
            };
        }

        let (linear_progress, is_active) = match self.repeat {
            Repeat::Once => {
                let progress =
                    Progress::clamped((elapsed.as_secs_f64() / self.duration.as_secs_f64()) as f32);
                (progress, !progress.is_complete())
            }
            Repeat::Forever => {
                let duration_nanos = self.duration.as_nanos();
                let elapsed_nanos = elapsed.as_nanos() % duration_nanos;
                let progress = elapsed_nanos as f64 / duration_nanos as f64;
                (Progress::clamped(progress as f32), true)
            }
        };

        MotionSample {
            progress: self.easing.sample(linear_progress),
            is_active,
        }
    }

    /// Evaluates this motion between two timestamps.
    pub fn sample_at<Time>(&self, started_at: Time, now: Time) -> MotionSample
    where
        Time: Sub<Time, Output = Duration>,
    {
        self.sample(now - started_at)
    }

    pub(crate) fn resting_progress(&self) -> Progress {
        match self.repeat {
            Repeat::Once => Progress::END,
            Repeat::Forever => Progress::START,
        }
    }
}

impl Motion<SpringDescription> {
    /// Creates a spring motion that runs until it settles.
    pub fn spring(config: SpringConfig) -> Self {
        let epsilon = DEFAULT_SPRING_EPSILON;

        Self {
            description: SpringDescription {
                config,
                epsilon,
                settle_after: config.settle_time(SpringState::default(), 1.0, epsilon),
            },
        }
    }

    /// Returns this spring's physical parameters.
    pub fn config(&self) -> SpringConfig {
        self.config
    }

    /// Returns this spring's settling tolerance.
    pub fn epsilon(&self) -> f32 {
        self.epsilon
    }

    /// Sets the spring's settling tolerance.
    pub fn with_epsilon(mut self, epsilon: f32) -> Self {
        self.description.epsilon = epsilon;
        self.description.settle_after =
            self.config
                .settle_time(SpringState::default(), 1.0, epsilon);
        self
    }

    /// Evaluates this spring after the supplied elapsed time, starting from rest at zero progress.
    pub fn sample(&self, elapsed: Duration) -> MotionSample {
        if elapsed >= self.settle_after {
            return MotionSample {
                progress: Progress::END,
                is_active: false,
            };
        }

        let state = self
            .config
            .step(SpringState::default(), 1.0, elapsed.as_secs_f32());

        MotionSample {
            progress: Progress::eased(state.position),
            is_active: true,
        }
    }

    /// Targets a value or projected path with this spring.
    pub fn to<T: SpringTarget>(self, target: T) -> SpringAnimation<T> {
        SpringAnimation {
            motion: self,
            target,
            initial: None,
            playback: crate::SpringPlayback::Running,
        }
    }
}

impl Default for Motion<DurationDescription> {
    fn default() -> Self {
        Self::new(Duration::ZERO)
    }
}

impl From<Duration> for Motion<DurationDescription> {
    fn from(duration: Duration) -> Self {
        Self::new(duration)
    }
}

impl From<SpringConfig> for Motion<SpringDescription> {
    fn from(config: SpringConfig) -> Self {
        Self::spring(config)
    }
}

/// A duration or spring motion that can be sampled through one interface.
/// Each spring sample starts from rest, so retargeting an animated value resets its velocity.
#[derive(Clone)]
pub enum AnyMotion {
    /// Motion that runs for a fixed duration.
    Duration(Motion<DurationDescription>),

    /// Motion that runs until its spring settles.
    Spring(Motion<SpringDescription>),
}

impl AnyMotion {
    /// Evaluates this motion after the supplied elapsed time.
    pub fn sample(&self, elapsed: Duration) -> MotionSample {
        match self {
            Self::Duration(motion) => motion.sample(elapsed),
            Self::Spring(motion) => motion.sample(elapsed),
        }
    }

    /// Evaluates this motion between two timestamps.
    pub fn sample_at<Time>(&self, started_at: Time, now: Time) -> MotionSample
    where
        Time: Sub<Time, Output = Duration>,
    {
        self.sample(now - started_at)
    }

    pub(crate) fn resting_progress(&self) -> Progress {
        match self {
            Self::Duration(motion) => motion.resting_progress(),
            Self::Spring(_) => Progress::END,
        }
    }
}

impl From<Motion<DurationDescription>> for AnyMotion {
    fn from(motion: Motion<DurationDescription>) -> Self {
        Self::Duration(motion)
    }
}

impl From<Motion<SpringDescription>> for AnyMotion {
    fn from(motion: Motion<SpringDescription>) -> Self {
        Self::Spring(motion)
    }
}

impl From<Duration> for AnyMotion {
    fn from(duration: Duration) -> Self {
        Self::Duration(duration.into())
    }
}

impl From<SpringConfig> for AnyMotion {
    fn from(config: SpringConfig) -> Self {
        Self::Spring(config.into())
    }
}

impl From<&Motion<DurationDescription>> for AnyMotion {
    fn from(motion: &Motion<DurationDescription>) -> Self {
        Self::Duration(motion.clone())
    }
}

impl From<&Motion<SpringDescription>> for AnyMotion {
    fn from(motion: &Motion<SpringDescription>) -> Self {
        Self::Spring(motion.clone())
    }
}

impl From<&AnyMotion> for AnyMotion {
    fn from(motion: &AnyMotion) -> Self {
        motion.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creates_durations() {
        assert_eq!(secs(2), Duration::from_secs(2));
        assert_eq!(millis(250), Duration::from_millis(250));
    }

    #[test]
    fn samples_one_shot_and_eased_motion() {
        let motion = Duration::from_secs(2).with_easing(|progress| progress * progress);

        let cases = [
            (
                Duration::from_secs(1),
                MotionSample {
                    progress: Progress::clamped(0.25),
                    is_active: true,
                },
            ),
            (
                Duration::from_secs(3),
                MotionSample {
                    progress: Progress::END,
                    is_active: false,
                },
            ),
        ];

        for (elapsed, expected) in cases {
            assert_eq!(motion.sample(elapsed), expected);
        }

        assert_eq!(
            motion.sample_at(Duration::from_secs(3), Duration::from_secs(5)),
            MotionSample {
                progress: Progress::END,
                is_active: false,
            }
        );

        assert_eq!(Progress::clamped(-1.0), Progress::START);
        assert_eq!(Progress::clamped(2.0), Progress::END);

        let spring = Duration::from_secs(1).with_spring(SpringConfig::new(100.0, 6.0, 1.0));

        assert_eq!(spring.sample(Duration::ZERO).progress, Progress::START);
        assert_eq!(
            spring.sample(Duration::from_secs(1)).progress,
            Progress::END
        );
        assert!((1..100).any(|step| {
            spring
                .sample(Duration::from_millis(step * 10))
                .progress
                .get()
                > 1.0
        }));

        let config = SpringConfig::new(100.0, 6.0, 1.0);
        let native_spring: AnyMotion = config.into();
        let loose_spring = Motion::spring(config).with_epsilon(0.1);
        let cutoff = loose_spring.settle_after;

        assert!(native_spring.sample(cutoff).is_active);
        assert_eq!(
            AnyMotion::from(loose_spring).sample_at(Duration::ZERO, cutoff),
            MotionSample {
                progress: Progress::END,
                is_active: false,
            }
        );
    }

    #[test]
    fn repeating_and_zero_duration_motion_use_their_resting_progress() {
        let once = Motion::new(Duration::ZERO).sample(Duration::from_secs(10));
        assert_eq!(once.progress, Progress::END);
        assert!(!once.is_active);

        let mut repeating = Motion::new(Duration::ZERO);
        repeating.repeat = Repeat::Forever;
        let sample = repeating.sample(Duration::from_secs(10));
        assert_eq!(sample.progress, Progress::START);
        assert!(!sample.is_active);

        let duration = Duration::from_secs(1);
        let mut motion = Motion::new(duration);
        motion.repeat = Repeat::Forever;

        assert_eq!(
            motion.sample(Duration::from_millis(250)),
            MotionSample {
                progress: Progress::clamped(0.25),
                is_active: true,
            }
        );
        assert_eq!(motion.sample(duration).progress, Progress::START);
        assert_eq!(
            motion.sample(duration * 2 + Duration::from_millis(500)),
            MotionSample {
                progress: Progress::clamped(0.5),
                is_active: true,
            }
        );

        let long_elapsed = Duration::from_secs(300 * 24 * 60 * 60) + Duration::from_millis(250);
        assert_eq!(
            motion.sample(long_elapsed).progress,
            Progress::clamped(0.25)
        );
    }
}
