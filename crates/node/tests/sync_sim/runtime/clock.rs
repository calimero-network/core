//! Simulated logical clock for deterministic time progression.
//!
//! See spec §5 - Deterministic Scheduling.

use std::cmp::Ordering;
use std::fmt;
use std::ops::{Add, AddAssign, Sub};

/// Simulated time in microseconds.
///
/// Using microseconds provides sufficient resolution for network simulation
/// while avoiding floating-point imprecision.
#[derive(Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SimTime(u64);

impl SimTime {
    /// Zero time (simulation start).
    pub const ZERO: SimTime = SimTime(0);

    /// Maximum representable time.
    pub const MAX: SimTime = SimTime(u64::MAX);

    /// Create from microseconds.
    #[must_use]
    pub const fn from_micros(micros: u64) -> Self {
        Self(micros)
    }

    /// Create from milliseconds.
    #[must_use]
    pub const fn from_millis(millis: u64) -> Self {
        Self(millis.saturating_mul(1000))
    }

    /// Create from seconds.
    #[must_use]
    pub const fn from_secs(secs: u64) -> Self {
        Self(secs.saturating_mul(1_000_000))
    }

    /// Get time in microseconds.
    #[must_use]
    pub const fn as_micros(&self) -> u64 {
        self.0
    }

    /// Get time in milliseconds.
    #[must_use]
    pub const fn as_millis(&self) -> u64 {
        self.0 / 1000
    }

    /// Get time in seconds (truncated).
    #[must_use]
    pub const fn as_secs(&self) -> u64 {
        self.0 / 1_000_000
    }

    /// Saturating addition.
    #[must_use]
    pub const fn saturating_add(self, duration: SimDuration) -> Self {
        Self(self.0.saturating_add(duration.0))
    }

    /// Saturating subtraction.
    #[must_use]
    pub const fn saturating_sub(self, other: Self) -> SimDuration {
        SimDuration(self.0.saturating_sub(other.0))
    }
}

impl Add<SimDuration> for SimTime {
    type Output = SimTime;

    fn add(self, rhs: SimDuration) -> Self::Output {
        Self(self.0.saturating_add(rhs.0))
    }
}

impl AddAssign<SimDuration> for SimTime {
    fn add_assign(&mut self, rhs: SimDuration) {
        self.0 = self.0.saturating_add(rhs.0);
    }
}

impl Sub for SimTime {
    type Output = SimDuration;

    fn sub(self, rhs: Self) -> Self::Output {
        SimDuration(self.0.saturating_sub(rhs.0))
    }
}

impl fmt::Debug for SimTime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "SimTime({}ms)", self.as_millis())
    }
}

impl fmt::Display for SimTime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let millis = self.as_millis();
        if millis < 1000 {
            write!(f, "{}ms", millis)
        } else {
            write!(f, "{:.2}s", millis as f64 / 1000.0)
        }
    }
}

/// Simulated duration in microseconds.
#[derive(Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SimDuration(u64);

impl SimDuration {
    /// Zero duration.
    pub const ZERO: SimDuration = SimDuration(0);

    /// Create from microseconds.
    #[must_use]
    pub const fn from_micros(micros: u64) -> Self {
        Self(micros)
    }

    /// Create from milliseconds.
    #[must_use]
    pub const fn from_millis(millis: u64) -> Self {
        Self(millis.saturating_mul(1000))
    }

    /// Create from seconds.
    #[must_use]
    pub const fn from_secs(secs: u64) -> Self {
        Self(secs.saturating_mul(1_000_000))
    }

    /// Get duration in microseconds.
    #[must_use]
    pub const fn as_micros(&self) -> u64 {
        self.0
    }

    /// Get duration in milliseconds.
    #[must_use]
    pub const fn as_millis(&self) -> u64 {
        self.0 / 1000
    }

    /// Get duration in seconds (truncated).
    #[must_use]
    pub const fn as_secs(&self) -> u64 {
        self.0 / 1_000_000
    }

    /// Check if zero.
    #[must_use]
    pub const fn is_zero(&self) -> bool {
        self.0 == 0
    }
}

impl Add for SimDuration {
    type Output = SimDuration;

    fn add(self, rhs: Self) -> Self::Output {
        Self(self.0.saturating_add(rhs.0))
    }
}

impl AddAssign for SimDuration {
    fn add_assign(&mut self, rhs: Self) {
        self.0 = self.0.saturating_add(rhs.0);
    }
}

impl fmt::Debug for SimDuration {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "SimDuration({}ms)", self.as_millis())
    }
}

impl fmt::Display for SimDuration {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let millis = self.as_millis();
        if millis < 1000 {
            write!(f, "{}ms", millis)
        } else {
            write!(f, "{:.2}s", millis as f64 / 1000.0)
        }
    }
}

/// Logical clock for simulation.
///
/// Single-threaded, advances only when explicitly told to.
#[derive(Debug, Clone)]
pub struct SimClock {
    /// Current simulation time.
    now: SimTime,
}

impl SimClock {
    /// Create a new clock at time zero.
    #[must_use]
    pub fn new() -> Self {
        Self { now: SimTime::ZERO }
    }

    /// Get current time.
    #[must_use]
    pub fn now(&self) -> SimTime {
        self.now
    }

    /// Advance clock to a specific time.
    ///
    /// # Panics
    /// Panics if `to` is before current time (time cannot go backwards).
    pub fn advance_to(&mut self, to: SimTime) {
        assert!(
            to >= self.now,
            "Cannot advance clock backwards: {} -> {}",
            self.now,
            to
        );
        self.now = to;
    }

    /// Advance clock by a duration.
    pub fn advance_by(&mut self, duration: SimDuration) {
        self.now += duration;
    }
}

impl Default for SimClock {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sim_time_arithmetic() {
        let t1 = SimTime::from_millis(100);
        let t2 = SimTime::from_millis(250);
        let d = SimDuration::from_millis(50);

        assert_eq!(t1 + d, SimTime::from_millis(150));
        assert_eq!(t2 - t1, SimDuration::from_millis(150));
    }

    #[test]
    fn test_sim_time_ordering() {
        let t1 = SimTime::from_millis(100);
        let t2 = SimTime::from_millis(200);

        assert!(t1 < t2);
        assert!(t2 > t1);
        assert_eq!(t1.cmp(&t1), Ordering::Equal);
    }

    #[test]
    fn test_sim_duration_conversions() {
        let d = SimDuration::from_secs(2);
        assert_eq!(d.as_secs(), 2);
        assert_eq!(d.as_millis(), 2000);
        assert_eq!(d.as_micros(), 2_000_000);
    }

    #[test]
    fn test_clock_advance() {
        let mut clock = SimClock::new();
        assert_eq!(clock.now(), SimTime::ZERO);

        clock.advance_by(SimDuration::from_millis(100));
        assert_eq!(clock.now(), SimTime::from_millis(100));

        clock.advance_to(SimTime::from_millis(500));
        assert_eq!(clock.now(), SimTime::from_millis(500));
    }

    #[test]
    #[should_panic(expected = "Cannot advance clock backwards")]
    fn test_clock_cannot_go_backwards() {
        let mut clock = SimClock::new();
        clock.advance_to(SimTime::from_millis(100));
        clock.advance_to(SimTime::from_millis(50)); // Panic!
    }

    #[test]
    fn test_saturating_arithmetic() {
        let t = SimTime::MAX;
        let d = SimDuration::from_millis(1);
        assert_eq!(t.saturating_add(d), SimTime::MAX);

        let t = SimTime::ZERO;
        let t2 = SimTime::from_millis(100);
        assert_eq!(t.saturating_sub(t2), SimDuration::ZERO);
    }
}
