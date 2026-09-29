#![allow(clippy::exhaustive_structs, reason = "TODO: Allowed until reviewed")]

use serde::{Deserialize, Serialize};

/// Node context section (local group governance; no chain).
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ContextConfig {
    /// Master switch for the PR-6 hybrid zero-downtime migration framework,
    /// threaded into [`crate::ContextManagerConfig::migration_v2`] at node
    /// startup. Now that PR-6a (no-freeze) and PR-6b (absorb-don't-drop) have
    /// landed, this defaults ON: absent (`#[serde(default = ...)]` → `true`)
    /// in every existing `config.toml`, so the non-freezing migration is the
    /// node's native behavior. An operator can pin `[context] migration_v2 =
    /// false` to restore the legacy namespace-cascade write-freeze.
    #[serde(default = "default_migration_v2")]
    pub migration_v2: bool,

    /// `[context.search]`: the node's full-text search. Absent in an existing
    /// `config.toml`, it takes [`SearchSettings::default`].
    #[serde(default)]
    pub search: SearchSettings,
}

/// Serde default for [`ContextConfig::migration_v2`]: ON, matching
/// [`crate::ContextManagerConfig::default`].
fn default_migration_v2() -> bool {
    true
}

/// The operator's knobs for full-text search (`[context.search]`).
///
/// Search is opt-in per app: only an app that declares an index (the SDK's
/// `search_indexes!`) is indexed, and any other app costs one export lookup
/// per execution. `enabled` is the node-wide switch on top of that.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct SearchSettings {
    /// Index the apps that declare an index, and let their views search.
    /// With `false` no dirty row is written and no indexer runs; a search
    /// view traps.
    pub enabled: bool,
    /// How often the indexer commits, in milliseconds: the lag between a
    /// write and a search that finds it.
    pub commit_interval_ms: u64,
    /// Index chunks cached in memory across all contexts, in MiB.
    pub cache_mib: u64,
    /// Indexes open at most (about 1.6 MiB each); the least recently used
    /// idle ones close past it.
    pub max_open_indexes: usize,
    /// An index unused for this long, in seconds, is closed.
    pub idle_close_secs: u64,
    /// How often every indexed context is checked for state a sync installed
    /// without going through the dirty log, in seconds.
    pub audit_interval_secs: u64,
    /// Deleted index bytes after which a context's slice of the index column
    /// is compacted, in MiB.
    pub compact_after_mib: u64,
}

impl Default for SearchSettings {
    fn default() -> Self {
        let defaults = calimero_search::SearchConfig::default();
        Self {
            enabled: true,
            commit_interval_ms: millis(defaults.commit_interval),
            cache_mib: (defaults.cache_bytes >> 20) as u64,
            max_open_indexes: defaults.max_open_indexes,
            idle_close_secs: defaults.reader_idle.as_secs(),
            audit_interval_secs: defaults.audit_interval.as_secs(),
            compact_after_mib: defaults.compact_after_bytes >> 20,
        }
    }
}

fn millis(duration: core::time::Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

impl SearchSettings {
    /// The service configuration these settings describe. Zero intervals are
    /// raised to one unit, since a timer cannot tick at zero.
    #[must_use]
    pub fn to_config(&self) -> calimero_search::SearchConfig {
        use core::time::Duration;

        calimero_search::SearchConfig {
            commit_interval: Duration::from_millis(self.commit_interval_ms.max(1)),
            cache_bytes: usize::try_from(self.cache_mib.saturating_mul(1 << 20))
                .unwrap_or(usize::MAX),
            max_open_indexes: self.max_open_indexes,
            reader_idle: Duration::from_secs(self.idle_close_secs),
            audit_interval: Duration::from_secs(self.audit_interval_secs.max(1)),
            compact_after_bytes: self.compact_after_mib.saturating_mul(1 << 20),
            ..calimero_search::SearchConfig::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{ContextConfig, SearchSettings};

    /// The on-disk `[context]` section that ships in every existing
    /// `config.toml`. It still carries the `config.signer.self` block that
    /// nodes wrote before the external-chain signer shape was removed, so this
    /// doubles as the backward-compatibility check: the key is now unknown and
    /// must be ignored rather than rejected. The absent `migration_v2` must
    /// still default to `true`.
    const LEGACY_CONTEXT_SECTION: &str = r#"{
        "config": { "signer": { "self": {} } }
    }"#;

    #[test]
    fn migration_v2_defaults_on_when_absent() {
        let cfg: ContextConfig = serde_json::from_str(LEGACY_CONTEXT_SECTION)
            .expect("legacy [context] section must still deserialize");

        assert!(
            cfg.migration_v2,
            "absent migration_v2 must default on now that 6a + 6b have landed"
        );
    }

    #[test]
    fn migration_v2_can_be_pinned_off() {
        let cfg: ContextConfig = serde_json::from_str(
            r#"{
                "config": { "signer": { "self": {} } },
                "migration_v2": false
            }"#,
        )
        .expect("[context] section with migration_v2 = false must deserialize");

        assert!(
            !cfg.migration_v2,
            "migration_v2 = false must thread through to restore the legacy freeze"
        );
    }

    #[test]
    fn migration_v2_threads_when_set() {
        let cfg: ContextConfig = serde_json::from_str(
            r#"{
                "config": { "signer": { "self": {} } },
                "migration_v2": true
            }"#,
        )
        .expect("[context] section with migration_v2 must deserialize");

        assert!(
            cfg.migration_v2,
            "migration_v2 = true must thread through to the resolved config"
        );
    }

    #[test]
    fn search_defaults_on_and_matches_the_service_defaults() {
        let cfg: ContextConfig = serde_json::from_str(LEGACY_CONTEXT_SECTION)
            .expect("a [context] section without [context.search] must deserialize");
        assert_eq!(cfg.search, SearchSettings::default());
        assert!(cfg.search.enabled);
        let service = calimero_search::SearchConfig::default();
        let resolved = cfg.search.to_config();
        assert_eq!(resolved.commit_interval, service.commit_interval);
        assert_eq!(resolved.cache_bytes, service.cache_bytes);
        assert_eq!(resolved.max_open_indexes, service.max_open_indexes);
        assert_eq!(resolved.reader_idle, service.reader_idle);
        assert_eq!(resolved.audit_interval, service.audit_interval);
        assert_eq!(resolved.compact_after_bytes, service.compact_after_bytes);
    }

    #[test]
    fn search_settings_thread_through_and_reject_typos() {
        let cfg: ContextConfig = serde_json::from_str(
            r#"{ "search": { "enabled": false, "cache_mib": 8, "commit_interval_ms": 0 } }"#,
        )
        .expect("a partial [context.search] must deserialize");
        assert!(!cfg.search.enabled);
        assert_eq!(cfg.search.to_config().cache_bytes, 8 << 20);
        assert_eq!(
            cfg.search.to_config().commit_interval.as_millis(),
            1,
            "a zero interval is raised to one"
        );
        assert!(serde_json::from_str::<ContextConfig>(r#"{ "search": { "enable": true } }"#)
            .is_err());
    }
}
