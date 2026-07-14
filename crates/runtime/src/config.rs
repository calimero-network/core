//! Operator-tunable runtime configuration.
//!
//! Surfaced as the `[runtime]` section of the node config file and threaded
//! through to the engine that runs guest WASM. Today it carries only the
//! subset of [`VMLimits`](crate::logic::VMLimits) that operators have a
//! demonstrated need to tune (the per-execution log buffer), but it is shaped
//! so further limits can be exposed without reworking the config surface: add
//! a field to [`RuntimeLimitsConfig`] and overlay it in
//! [`RuntimeLimitsConfig::apply`].

use eyre::{bail, Result as EyreResult};
use serde::{Deserialize, Serialize};

use crate::logic::VMLimits;

/// Upper bound accepted for `max_logs`. Past this, the worst-case log buffer
/// (`max_logs * max_log_size`) is large enough that the value is almost
/// certainly a typo or a misguided attempt to disable the limit, so we reject
/// it rather than let it exhaust node memory.
const MAX_LOGS_CEILING: u64 = 1_000_000;
/// Upper bound accepted for `max_log_size`, in bytes (10 MiB). Single log
/// lines are never legitimately this large; a value above it is treated as a
/// misconfiguration.
const MAX_LOG_SIZE_CEILING: u64 = 10 * 1024 * 1024;
/// Upper bound accepted for `max_precompiled_module_size`, in bytes (1 GiB).
/// A precompiled artifact larger than this is far outside any legitimate range
/// (source WASM is itself capped at a few MiB) and would defeat the purpose of
/// the cap, so it is treated as a misconfiguration.
const MAX_PRECOMPILED_MODULE_SIZE_CEILING: u64 = 1024 * 1024 * 1024;

/// The `[runtime]` config section.
///
/// Every field defaults, so an absent `[runtime]` section (or any absent
/// sub-key) leaves the built-in [`VMLimits::default`] behavior untouched.
///
/// Not `Copy` despite being small and currently all-`Copy`: `#[non_exhaustive]`
/// signals this will grow, and a future non-`Copy` field (a path, a list)
/// would make dropping the derive a breaking change. `Clone` covers every
/// current use.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[non_exhaustive]
pub struct RuntimeConfig {
    /// Per-execution VM resource limits (`[runtime.limits]`).
    #[serde(default)]
    pub limits: RuntimeLimitsConfig,
}

impl RuntimeConfig {
    /// Resolve the configured overrides into a concrete [`VMLimits`], falling
    /// back to [`VMLimits::default`] for every field left unset.
    ///
    /// Call [`RuntimeConfig::validate`] (e.g. at node startup) before relying
    /// on the result; this method itself does not reject out-of-range values.
    #[must_use]
    pub fn vm_limits(&self) -> VMLimits {
        self.limits.apply(VMLimits::default())
    }

    /// Reject operator misconfigurations before they reach the engine. See
    /// [`RuntimeLimitsConfig::validate`].
    pub fn validate(&self) -> EyreResult<()> {
        self.limits.validate()
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
///
/// **Memory budgeting:** the transient per-execution log buffer is bounded by
/// the *product* `max_logs * max_log_size` (worst case, held in the `Outcome`).
/// With the defaults that is ~16 MiB; raising both simultaneously multiplies
/// the ceiling, so tune them together with that product in mind.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
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
    /// Override for [`VMLimits::max_precompiled_module_size`]: the maximum size,
    /// in bytes, of a precompiled (serialized) module accepted by
    /// `Engine::from_precompiled`. Bounds deserialization input as
    /// defense-in-depth; lower it in memory-constrained deployments. Distinct
    /// from the source-WASM `max_module_size`, which guards a separate path and
    /// is not operator-tunable today.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_precompiled_module_size: Option<u64>,
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
        if let Some(max_precompiled_module_size) = self.max_precompiled_module_size {
            base.max_precompiled_module_size = max_precompiled_module_size;
        }
        base
    }

    /// Reject values large enough to be footguns rather than intent. An
    /// operator who sets `max_logs` near `u64::MAX` would let a single
    /// execution's log buffer exhaust node memory (worst case
    /// `max_logs * max_log_size`); a sanity ceiling turns that into a
    /// startup error instead of a runtime OOM. Unset fields always pass.
    pub fn validate(&self) -> EyreResult<()> {
        if let Some(max_logs) = self.max_logs {
            if max_logs > MAX_LOGS_CEILING {
                bail!(
                    "runtime.limits.max_logs = {max_logs} exceeds the maximum of {MAX_LOGS_CEILING}"
                );
            }
        }
        if let Some(max_log_size) = self.max_log_size {
            if max_log_size > MAX_LOG_SIZE_CEILING {
                bail!(
                    "runtime.limits.max_log_size = {max_log_size} exceeds the maximum of \
                     {MAX_LOG_SIZE_CEILING} bytes"
                );
            }
        }
        if let Some(max_precompiled_module_size) = self.max_precompiled_module_size {
            if max_precompiled_module_size > MAX_PRECOMPILED_MODULE_SIZE_CEILING {
                bail!(
                    "runtime.limits.max_precompiled_module_size = \
                     {max_precompiled_module_size} exceeds the maximum of \
                     {MAX_PRECOMPILED_MODULE_SIZE_CEILING} bytes"
                );
            }
        }
        Ok(())
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
                max_precompiled_module_size: None,
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
    fn precompiled_module_size_override_applies() {
        let defaults = VMLimits::default();
        let cfg = RuntimeConfig {
            limits: RuntimeLimitsConfig {
                max_logs: None,
                max_log_size: None,
                max_precompiled_module_size: Some(8 * 1024 * 1024),
            },
        };
        let resolved = cfg.vm_limits();
        assert_eq!(resolved.max_precompiled_module_size, 8 * 1024 * 1024);
        // The source-WASM cap is independent and stays at its default.
        assert_eq!(resolved.max_module_size, defaults.max_module_size);
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

    #[test]
    fn validate_accepts_defaults_and_reasonable_overrides() {
        RuntimeConfig::default()
            .validate()
            .expect("defaults are valid");

        let cfg = RuntimeConfig {
            limits: RuntimeLimitsConfig {
                max_logs: Some(4096),
                max_log_size: Some(64 * 1024),
                max_precompiled_module_size: Some(64 * 1024 * 1024),
            },
        };
        cfg.validate().expect("reasonable overrides are valid");
    }

    #[test]
    fn validate_rejects_out_of_range_values() {
        let cfg = RuntimeConfig {
            limits: RuntimeLimitsConfig {
                max_logs: Some(u64::MAX),
                max_log_size: None,
                max_precompiled_module_size: None,
            },
        };
        let err = cfg.validate().unwrap_err().to_string();
        assert!(err.contains("max_logs"), "unexpected error: {err}");

        let cfg = RuntimeConfig {
            limits: RuntimeLimitsConfig {
                max_logs: None,
                max_log_size: Some(u64::MAX),
                max_precompiled_module_size: None,
            },
        };
        let err = cfg.validate().unwrap_err().to_string();
        assert!(err.contains("max_log_size"), "unexpected error: {err}");

        let cfg = RuntimeConfig {
            limits: RuntimeLimitsConfig {
                max_logs: None,
                max_log_size: None,
                max_precompiled_module_size: Some(u64::MAX),
            },
        };
        let err = cfg.validate().unwrap_err().to_string();
        assert!(
            err.contains("max_precompiled_module_size"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn unknown_keys_are_ignored_not_rejected() {
        // The config is intentionally lenient (matching the rest of the node
        // config): an unknown/typo'd key is silently ignored rather than
        // failing the parse. This test pins that behavior so a future switch
        // to `deny_unknown_fields` is a conscious, reviewed change.
        let toml = r#"
            [limits]
            max_log = 4096
        "#;
        let cfg: RuntimeConfig = toml::from_str(toml).expect("unknown key is ignored");
        let defaults = VMLimits::default();
        assert_eq!(cfg.vm_limits().max_logs, defaults.max_logs);
    }
}
