//! The per-context upgrade decision table: what the upgrade engine should do
//! to ONE context, derived purely from the embedded ABIs (current vs target)
//! of that context's service. No I/O — callers resolve the two manifests
//! (`application_bytes_from_blob` + `read_embedded_state_schema`) and feed
//! them in, which keeps every rule unit-testable.
//!
//! This is the v2 replacement for caller-supplied migrate-method strings:
//! the app declares its state version and migration edges in the ABI
//! (`#[app::state(version = N)]` + `#[derive(app::Migrate)]`), and the node
//! derives where/whether/what to run — per service, so multi-service bundles
//! migrate only the services whose schema actually changed.

use calimero_wasm_abi::schema::Manifest;

/// What the upgrade engine should do to one context.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpgradeAction {
    /// Same state version — swap bytecode and record activation. Never runs
    /// wasm: services untouched by a release are vacuous BY CONSTRUCTION
    /// (no MethodNotFound probing).
    CodeOnly,
    /// One hop behind: run `method`, then record activation.
    Migrate { method: String, from: u32, to: u32 },
    /// More than one hop behind — chained dispatch or peer resync territory;
    /// running the latest edge against too-old state would corrupt it.
    Behind { from: u32, to: u32 },
    /// Target is OLDER than current — the identity-downgrade gate owns this.
    Downgrade { from: u32, to: u32 },
    /// A migration is needed (`to == from + 1`) but the target ABI declares
    /// no edge for `from` — a mis-built app. Emit-side: reject the upgrade;
    /// actuation-side: report apply-failed rather than guessing.
    MissingEdge { from: u32, to: u32 },
}

/// Either side's ABI could not be resolved (blob absent, service missing,
/// or a pre-ABI build). Emit-side callers reject migration-needing upgrades
/// with a "rebuild with the current SDK" error; actuation-side callers fall
/// back to legacy behavior.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("embedded ABI unavailable for the {side} bytecode")]
pub struct AbiUnavailable {
    pub side: AbiSide,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AbiSide {
    Current,
    Target,
}

impl core::fmt::Display for AbiSide {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Current => f.write_str("current"),
            Self::Target => f.write_str("target"),
        }
    }
}

/// Resolve the action for one context from its service's two manifests.
///
/// Version source is [`Manifest::state_version_or_default`]: the explicit
/// `state_version` (always emitted by current SDKs), falling back to the
/// legacy `migration.to_schema_version`, then 1 — so pre-versioned and
/// 9.4-era builds resolve correctly.
pub fn plan_upgrade(
    current: Option<&Manifest>,
    target: Option<&Manifest>,
) -> Result<UpgradeAction, AbiUnavailable> {
    let current = current.ok_or(AbiUnavailable {
        side: AbiSide::Current,
    })?;
    let target = target.ok_or(AbiUnavailable {
        side: AbiSide::Target,
    })?;

    let from = current.state_version_or_default();
    let to = target.state_version_or_default();

    if from == to {
        return Ok(UpgradeAction::CodeOnly);
    }
    if to < from {
        return Ok(UpgradeAction::Downgrade { from, to });
    }
    if to == from + 1 {
        return Ok(match target.edge_from(from) {
            Some(edge) => UpgradeAction::Migrate {
                method: edge.method,
                from,
                to,
            },
            None => UpgradeAction::MissingEdge { from, to },
        });
    }
    Ok(UpgradeAction::Behind { from, to })
}

#[cfg(test)]
mod tests {
    use calimero_wasm_abi::schema::{MigrationAbi, MigrationEdgeAbi};

    use super::*;

    fn manifest(state_version: Option<u32>) -> Manifest {
        let mut m = Manifest::new();
        m.state_version = state_version;
        m
    }

    fn with_edge(mut m: Manifest, method: &str, from_version: u32) -> Manifest {
        m.migrations.push(MigrationEdgeAbi {
            method: method.to_owned(),
            from_version,
        });
        m
    }

    #[test]
    fn equal_versions_are_code_only() {
        let cur = manifest(Some(2));
        let tgt = manifest(Some(2));
        assert_eq!(
            plan_upgrade(Some(&cur), Some(&tgt)),
            Ok(UpgradeAction::CodeOnly)
        );
    }

    #[test]
    fn unversioned_manifests_default_to_one_and_code_only() {
        // Pre-versioned apps on both sides: 1 == 1.
        let cur = manifest(None);
        let tgt = manifest(None);
        assert_eq!(
            plan_upgrade(Some(&cur), Some(&tgt)),
            Ok(UpgradeAction::CodeOnly)
        );
    }

    #[test]
    fn one_hop_with_edge_migrates() {
        let cur = manifest(Some(1));
        let tgt = with_edge(manifest(Some(2)), "migrate", 1);
        assert_eq!(
            plan_upgrade(Some(&cur), Some(&tgt)),
            Ok(UpgradeAction::Migrate {
                method: "migrate".to_owned(),
                from: 1,
                to: 2
            })
        );
    }

    #[test]
    fn one_hop_resolves_via_legacy_single_migration() {
        // 9.4-era target: no edge list, only the single `migration` field.
        let cur = manifest(Some(1));
        let mut tgt = manifest(Some(2));
        tgt.migration = Some(MigrationAbi {
            method: "migrate_v1_to_v2".to_owned(),
            to_schema_version: 2,
        });
        assert_eq!(
            plan_upgrade(Some(&cur), Some(&tgt)),
            Ok(UpgradeAction::Migrate {
                method: "migrate_v1_to_v2".to_owned(),
                from: 1,
                to: 2
            })
        );
    }

    #[test]
    fn legacy_current_resolves_from_via_migration_target() {
        // Current built by a 9.4-era SDK: version only visible through
        // migration.to_schema_version.
        let mut cur = manifest(None);
        cur.migration = Some(MigrationAbi {
            method: "migrate_v1_to_v2".to_owned(),
            to_schema_version: 2,
        });
        let tgt = with_edge(manifest(Some(3)), "migrate_v2_to_v3", 2);
        assert_eq!(
            plan_upgrade(Some(&cur), Some(&tgt)),
            Ok(UpgradeAction::Migrate {
                method: "migrate_v2_to_v3".to_owned(),
                from: 2,
                to: 3
            })
        );
    }

    #[test]
    fn one_hop_without_edge_is_missing_edge() {
        let cur = manifest(Some(1));
        let tgt = manifest(Some(2)); // declares v2 but no edge from 1
        assert_eq!(
            plan_upgrade(Some(&cur), Some(&tgt)),
            Ok(UpgradeAction::MissingEdge { from: 1, to: 2 })
        );
    }

    #[test]
    fn multiple_hops_are_behind() {
        let cur = manifest(Some(1));
        let tgt = with_edge(manifest(Some(3)), "migrate_v2_to_v3", 2);
        assert_eq!(
            plan_upgrade(Some(&cur), Some(&tgt)),
            Ok(UpgradeAction::Behind { from: 1, to: 3 })
        );
    }

    #[test]
    fn older_target_is_downgrade() {
        let cur = manifest(Some(3));
        let tgt = manifest(Some(2));
        assert_eq!(
            plan_upgrade(Some(&cur), Some(&tgt)),
            Ok(UpgradeAction::Downgrade { from: 3, to: 2 })
        );
    }

    #[test]
    fn missing_abi_is_reported_per_side() {
        let m = manifest(Some(1));
        assert_eq!(
            plan_upgrade(None, Some(&m)),
            Err(AbiUnavailable {
                side: AbiSide::Current
            })
        );
        assert_eq!(
            plan_upgrade(Some(&m), None),
            Err(AbiUnavailable {
                side: AbiSide::Target
            })
        );
    }
}
