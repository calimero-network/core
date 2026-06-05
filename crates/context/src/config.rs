#![allow(clippy::exhaustive_structs, reason = "TODO: Allowed until reviewed")]

pub use calimero_context_config::client_config::ClientConfig;
use serde::{Deserialize, Serialize};

/// Node context section: client config only (local group governance; no chain).
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ContextConfig {
    #[serde(rename = "config")]
    pub client: ClientConfig,

    /// Master switch for the PR-6 hybrid zero-downtime migration framework,
    /// threaded into [`crate::ContextManagerConfig::migration_v2`] at node
    /// startup. Now that PR-6a (no-freeze) and PR-6b (absorb-don't-drop) have
    /// landed, this defaults ON: absent (`#[serde(default = ...)]` → `true`)
    /// in every existing `config.toml`, so the non-freezing migration is the
    /// node's native behavior. An operator can pin `[context] migration_v2 =
    /// false` to restore the legacy namespace-cascade write-freeze.
    #[serde(default = "default_migration_v2")]
    pub migration_v2: bool,
}

/// Serde default for [`ContextConfig::migration_v2`]: ON, matching
/// [`crate::ContextManagerConfig::default`].
fn default_migration_v2() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::ContextConfig;

    /// The on-disk `[context]` section that ships in every existing
    /// `config.toml` (only the client signer). It must still deserialize, and
    /// the absent `migration_v2` must default to `true` now that PR-6a
    /// (no-freeze) and PR-6b (absorb-don't-drop) have landed — the
    /// non-freezing migration is the node's native behavior.
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
}
