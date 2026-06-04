use std::collections::BTreeSet;

use actix::{ActorResponse, Handler, Message};
use calimero_context_client::group::{
    compute_migration_status_rollup, GetMigrationStatusRequest, MigrationStatus,
};
use calimero_context_config::types::ContextGroupId;
use calimero_governance_store::{MembershipRepository, NamespaceRepository, UpgradesRepository};
use calimero_primitives::identity::PublicKey;
use calimero_storage::logical_clock::HybridTimestamp;
use eyre::bail;

use crate::ContextManager;

/// Build the pinned-cohort expected-member set for a namespace migration.
///
/// **Expected members = the inherited-membership closure** over the namespace
/// subtree, NOT `MembershipRepository::count` (which only sees direct rows and
/// would undercount inherited Open-subgroup members → a false green). For the
/// namespace root and every descendant (`collect_descendants`, mirroring
/// `collect_cascade_status`), we union the direct member list with
/// `enumerate_inherited` (the #2371 pattern), deduplicating across the subtree.
///
/// Pure (store read only) so it can be exercised without standing up an actor.
pub fn collect_migration_cohort(
    store: &calimero_store::Store,
    namespace_id: &ContextGroupId,
) -> eyre::Result<Vec<PublicKey>> {
    // `collect_descendants` EXCLUDES the starting group, so we prepend it.
    let mut groups = vec![*namespace_id];
    groups.extend(NamespaceRepository::new(store).collect_descendants(namespace_id)?);

    let membership = MembershipRepository::new(store);
    // BTreeSet both dedups across the subtree (a member can appear directly in
    // one group and inherited in another) and yields a deterministic order.
    let mut cohort: BTreeSet<PublicKey> = BTreeSet::new();
    for gid in &groups {
        for (pk, _role) in membership.list(gid, 0, usize::MAX)? {
            let _ = cohort.insert(pk);
        }
        for (pk, _role) in membership.enumerate_inherited(gid)? {
            let _ = cohort.insert(pk);
        }
    }

    Ok(cohort.into_iter().collect())
}

impl Handler<GetMigrationStatusRequest> for ContextManager {
    type Result = ActorResponse<Self, <GetMigrationStatusRequest as Message>::Result>;

    fn handle(
        &mut self,
        GetMigrationStatusRequest {
            namespace_id,
            member_reports,
        }: GetMigrationStatusRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        let result = (|| {
            let Some((node_identity, _)) = self.node_namespace_identity(&namespace_id) else {
                bail!("node has no group identity configured");
            };
            if !MembershipRepository::new(&self.datastore)
                .is_member(&namespace_id, &node_identity)?
            {
                bail!("node is not a member of namespace '{namespace_id:?}'");
            }

            // Pin the cohort at the migration's expand-entry HLC (the sticky
            // `cascade_hlc` the originating op stamped). `target_version` reads
            // off the upgrade record's `to_version`; absent an upgrade record
            // there is no migration in flight, so the target is the current
            // (from) version and the cohort still reports against it.
            let upgrade = UpgradesRepository::new(&self.datastore).load(&namespace_id)?;
            let cohort_pinned_at_hlc: Option<HybridTimestamp> =
                upgrade.as_ref().and_then(|u| u.cascade_hlc);
            // The overlay pin: the migration's expand-entry governance position
            // (`NamespaceGovHead.sequence`), captured by `cascade_upgrade::apply`.
            // It lives in the SAME number space as the heartbeat's
            // `synced_up_to_hlc` (`= head.sequence`), so the rollup compares them
            // like-for-like. `cohort_pinned_at_hlc` is the replicated NTP64 HLC
            // fence surfaced for display only — never the overlay pin.
            let cohort_pinned_at_seq: Option<u64> = upgrade.as_ref().and_then(|u| u.cascade_seq);
            let target_version = upgrade
                .as_ref()
                .and_then(|u| parse_schema_version(&u.to_version))
                .unwrap_or(0);

            // The full inherited-membership closure for the subtree (the #2371
            // `list ∪ enumerate_inherited` set). The rollup applies the
            // expand-entry HLC pin over this closure via the per-member
            // `synced_up_to_hlc` overlay: a member whose freshest heartbeat
            // proves it had not synced through the pin is dropped from the
            // cohort (it was not part of the converged pinned state), realizing
            // cohort-pinning without an as-of membership enumeration the store
            // does not provide.
            let closure = collect_migration_cohort(&self.datastore, &namespace_id)?;

            // Roll up the freshest per-member heartbeat reports the caller
            // snapshotted from the node-side `MigrationStatusCache` (Task 6c.8).
            // A member absent from the snapshot resolves to `unknown`, which
            // keeps `all_migrated` false — never a false green.
            Ok::<MigrationStatus, eyre::Report>(compute_migration_status_rollup(
                target_version,
                cohort_pinned_at_hlc,
                cohort_pinned_at_seq,
                &closure,
                |peer| member_reports.get(peer).copied(),
            ))
        })();
        ActorResponse::reply(result)
    }
}

/// Parse the leading integer of a semver `to_version` ("2", "2.0.0") into the
/// `u32` schema-version a heartbeat reports. Best-effort: a non-numeric leading
/// component yields `None` and the handler falls back to `0`.
fn parse_schema_version(version: &str) -> Option<u32> {
    version
        .split('.')
        .next()
        .and_then(|major| major.trim().parse::<u32>().ok())
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use calimero_context_config::types::ContextGroupId;
    use calimero_context_config::{MemberCapabilities, VisibilityMode};
    use calimero_governance_store::{
        CapabilitiesRepository, MembershipRepository, MetaRepository, NamespaceRepository,
    };
    use calimero_primitives::application::ApplicationId;
    use calimero_primitives::context::{GroupMemberRole, UpgradePolicy};
    use calimero_primitives::identity::PublicKey;
    use calimero_store::db::InMemoryDB;
    use calimero_store::key::GroupMetaValue;
    use calimero_store::Store;

    use super::{collect_migration_cohort, parse_schema_version};

    fn meta(admin: PublicKey) -> GroupMetaValue {
        GroupMetaValue {
            app_key: [0x55; 32],
            target_application_id: ApplicationId::from([0x66; 32]),
            upgrade_policy: UpgradePolicy::Automatic,
            created_at: 1_700_000_000,
            admin_identity: admin,
            owner_identity: admin,
            migration: None,
            auto_join: true,
        }
    }

    /// An inherited (non-direct-row) member of an Open subgroup MUST be counted
    /// in the cohort. Guards against the `MembershipRepository::count`
    /// undercount that only sees direct rows → false green.
    #[test]
    fn cohort_counts_inherited_open_subgroup_member() {
        let store = Store::new(Arc::new(InMemoryDB::owned()));
        let parent = ContextGroupId::from([0x11; 32]);
        let child = ContextGroupId::from([0x22; 32]);

        let direct = PublicKey::from([0xA1; 32]);
        let inherited = PublicKey::from([0xB2; 32]);

        // Both groups need metadata + a parent edge so the inherited walk runs.
        MetaRepository::new(&store)
            .save(&parent, &meta(direct))
            .unwrap();
        MetaRepository::new(&store)
            .save(&child, &meta(direct))
            .unwrap();
        NamespaceRepository::new(&store)
            .nest(&parent, &child)
            .unwrap();
        // Child is Open so parent members with the join cap inherit into it.
        CapabilitiesRepository::new(&store)
            .set_subgroup_visibility(&child, VisibilityMode::Open)
            .unwrap();

        // `inherited` is a DIRECT member only of the parent, with the
        // open-subgroup join cap — it must surface in the child's closure
        // purely by inheritance (no stored row in the child).
        MembershipRepository::new(&store)
            .add_member(&parent, &inherited, GroupMemberRole::Member)
            .unwrap();
        CapabilitiesRepository::new(&store)
            .set_member_capability(
                &parent,
                &inherited,
                MemberCapabilities::CAN_JOIN_OPEN_SUBGROUPS,
            )
            .unwrap();
        // `direct` is a direct member of the child.
        MembershipRepository::new(&store)
            .add_member(&child, &direct, GroupMemberRole::Member)
            .unwrap();

        // count() on the child sees only its direct rows — the undercount we
        // must NOT use as expected_members.
        let direct_count = MembershipRepository::new(&store).count(&child).unwrap();
        assert_eq!(direct_count, 1, "count() sees only the direct child row");

        // The cohort over the subtree rooted at the child must include BOTH.
        let cohort = collect_migration_cohort(&store, &child).unwrap();
        assert!(
            cohort.contains(&inherited),
            "inherited Open-subgroup member must be in the cohort, not dropped by count()"
        );
        assert!(cohort.contains(&direct));
        assert_eq!(cohort.len(), 2);
        assert!(
            cohort.len() > direct_count,
            "cohort must exceed the direct-row count (inherited member included)"
        );
    }

    #[test]
    fn parse_schema_version_reads_major() {
        assert_eq!(parse_schema_version("2"), Some(2));
        assert_eq!(parse_schema_version("2.0.0"), Some(2));
        assert_eq!(parse_schema_version("11.3.1"), Some(11));
        assert_eq!(parse_schema_version("v2"), None);
        assert_eq!(parse_schema_version(""), None);
    }
}
