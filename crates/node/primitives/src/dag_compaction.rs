//! Configuration for DAG compaction (issue #2026).
//!
//! Every regular delta a context applies is persisted forever in the delta
//! column, so the on-disk DAG log grows linearly with lifetime transaction
//! count. Compaction bounds that growth: once a context accumulates enough
//! delta rows, the oldest history is pruned outright, retaining only a recent
//! window for cheap incremental delta catch-up. (No checkpoint object is
//! written — pruned deltas simply become absent.) Anything older converges via
//! HashComparison,
//! which reconciles current state without consulting the delta log — so the
//! pruned history is never needed for correctness, only as an optimization.

use core::time::Duration;

use serde::{Deserialize, Serialize};

/// Default number of delta rows a context must hold before compaction is
/// eligible. Aligned with the in-memory `MAX_TOPOLOGY_ENTRIES` cap (10k) in
/// the delta store, so on-disk pruning and in-memory ancestry eviction kick
/// in at the same threshold. The gap to [`DEFAULT_RETAIN_RECENT_COUNT`] (10:1)
/// gives hysteresis: after compacting down to the retain window a context does
/// not become eligible again until it has grown another ~9k deltas.
pub const DEFAULT_MIN_DELTAS_BEFORE_COMPACT: usize = 10_000;

/// Default number of most-recent deltas to keep after compaction. Kept at or
/// below `MAX_DELTA_QUERY_LIMIT` (3000) so a behind peer can still catch up on
/// the retained window in a single delta-sync round; beyond that, replaying
/// the op log stops being cheaper than HashComparison anyway.
pub const DEFAULT_RETAIN_RECENT_COUNT: usize = 1_000;

/// Default interval between compaction sweeps. The per-context eligibility
/// check (count delta rows) is cheap, and DAG growth between sweeps is bounded
/// by op throughput, so an hour bounds worst-case inter-sweep bloat without
/// busy-work. Mirrors the cadence story of the tombstone `GarbageCollector`.
pub const DEFAULT_CHECK_INTERVAL_SECS: u64 = 3_600;

/// Operator-tunable DAG compaction settings (`[dag_compaction]`).
///
/// Compaction is **enabled by default** with conservative thresholds. It is
/// safe to prune because deltas are an optimization, not a convergence
/// requirement: a peer that requests a pruned delta gets "not found" and the
/// next sync round reconciles via HashComparison — already the protocol
/// selected for a diverged initialized node — which converges current state
/// without consulting the delta log. Operators can still set `enabled = false`
/// to opt out.
#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[non_exhaustive]
pub struct DagCompactionConfig {
    /// Whether periodic DAG compaction runs at all.
    #[serde(default = "default_enabled")]
    pub enabled: bool,

    /// Minimum delta rows a context must hold before it is eligible for
    /// compaction.
    #[serde(default = "default_min_deltas_before_compact")]
    pub min_deltas_before_compact: usize,

    /// Number of most-recent deltas to retain after pruning.
    #[serde(default = "default_retain_recent_count")]
    pub retain_recent_count: usize,

    /// Interval between compaction sweeps.
    #[serde(default = "default_check_interval", with = "duration_secs")]
    pub check_interval: Duration,
}

impl Default for DagCompactionConfig {
    fn default() -> Self {
        Self {
            enabled: default_enabled(),
            min_deltas_before_compact: DEFAULT_MIN_DELTAS_BEFORE_COMPACT,
            retain_recent_count: DEFAULT_RETAIN_RECENT_COUNT,
            check_interval: default_check_interval(),
        }
    }
}

impl DagCompactionConfig {
    /// Returns `true` when the configuration is internally consistent enough
    /// to run:
    ///
    /// * `retain_recent_count` must be strictly below
    ///   `min_deltas_before_compact`, otherwise a sweep would either do no
    ///   work or re-trigger immediately (no hysteresis); and
    /// * `check_interval` must be non-zero — a zero interval would turn
    ///   `run_interval` into a busy-loop spawning sweeps as fast as the
    ///   mailbox allows.
    #[must_use]
    pub fn is_valid(&self) -> bool {
        self.retain_recent_count < self.min_deltas_before_compact && !self.check_interval.is_zero()
    }
}

const fn default_enabled() -> bool {
    true
}

const fn default_min_deltas_before_compact() -> usize {
    DEFAULT_MIN_DELTAS_BEFORE_COMPACT
}

const fn default_retain_recent_count() -> usize {
    DEFAULT_RETAIN_RECENT_COUNT
}

const fn default_check_interval() -> Duration {
    Duration::from_secs(DEFAULT_CHECK_INTERVAL_SECS)
}

/// Serialize/deserialize a [`Duration`] as whole seconds, matching how other
/// interval-style config values are expressed in `config.toml`.
mod duration_secs {
    use core::time::Duration;

    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(value: &Duration, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_u64(value.as_secs())
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Duration, D::Error> {
        let secs = u64::deserialize(deserializer)?;
        Ok(Duration::from_secs(secs))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_enabled_with_sane_thresholds() {
        let cfg = DagCompactionConfig::default();
        assert!(cfg.enabled);
        assert_eq!(
            cfg.min_deltas_before_compact,
            DEFAULT_MIN_DELTAS_BEFORE_COMPACT
        );
        assert_eq!(cfg.retain_recent_count, DEFAULT_RETAIN_RECENT_COUNT);
        assert_eq!(cfg.check_interval.as_secs(), DEFAULT_CHECK_INTERVAL_SECS);
        assert!(cfg.is_valid());
    }

    #[test]
    fn retain_at_or_above_min_is_invalid() {
        let mut cfg = DagCompactionConfig::default();
        cfg.retain_recent_count = cfg.min_deltas_before_compact;
        assert!(!cfg.is_valid());
        cfg.retain_recent_count = cfg.min_deltas_before_compact + 1;
        assert!(!cfg.is_valid());
    }

    #[test]
    fn zero_check_interval_is_invalid() {
        let cfg = DagCompactionConfig {
            check_interval: Duration::ZERO,
            ..DagCompactionConfig::default()
        };
        assert!(!cfg.is_valid());
    }

    #[test]
    fn absent_fields_fall_back_to_defaults() {
        // An empty object must deserialize to the enabled default — every
        // field has an explicit `#[serde(default = ...)]`, so an omitted
        // section behaves identically to a fully-defaulted struct.
        let cfg: DagCompactionConfig = serde_json::from_str("{}").expect("empty object");
        assert!(cfg.enabled);
        assert_eq!(
            cfg.min_deltas_before_compact,
            DEFAULT_MIN_DELTAS_BEFORE_COMPACT
        );
        assert_eq!(cfg.check_interval.as_secs(), DEFAULT_CHECK_INTERVAL_SECS);
    }

    #[test]
    fn explicit_disable_is_respected() {
        let cfg: DagCompactionConfig =
            serde_json::from_str(r#"{"enabled": false}"#).expect("explicit disable");
        assert!(!cfg.enabled);
    }

    #[test]
    fn check_interval_round_trips_as_seconds() {
        let cfg = DagCompactionConfig {
            enabled: true,
            min_deltas_before_compact: 5000,
            retain_recent_count: 500,
            check_interval: Duration::from_secs(900),
        };
        let json = serde_json::to_string(&cfg).expect("serialize");
        // The duration is encoded as whole seconds, not a struct.
        assert!(json.contains("\"check_interval\":900"));
        let back: DagCompactionConfig = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.check_interval.as_secs(), 900);
        assert!(back.enabled);
        assert!(back.is_valid());
    }
}
