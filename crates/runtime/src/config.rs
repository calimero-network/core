//! Operator-tunable runtime configuration.
//!
//! Surfaced as the `[runtime]` section of the node config file and threaded
//! through to the engine that runs guest WASM. Today it carries only the
//! subset of [`VMLimits`](crate::logic::VMLimits) that operators have a
//! demonstrated need to tune (the per-execution log buffer), but it is shaped
//! so further limits can be exposed without reworking the config surface: add
//! a field to [`RuntimeLimitsConfig`] and overlay it in
//! [`RuntimeLimitsConfig::apply`].

use serde::{Deserialize, Serialize};

use crate::logic::VMLimits;

/// The `[runtime]` config section.
///
/// Every field defaults, so an absent `[runtime]` section (or any absent
/// sub-key) leaves the built-in [`VMLimits::default`] behavior untouched.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize)]
#[non_exhaustive]
pub struct RuntimeConfig {
    /// Per-execution VM resource limits (`[runtime.limits]`).
    #[serde(default)]
    pub limits: RuntimeLimitsConfig,
}

impl RuntimeConfig {
    /// Resolve the configured overrides into a concrete [`VMLimits`], falling
    /// back to [`VMLimits::default`] for every field left unset.
    #[must_use]
    pub fn vm_limits(&self) -> VMLimits {
        self.limits.apply(VMLimits::default())
    }
}

/// Operator overrides for [`VMLimits`] (`[runtime.limits]`).
///
/// Each field is optional: an unset field keeps the corresponding built-in
/// default rather than forcing operators to restate every limit. The two
/// fields exposed here are the ones with conflicting operational pulls —
/// resource-constrained deployments want them lower, verbose-tracing debug
/// sessions want them higher — which the hardcoded defaults could not satisfy
/// without recompilation.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize)]
#[non_exhaustive]
pub struct RuntimeLimitsConfig {
    /// Override for [`VMLimits::max_logs`]: the maximum number of log entries
    /// a single execution may emit. Exceeding it traps the execution with
    /// `LogsOverflow`. Raise it for verbose `tracing` runs; lower it to bound
    /// the transient per-execution log buffer under memory pressure.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_logs: Option<u64>,
    /// Override for [`VMLimits::max_log_size`]: the maximum size, in bytes, of
    /// a single log line. An over-long line traps the execution rather than
    /// truncating, so raise this (not just `max_logs`) when dependency-crate
    /// `tracing` emits long lines.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_log_size: Option<u64>,
}

impl RuntimeLimitsConfig {
    /// Overlay the configured overrides onto `base`, leaving unset fields
    /// untouched. `base` is typically [`VMLimits::default`].
    #[must_use]
    pub fn apply(&self, mut base: VMLimits) -> VMLimits {
        if let Some(max_logs) = self.max_logs {
            base.max_logs = max_logs;
        }
        if let Some(max_log_size) = self.max_log_size {
            base.max_log_size = max_log_size;
        }
        base
    }
}

#[cfg(test)]
mod tests {
    use super::{RuntimeConfig, RuntimeLimitsConfig};
    use crate::logic::VMLimits;

    #[test]
    fn empty_config_matches_defaults() {
        let defaults = VMLimits::default();
        let resolved = RuntimeConfig::default().vm_limits();
        assert_eq!(resolved.max_logs, defaults.max_logs);
        assert_eq!(resolved.max_log_size, defaults.max_log_size);
    }

    #[test]
    fn overrides_apply_and_leave_others_default() {
        let defaults = VMLimits::default();
        let cfg = RuntimeConfig {
            limits: RuntimeLimitsConfig {
                max_logs: Some(4096),
                max_log_size: None,
            },
        };
        let resolved = cfg.vm_limits();
        assert_eq!(resolved.max_logs, 4096);
        // Unset field falls back to the built-in default.
        assert_eq!(resolved.max_log_size, defaults.max_log_size);
        // Untouched limits are preserved.
        assert_eq!(resolved.max_events, defaults.max_events);
    }

    #[test]
    fn deserializes_runtime_limits_section() {
        let toml = r#"
            [limits]
            max_logs = 2048
            max_log_size = 32768
        "#;
        let cfg: RuntimeConfig = toml::from_str(toml).expect("valid config");
        let resolved = cfg.vm_limits();
        assert_eq!(resolved.max_logs, 2048);
        assert_eq!(resolved.max_log_size, 32768);
    }
}
