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
    /// startup. Absent (`#[serde(default)]` → `false`) in every existing
    /// `config.toml`, so master behavior — the namespace-cascade write-freeze
    /// — is preserved until an operator sets `[context] migration_v2 = true`.
    #[serde(default)]
    pub migration_v2: bool,
}

#[cfg(test)]
mod tests {
    use super::ContextConfig;

    /// The on-disk `[context]` section that ships in every existing
    /// `config.toml` (only the client signer). It must still deserialize, and
    /// the absent `migration_v2` must default to `false` so master behavior —
    /// the namespace-cascade write-freeze — is preserved (backward-compat).
    const LEGACY_CONTEXT_SECTION: &str = r#"{
        "config": { "signer": { "self": {} } }
    }"#;

    #[test]
    fn migration_v2_defaults_off_when_absent() {
        let cfg: ContextConfig = serde_json::from_str(LEGACY_CONTEXT_SECTION)
            .expect("legacy [context] section must still deserialize");

        assert!(
            !cfg.migration_v2,
            "absent migration_v2 must default off so existing configs are unchanged"
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
