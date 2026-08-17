use std::collections::BTreeSet;

use actix::{ActorResponse, Handler, Message};
use calimero_account::AccountId;
use calimero_context_client::group::{
    compute_migration_status_rollup, GetMigrationStatusRequest, MemberMigrationReport,
    MigrationStatus,
};
use calimero_context_config::types::ContextGroupId;
use calimero_governance_store::{MembershipRepository, NamespaceRepository, UpgradesRepository};
use calimero_primitives::identity::PublicKey;
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
    // The cohort is a set of REPLICAS, not of people: each entry is matched
    // against a migration heartbeat, which a node emits per device. So the
    // membership answer (accounts) is expanded to the devices that speak for
    // them — with one device per member this is the same set it always was, and
    // with two it correctly expects both replicas to report.
    // Resolved to the namespace ANCHOR before reading bindings. `namespace_id` is
    // whatever group the caller rooted this walk at, and a subgroup holds no
    // bindings of its own — reading them there returns nothing and the cohort
    // comes back empty, which reads as "every member has migrated".
    let anchor = NamespaceRepository::new(store).resolve(namespace_id)?;
    let expand = |accounts: std::collections::BTreeSet<AccountId>| -> eyre::Result<Vec<PublicKey>> {
        let bindings = calimero_governance_store::AccountBindingRepository::new(store)
            .live_bindings(&anchor)?;
        Ok(bindings
            .iter()
            .filter(|binding| accounts.contains(&binding.account))
            .map(|binding| binding.sign_pk)
            .collect())
    };

    if let Some(projected) =
        crate::scope_projection::ScopeProjections::member_identities_subtree_ephemeral(
            store,
            namespace_id,
            &groups,
        )
    {
        return expand(projected);
    }

    let membership = MembershipRepository::new(store);
    // BTreeSet both dedups across the subtree (a member can appear directly in
    // one group and inherited in another) and yields a deterministic order.
    let mut cohort: BTreeSet<AccountId> = BTreeSet::new();
    for gid in &groups {
        for (pk, _role) in membership.list(gid, 0, usize::MAX)? {
            let _ = cohort.insert(pk);
        }
        for (pk, _role) in membership.enumerate_inherited(gid)? {
            let _ = cohort.insert(pk);
        }
    }
    expand(cohort)
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
    let node_account = crate::member_account::require(store, namespace_id, node_identity)?;
    MembershipRepository::new(store).require_admin(namespace_id, &node_account)
}

/// Assemble and roll up a namespace's migration status.
///
/// Shared by the admin read and the node-side heartbeat reaction so both
/// answer the same question from the same inputs; a second assembly is how
/// the panel and the event stream start disagreeing.
///
/// Pins the cohort at the migration's expand-entry HLC (the sticky
/// `cascade_hlc` the originating op stamped). The pin lives on the
/// namespace-root record (a cascade stamps it there), so the pin still reads
/// the root.
///
/// The overlay pin is the migration's expand-entry governance position
/// (`NamespaceGovHead.sequence`), captured by `cascade_upgrade::apply`. It
/// lives in the SAME number space as the heartbeat's `synced_up_to_hlc`
/// (`= head.sequence`), so the rollup compares them like-for-like. The
/// returned `cohort_pinned_at_hlc` is the replicated NTP64 HLC fence surfaced
/// for display only - never the overlay pin.
///
/// The target is the MAX `to_version` across the namespace-root group AND
/// every descendant subgroup's upgrade record - NOT the root's alone. A bare
/// `upgrade_group` on a subgroup records the target only there, so reading
/// the root alone returned 0, making every member trivially "migrated" (the
/// false green that hid a stranded subgroup member).
///
/// The cohort is the full inherited-membership closure for the subtree (the
/// `list ∪ enumerate_inherited` set). The rollup applies the
/// expand-entry HLC pin over this closure via the per-member
/// `synced_up_to_hlc` overlay: a member whose freshest heartbeat proves it
/// had not synced through the pin is dropped from the cohort (it was not
/// part of the converged pinned state), realizing cohort-pinning without an
/// as-of membership enumeration the store does not provide.
///
/// `report_for` resolves the freshest per-member heartbeat report; a member
/// absent from it resolves to `unknown`, which keeps `all_migrated` false -
/// never a false green.
pub fn compute_namespace_rollup(
    store: &calimero_store::Store,
    namespace_id: &ContextGroupId,
    mut report_for: impl FnMut(&PublicKey) -> Option<MemberMigrationReport>,
) -> eyre::Result<MigrationStatus> {
    let upgrades = UpgradesRepository::new(store);
    let upgrade = upgrades.load(namespace_id)?;
    let status = compute_migration_status_rollup(
        max_subtree_target_version(store, namespace_id)?,
        upgrade.as_ref().and_then(|u| u.cascade_hlc),
        upgrade.as_ref().and_then(|u| u.cascade_seq),
        &collect_migration_cohort(store, namespace_id)?,
        &mut report_for,
    );
    Ok(MigrationStatus {
        fleet_completed_at: upgrades.fleet_completed_at(namespace_id)?,
        ..status
    })
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
            let Some((node_identity, _)) = self.node_signing_key(&namespace_id) else {
                bail!("node has no group identity configured");
            };
            // Admin-gated observability: `get_migration_status` is an admin-API
            // read (it exposes per-member completion across the cohort), so it
            // requires the same MANAGE/admin authority the sibling migration
            // admin operations (`retry_group_upgrade`, `upgrade_group`) enforce
            // via `require_admin`, not merely membership.
            authorize_migration_status(&self.datastore, &namespace_id, &node_identity)?;

            // Roll up the freshest per-member heartbeat reports the caller
            // snapshotted from the node-side `MigrationStatusCache`.
            compute_namespace_rollup(&self.datastore, &namespace_id, |peer| {
                member_reports.get(peer).copied()
            })
        })();
        ActorResponse::reply(result)
    }
}

/// Derive the `target_version` the cohort is rolled up against from the group's
/// local upgrade record.
///
/// * `None` — no migration has ever been recorded, so nothing is in flight: the
///   target is the group's current (baseline) app version `0`. Every member is
///   trivially at target (the SDK's unversioned default is `0`), so a converged
///   cohort rolls up to `all_migrated`. Reporting `0` here is correct, NOT a
///   bogus "no migration to compare against" sentinel.
/// * `Some(record)` — the group is at / heading to `record.to_state_version`.
///   For a `Completed` record that is the version it reached (current); for an
///   `InProgress` one it is the pending destination.
///
/// A `Some(record)` whose `to_state_version` is `0` (unresolvable) must NOT
/// collapse to `0` as a target: doing so makes every member reporting
/// `schema_version >= 0` (all of them) trivially "migrated" — a false green
/// over a real migration. An unresolvable target pins to [`u32::MAX`] so no
/// real version satisfies it until the record is replaced by a resolvable one.
fn derive_target_version(upgrade: Option<&calimero_store::key::GroupUpgradeValue>) -> u32 {
    match upgrade {
        None => 0,
        // `0` means the target's ABI was unreadable. It must NOT collapse to a
        // satisfied target: every member reporting `>= 0` would be trivially
        // migrated, a false green over a real migration. Pin to MAX so no real
        // version satisfies it until the record is replaced by a resolvable one.
        Some(record) if record.to_state_version == 0 => u32::MAX,
        Some(record) => record.to_state_version,
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
    use calimero_primitives::context::GroupMemberRole;
    use calimero_primitives::identity::PublicKey;
    use calimero_store::db::InMemoryDB;
    use calimero_store::key::GroupMetaValue;
    use calimero_store::Store;

    use super::{authorize_migration_status, collect_migration_cohort};

    fn meta(admin: PublicKey) -> GroupMetaValue {
        GroupMetaValue {
            app_key: [0x55; 32],
            target_application_id: ApplicationId::from([0x66; 32]),
            created_at: 1_700_000_000,
            admin_identity: crate::test_support::account_for(&admin),
            owner_identity: crate::test_support::account_for(&admin),
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
            .add_member(
                &parent,
                &crate::test_support::enrol(&store, &parent, &inherited),
                GroupMemberRole::Member,
            )
            .unwrap();
        CapabilitiesRepository::new(&store)
            .set_member_capability(
                &parent,
                &crate::test_support::account_for(&inherited),
                MemberCapabilities::CAN_JOIN_OPEN_SUBGROUPS.bits(),
            )
            .unwrap();
        // `direct` is a direct member of the child.
        MembershipRepository::new(&store)
            .add_member(
                &child,
                // Enrolled at the PARENT: bindings live at the namespace anchor
                // and readers resolve up to it, so a row against a nested child
                // would be invisible.
                &crate::test_support::enrol(&store, &parent, &direct),
                GroupMemberRole::Member,
            )
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

    /// The handler gates `GetMigrationStatusRequest` on admin authority (the
    /// same `require_admin` the sibling migration admin ops enforce), NOT mere
    /// membership: a plain member must be rejected, the group admin allowed.
    ///
    /// The payload is a subtree-wide cohort rollup that `collect_descendants`
    /// walks unfiltered, so a member-level gate would disclose peer identities
    /// inside Restricted subgroups. This drives `authorize_migration_status` -
    /// the exact gate the handler calls - so relaxing the gate fails it.
    #[test]
    fn admin_gate_rejects_non_admin_member() {
        let store = Store::new(Arc::new(InMemoryDB::owned()));
        let ns = ContextGroupId::from([0x33; 32]);
        let admin = PublicKey::from([0xAD; 32]);
        let member = PublicKey::from([0x11; 32]);

        // The admin is enrolled too: the gate resolves its KEY, and a meta that
        // merely names an account nobody is bound to leaves it unresolvable.
        let admin_account = crate::test_support::enrol(&store, &ns, &admin);
        MetaRepository::new(&store).save(&ns, &meta(admin)).unwrap();
        MembershipRepository::new(&store)
            .add_member(&ns, &admin_account, GroupMemberRole::Admin)
            .unwrap();
        // `member` is a plain (non-admin) member of the namespace.
        MembershipRepository::new(&store)
            .add_member(
                &ns,
                &crate::test_support::enrol(&store, &ns, &member),
                GroupMemberRole::Member,
            )
            .unwrap();

        // A genuine member is still rejected by the handler's gate - the gate is
        // admin authority, not membership.
        assert!(
            MembershipRepository::new(&store)
                .is_member(&ns, &crate::test_support::account_for(&member))
                .unwrap(),
            "the rejected caller is genuinely a member - membership alone is not enough"
        );
        assert!(
            MembershipRepository::new(&store)
                .effective_capabilities(&ns, &crate::test_support::account_for(&member))
                .unwrap()
                .is_some(),
            "the rejected caller also passes the subscription-level gate"
        );
        assert!(
            authorize_migration_status(&store, &ns, &member).is_err(),
            "a non-admin member must be rejected by the migration-status admin gate"
        );
        // The admin passes the same gate.
        assert!(authorize_migration_status(&store, &ns, &admin).is_ok());
    }

    /// The bytes a shipped binary writes for a `Completed` record, laid out by
    /// hand. `status` sits mid-struct, so a field added inside the variant
    /// shifts `cascade_hlc`, `cascade_seq` and `to_state_version` and this row
    /// stops decoding - on every namespace that has already migrated once.
    fn shipped_completed_record(to_state_version: u32) -> Vec<u8> {
        use borsh::BorshSerialize;

        let mut bytes = Vec::new();
        "1".to_owned().serialize(&mut bytes).unwrap();
        "2".to_owned().serialize(&mut bytes).unwrap();
        None::<Vec<u8>>.serialize(&mut bytes).unwrap();
        0u64.serialize(&mut bytes).unwrap();
        PublicKey::from([0x01; 32]).serialize(&mut bytes).unwrap();
        // `Completed`: variant tag, then `completed_at` and nothing else.
        bytes.push(1);
        Some(1_700_001_000u64).serialize(&mut bytes).unwrap();
        // `cascade_hlc` then `cascade_seq`, both absent (one `0` tag each).
        bytes.extend_from_slice(&[0, 0]);
        to_state_version.serialize(&mut bytes).unwrap();
        bytes
    }

    /// `GET /migration-status` opens with this read, so a record it cannot
    /// decode errors the whole endpoint out for that namespace - and every
    /// namespace that has already migrated once holds exactly such a record the
    /// moment its node upgrades.
    #[test]
    fn rollup_reads_the_record_a_shipped_binary_wrote() {
        use calimero_store::layer::WriteLayer;

        let mut store = Store::new(Arc::new(InMemoryDB::owned()));
        let ns = ContextGroupId::from([0x88; 32]);
        MetaRepository::new(&store)
            .save(&ns, &meta(PublicKey::from([0xAD; 32])))
            .unwrap();
        let record = shipped_completed_record(2);
        store
            .put(
                &calimero_store::key::GroupUpgradeKey::new(ns.to_bytes()),
                calimero_store::slice::Slice::from(&record[..]),
            )
            .unwrap();

        let status = super::compute_namespace_rollup(&store, &ns, |_| None)
            .expect("a stored record must roll up, not error");
        assert_eq!(
            status.target_version, 2,
            "the target must come off the record, not a shifted read of it"
        );
        assert_eq!(
            status.fleet_completed_at, None,
            "nothing has stamped fleet convergence on this node yet"
        );
    }

    fn upgrade_record(
        from: &str,
        to: &str,
        to_state_version: u32,
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
            to_state_version,
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
            2,
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
            2,
            calimero_store::key::GroupUpgradeStatus::InProgress {
                total: 1,
                completed: 0,
                failed: 0,
            },
        );
        assert_eq!(super::derive_target_version(Some(&rec)), 2);
    }

    /// A record targeting semver 10.2.0 at ABI state 2 must roll up against 2.
    /// Parsing the semver yields 10, which every member on major 10 trivially
    /// satisfies — the false green this PR exists to remove.
    #[test]
    fn target_version_reads_the_abi_state_version() {
        let record = calimero_store::key::GroupUpgradeValue {
            from_version: "10.1.3".to_owned(),
            to_version: "10.2.0".to_owned(),
            migration: None,
            initiated_at: 0,
            initiated_by: PublicKey::from([0x01; 32]),
            status: calimero_store::key::GroupUpgradeStatus::InProgress {
                total: 1,
                completed: 0,
                failed: 0,
            },
            cascade_hlc: None,
            cascade_seq: None,
            to_state_version: 2,
        };

        assert_eq!(super::derive_target_version(Some(&record)), 2);
    }

    /// An unresolvable target must be unsatisfiable, never trivially satisfied.
    #[test]
    fn unresolvable_target_state_version_pins_to_max() {
        let record = calimero_store::key::GroupUpgradeValue {
            from_version: "10.1.3".to_owned(),
            to_version: "10.2.0".to_owned(),
            migration: None,
            initiated_at: 0,
            initiated_by: PublicKey::from([0x01; 32]),
            status: calimero_store::key::GroupUpgradeStatus::InProgress {
                total: 1,
                completed: 0,
                failed: 0,
            },
            cascade_hlc: None,
            cascade_seq: None,
            to_state_version: 0,
        };

        assert_eq!(super::derive_target_version(Some(&record)), u32::MAX);
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
                    3,
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
