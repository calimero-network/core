//! Tombstone garbage-collection settings (`[gc]`).

use core::time::Duration;

use serde::{Deserialize, Serialize};

/// Default interval between tombstone GC sweeps, in seconds.
///
/// A sweep reads every state row of every context looking for tombstones, so
/// its cost scales with the node's state, not with how much was deleted; the
/// interval trades that recurring scan against how long a collectable
/// tombstone lingers. A tombstone goes on the first sweep after the one that
/// noted it once every member has caught up, so it lives about one to two
/// intervals past that point. Ten minutes keeps that short without scanning
/// a large store more than a few times an hour; `gc_sweep_duration_seconds`
/// and `gc_rows_scanned_total` show what a sweep costs on a given node.
pub const DEFAULT_GC_CHECK_INTERVAL_SECS: u64 = 600;

/// Operator-tunable tombstone GC settings (`[gc]`).
///
/// Only the cadence is tunable. When a tombstone may go is not: it is
/// collected once every member device of its context has applied the delete,
/// never on a timer, so a shorter interval reclaims sooner but never earlier
/// than that.
#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[non_exhaustive]
pub struct GcConfig {
    /// Interval between sweeps, in seconds.
    #[serde(default = "default_check_interval", with = "duration_secs")]
    pub check_interval: Duration,
}

impl Default for GcConfig {
    fn default() -> Self {
        Self {
            check_interval: default_check_interval(),
        }
    }
}

impl GcConfig {
    /// A config sweeping every `check_interval`.
    #[must_use]
    pub const fn with_check_interval(check_interval: Duration) -> Self {
        Self { check_interval }
    }

    /// `false` for a zero interval, which would turn the sweep timer into a
    /// busy loop.
    #[must_use]
    pub const fn is_valid(&self) -> bool {
        !self.check_interval.is_zero()
    }
}

const fn default_check_interval() -> Duration {
    Duration::from_secs(DEFAULT_GC_CHECK_INTERVAL_SECS)
}

/// Serialize/deserialize a [`Duration`] as whole seconds, like the other
/// interval settings in `config.toml`.
mod duration_secs {
    use core::time::Duration;

    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(value: &Duration, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_u64(value.as_secs())
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Duration, D::Error> {
        Ok(Duration::from_secs(u64::deserialize(deserializer)?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_absent_section_sweeps_every_ten_minutes() {
        let cfg: GcConfig = toml::from_str("").unwrap();
        assert_eq!(cfg.check_interval, Duration::from_secs(600));
        assert!(cfg.is_valid());
    }

    #[test]
    fn the_interval_is_read_in_seconds() {
        let cfg: GcConfig = toml::from_str("check_interval = 300").unwrap();
        assert_eq!(cfg.check_interval, Duration::from_secs(300));
        let back: GcConfig = toml::from_str(&toml::to_string(&cfg).unwrap()).unwrap();
        assert_eq!(back.check_interval, cfg.check_interval);
    }

    #[test]
    fn a_zero_interval_is_refused() {
        let cfg: GcConfig = toml::from_str("check_interval = 0").unwrap();
        assert!(!cfg.is_valid());
    }
}
