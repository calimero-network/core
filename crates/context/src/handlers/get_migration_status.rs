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

    // The cohort is the PROJECTION's effective-member union across the subtree —
    // folded ONCE at a SINGLE cut — validated divergence-free across the e2e
    // `membership-enum` plane. `None` (empty/unfed namespace or store fault) falls
    // back to the live `list ∪ enumerate_inherited` union below; that live fallback
    // retires in #29b.
    if let Some(projected) =
        crate::scope_projection::ScopeProjections::member_identities_subtree_ephemeral(
            store,
            namespace_id,
            &groups,
        )
    {
        return Ok(projected.into_iter().collect());
    }

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

/// Admin-gate the `get_migration_status` read.
///
/// `get_migration_status` is an admin-API read (it exposes per-member completion
/// across the cohort), so it requires the same MANAGE/admin authority the sibling
/// migration admin operations (`retry_group_upgrade`, `upgrade_group`) enforce
/// via `require_admin`, NOT mere membership. Extracted as a pure (store read
/// only) helper so the gate the handler applies can be exercised directly.
pub fn authorize_migration_status(
    store: &calimero_store::Store,
    namespace_id: &ContextGroupId,
    node_identity: &PublicKey,
) -> eyre::Result<()> {
    MembershipRepository::new(store).require_admin(namespace_id, node_identity)
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
            // Admin-gated observability: `get_migration_status` is an admin-API
            // read (it exposes per-member completion across the cohort), so it
            // requires the same MANAGE/admin authority the sibling migration
            // admin operations (`retry_group_upgrade`, `upgrade_group`) enforce
            // via `require_admin`, not merely membership.
            authorize_migration_status(&self.datastore, &namespace_id, &node_identity)?;

            // Pin the cohort at the migration's expand-entry HLC (the sticky
            // `cascade_hlc` the originating op stamped). The pin lives on the
            // namespace-root record (a cascade stamps it there), so the pin still
            // reads the root.
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
            // The target is the MAX `to_version` across the namespace-root group
            // AND every descendant subgroup's upgrade record — NOT the root's
            // alone. A bare `upgrade_group` on a subgroup records the target only
            // there, so reading the root alone returned 0, making every member
            // trivially "migrated" (the false green that hid a stranded subgroup
            // member in #37).
            let target_version = max_subtree_target_version(&self.datastore, &namespace_id)?;

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
/// component yields `None`.
fn parse_schema_version(version: &str) -> Option<u32> {
    version
        .split('.')
        .next()
        .and_then(|major| major.trim().parse::<u32>().ok())
}

/// Derive the `target_version` the cohort is rolled up against from the group's
/// local upgrade record.
///
/// * `None` — no migration has ever been recorded, so nothing is in flight: the
///   target is the group's current (baseline) app version `0`. Every member is
///   trivially at target (the SDK's unversioned default is `0`), so a converged
///   cohort rolls up to `all_migrated`. Reporting `0` here is correct, NOT a
///   bogus "no migration to compare against" sentinel.
/// * `Some(record)` — the group is at / heading to `record.to_version`. For a
///   `Completed` record that is the version it reached (current); for an
///   `InProgress` one it is the pending destination. Either way the cohort
///   rolls up against `to_version`.
///
/// A `Some(record)` whose `to_version` does not parse to a `u32` must NOT
/// collapse to `0`: doing so makes every member reporting `schema_version >= 0`
/// (all of them) trivially "migrated" — a false green over a real migration.
/// An unknowable target pins to [`u32::MAX`] so no real version satisfies it and
/// `all_migrated` stays false until the record is replaced by a parseable one.
fn derive_target_version(upgrade: Option<&calimero_store::key::GroupUpgradeValue>) -> u32 {
    match upgrade {
        None => 0,
        Some(record) => parse_schema_version(&record.to_version).unwrap_or(u32::MAX),
    }
}

/// The migration target the whole cohort rolls up against: the MAX
/// [`derive_target_version`] across the namespace-root group AND every
/// descendant subgroup's upgrade record (`collect_descendants`, the same
/// subtree `collect_migration_cohort` walks).
///
/// Reading the root record alone returned `0` for a bare subgroup upgrade
/// (`upgrade_group` records the target on the subgroup, leaving the root
/// recordless) — making every member's `schema_version >= 0` trivially
/// "migrated", a false green that hid a stranded subgroup member. Taking the
/// max keeps the no-false-green rule: an unparseable record on any subtree
/// group resolves to [`u32::MAX`], which then dominates the max so no real
/// version satisfies it.
fn max_subtree_target_version(
    store: &calimero_store::Store,
    namespace_id: &ContextGroupId,
) -> eyre::Result<u32> {
    let mut groups = vec![*namespace_id];
    groups.extend(NamespaceRepository::new(store).collect_descendants(namespace_id)?);
    let upgrades = UpgradesRepository::new(store);
    let mut target = 0u32;
    for gid in &groups {
        target = target.max(derive_target_version(upgrades.load(gid)?.as_ref()));
    }
    Ok(target)
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

    use super::{authorize_migration_status, collect_migration_cohort, parse_schema_version};

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
                MemberCapabilities::CAN_JOIN_OPEN_SUBGROUPS.bits(),
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

    /// The handler gates `GetMigrationStatusRequest` on admin authority (the
    /// same `require_admin` the sibling migration admin ops enforce), NOT mere
    /// membership: a plain member must be rejected, the group admin allowed.
    ///
    /// This drives `authorize_migration_status` — the exact gate the handler
    /// calls — so reverting the gate back to a membership check (the regression
    /// this test is named to catch) fails it.
    #[test]
    fn admin_gate_rejects_non_admin_member() {
        let store = Store::new(Arc::new(InMemoryDB::owned()));
        let ns = ContextGroupId::from([0x33; 32]);
        let admin = PublicKey::from([0xAD; 32]);
        let member = PublicKey::from([0x11; 32]);

        MetaRepository::new(&store).save(&ns, &meta(admin)).unwrap();
        // `member` is a plain (non-admin) member of the namespace.
        MembershipRepository::new(&store)
            .add_member(&ns, &member, GroupMemberRole::Member)
            .unwrap();

        // A genuine member is still rejected by the handler's gate — the gate is
        // admin authority, not membership.
        assert!(
            MembershipRepository::new(&store)
                .is_member(&ns, &member)
                .unwrap(),
            "the rejected caller is genuinely a member — membership alone is not enough"
        );
        assert!(
            authorize_migration_status(&store, &ns, &member).is_err(),
            "a non-admin member must be rejected by the migration-status admin gate"
        );
        // The admin passes the same gate.
        assert!(authorize_migration_status(&store, &ns, &admin).is_ok());
    }

    fn upgrade_record(
        from: &str,
        to: &str,
        status: calimero_store::key::GroupUpgradeStatus,
    ) -> calimero_store::key::GroupUpgradeValue {
        calimero_store::key::GroupUpgradeValue {
            from_version: from.to_owned(),
            to_version: to.to_owned(),
            migration: None,
            initiated_at: 0,
            initiated_by: PublicKey::from([0x01; 32]),
            status,
            cascade_hlc: None,
            cascade_seq: None,
        }
    }

    /// No upgrade record ⇒ no migration is in flight, so the target is the
    /// group's current (baseline) app version `0` — every member trivially at
    /// target. NOT a value that makes a fully-converged cohort look stuck.
    #[test]
    fn target_version_no_record_is_current_baseline() {
        assert_eq!(
            super::derive_target_version(None),
            0,
            "no migration pending ⇒ target is the current baseline version"
        );
    }

    /// A Completed (non-pending) record ⇒ the group's current version is the
    /// version it reached (`to_version`), so the cohort rolls up against that.
    #[test]
    fn target_version_completed_record_is_reached_version() {
        let rec = upgrade_record(
            "1",
            "2",
            calimero_store::key::GroupUpgradeStatus::Completed { completed_at: None },
        );
        assert_eq!(super::derive_target_version(Some(&rec)), 2);
    }

    /// An InProgress record with a parseable `to_version` targets that version.
    #[test]
    fn target_version_in_progress_record_targets_to_version() {
        let rec = upgrade_record(
            "1",
            "2",
            calimero_store::key::GroupUpgradeStatus::InProgress {
                total: 1,
                completed: 0,
                failed: 0,
            },
        );
        assert_eq!(super::derive_target_version(Some(&rec)), 2);
    }

    /// Regression: a PENDING (`InProgress`) record whose `to_version` does not
    /// parse to a `u32` (e.g. a "v2"-style or otherwise non-numeric semver)
    /// must NOT collapse to target `0`. Target `0` would make every member
    /// reporting `schema_version >= 0` (i.e. all of them) trivially "migrated"
    /// — a FALSE GREEN in the middle of a real pending migration. An
    /// unknowable pending target pins to `u32::MAX` so no real version can
    /// satisfy it and `all_migrated` stays false until the migration resolves.
    #[test]
    fn target_version_in_progress_unparseable_to_version_is_not_false_green() {
        let rec = upgrade_record(
            "1",
            "v2",
            calimero_store::key::GroupUpgradeStatus::InProgress {
                total: 1,
                completed: 0,
                failed: 0,
            },
        );
        assert_eq!(
            super::derive_target_version(Some(&rec)),
            u32::MAX,
            "an unparseable pending target must not collapse to 0 (false green)"
        );
    }

    /// The cohort target must reflect a SUBGROUP's own upgrade record, not just
    /// the namespace-root group. A bare `upgrade_group` on a subgroup records
    /// the target on the subgroup, leaving the root recordless — reading the
    /// root alone resolved to `0`, making every member `schema_version >= 0`
    /// trivially "migrated" and hiding a stranded subgroup member (the #37 bug).
    #[test]
    fn target_version_takes_max_across_subgroup_records() {
        let store = Store::new(Arc::new(InMemoryDB::owned()));
        let root = ContextGroupId::from([0x44; 32]);
        let subgroup = ContextGroupId::from([0x55; 32]);
        let admin = PublicKey::from([0xAD; 32]);

        MetaRepository::new(&store)
            .save(&root, &meta(admin))
            .unwrap();
        MetaRepository::new(&store)
            .save(&subgroup, &meta(admin))
            .unwrap();
        NamespaceRepository::new(&store)
            .nest(&root, &subgroup)
            .unwrap();

        // Upgrade record ONLY on the subgroup (root recordless), heading to v3.
        calimero_governance_store::UpgradesRepository::new(&store)
            .save(
                &subgroup,
                &upgrade_record(
                    "1",
                    "3",
                    calimero_store::key::GroupUpgradeStatus::InProgress {
                        total: 1,
                        completed: 0,
                        failed: 0,
                    },
                ),
            )
            .unwrap();

        // The root record alone resolves to 0 (the false-green path)...
        assert_eq!(
            super::derive_target_version(
                calimero_governance_store::UpgradesRepository::new(&store)
                    .load(&root)
                    .unwrap()
                    .as_ref()
            ),
            0,
            "root group is recordless → root-only target is a false-green 0"
        );
        // ...but the subtree max reads the subgroup's own v3.
        assert_eq!(
            super::max_subtree_target_version(&store, &root).unwrap(),
            3,
            "subtree target must read the subgroup's own record, not the root's"
        );
    }
}
