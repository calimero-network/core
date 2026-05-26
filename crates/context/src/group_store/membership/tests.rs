//! Tests for `group_store::membership::*`. Extracted from the monolithic
//! `group_store/tests.rs` as part of issue #2306 (epic #2300).
//!
//! Test order is preserved from `tests.rs` to keep `git blame` useful;
//! helpers that are only used by membership tests came along (e.g.
//! `nest_for_test`). Helpers shared with non-membership tests
//! (`test_store`, `test_group_id`, `test_meta`, `dummy_member_removed_op`)
//! are imported from the parent `group_store::test_fixtures` module.

use std::sync::Arc;

use calimero_context_client::local_governance::GroupOp;
use calimero_context_config::types::ContextGroupId;
use calimero_primitives::application::ApplicationId;
use calimero_primitives::context::{ContextId, GroupMemberRole, UpgradePolicy};
use calimero_primitives::identity::PublicKey;
use calimero_store::db::InMemoryDB;
use calimero_store::key::{GroupMetaValue, GroupUpgradeStatus, GroupUpgradeValue};
use calimero_store::Store;

use super::super::*;
use super::super::test_fixtures::{
    dummy_member_removed_op, nest_for_test, sample_meta_with_admin, test_group_id, test_meta,
    test_store,
};

#[test]
fn add_and_check_membership() {
    let store = test_store();
    let gid = test_group_id();
    let pk = PublicKey::from([0x01; 32]);

    assert!(!check_group_membership(&store, &gid, &pk).unwrap());

    add_group_member(&store, &gid, &pk, GroupMemberRole::Admin).unwrap();
    assert!(check_group_membership(&store, &gid, &pk).unwrap());
    assert!(is_group_admin(&store, &gid, &pk).unwrap());
}

#[test]
fn remove_member() {
    let store = test_store();
    let gid = test_group_id();
    let pk = PublicKey::from([0x02; 32]);

    add_group_member(&store, &gid, &pk, GroupMemberRole::Member).unwrap();
    assert!(check_group_membership(&store, &gid, &pk).unwrap());

    remove_group_member(&store, &gid, &pk).unwrap();
    assert!(!check_group_membership(&store, &gid, &pk).unwrap());
}

#[test]
fn get_member_role() {
    let store = test_store();
    let gid = test_group_id();
    let admin = PublicKey::from([0x01; 32]);
    let member = PublicKey::from([0x02; 32]);

    add_group_member(&store, &gid, &admin, GroupMemberRole::Admin).unwrap();
    add_group_member(&store, &gid, &member, GroupMemberRole::Member).unwrap();

    assert_eq!(
        get_group_member_role(&store, &gid, &admin).unwrap(),
        Some(GroupMemberRole::Admin)
    );
    assert_eq!(
        get_group_member_role(&store, &gid, &member).unwrap(),
        Some(GroupMemberRole::Member)
    );
    assert!(!is_group_admin(&store, &gid, &member).unwrap());
}

#[test]
fn require_group_admin_rejects_non_admin() {
    let store = test_store();
    let gid = test_group_id();
    let member = PublicKey::from([0x03; 32]);

    add_group_member(&store, &gid, &member, GroupMemberRole::Member).unwrap();
    assert!(require_group_admin(&store, &gid, &member).is_err());
}

#[test]
fn membership_policy_guards_last_admin_and_tee_paths() {
    use calimero_context_client::local_governance::{GroupOp, SignedGroupOp};
    use calimero_primitives::identity::PrivateKey;
    use rand::rngs::OsRng;

    let mut rng = OsRng;
    let store = test_store();
    let gid = test_group_id();
    let admin = PrivateKey::random(&mut rng).public_key();
    let admin2 = PrivateKey::random(&mut rng).public_key();
    let member = PrivateKey::random(&mut rng).public_key();
    let outsider = PrivateKey::random(&mut rng).public_key();

    add_group_member(&store, &gid, &admin, GroupMemberRole::Admin).unwrap();
    add_group_member(&store, &gid, &member, GroupMemberRole::Member).unwrap();

    let membership = MembershipPolicy::new(&store, gid);
    assert!(membership.ensure_not_last_admin_removal(&admin).is_err());
    assert!(membership
        .ensure_not_last_admin_demotion(&admin, &GroupMemberRole::Member)
        .is_err());
    assert!(membership
        .ensure_not_last_admin_demotion(&admin, &GroupMemberRole::Admin)
        .is_ok());

    add_group_member(&store, &gid, &admin2, GroupMemberRole::Admin).unwrap();
    assert!(membership.ensure_not_last_admin_removal(&admin).is_ok());
    assert!(membership
        .ensure_not_last_admin_demotion(&admin, &GroupMemberRole::Member)
        .is_ok());

    assert!(membership
        .require_tee_attestation_verifier_membership(&member)
        .is_ok());
    assert!(membership
        .require_tee_attestation_verifier_membership(&outsider)
        .is_err());
    assert!(membership.read_required_tee_admission_policy().is_err());

    let signer_sk = PrivateKey::random(&mut rng);
    let policy_op = SignedGroupOp::sign(
        &signer_sk,
        gid.to_bytes(),
        vec![],
        [0u8; 32],
        1,
        GroupOp::TeeAdmissionPolicySet {
            allowed_mrtd: vec!["m1".to_owned()],
            allowed_rtmr0: vec!["r0".to_owned()],
            allowed_rtmr1: vec![],
            allowed_rtmr2: vec![],
            allowed_rtmr3: vec![],
            allowed_tcb_statuses: vec!["ok".to_owned()],
            accept_mock: false,
        },
    )
    .unwrap();
    append_op_log_entry(&store, &gid, 1, &borsh::to_vec(&policy_op).unwrap()).unwrap();

    let policy = membership.read_required_tee_admission_policy().unwrap();
    assert!(membership
        .validate_tee_attestation_allowlists(&policy, "m1", "r0", "x", "y", "z", "ok")
        .is_ok());
    assert!(membership
        .validate_tee_attestation_allowlists(&policy, "wrong", "r0", "x", "y", "z", "ok")
        .is_err());
    assert!(membership
        .validate_tee_attestation_allowlists(&policy, "m1", "wrong", "x", "y", "z", "ok")
        .is_err());

    let tee_joined = PrivateKey::random(&mut rng).public_key();
    assert!(!check_group_membership(&store, &gid, &tee_joined).unwrap());
    membership
        .admit_member_if_absent(&tee_joined, &GroupMemberRole::Member)
        .unwrap();
    assert!(check_group_membership(&store, &gid, &tee_joined).unwrap());
    membership
        .admit_member_if_absent(&tee_joined, &GroupMemberRole::Member)
        .unwrap();
    assert!(check_group_membership(&store, &gid, &tee_joined).unwrap());
}

#[test]
fn membership_policy_rules_report_rejection_reasons() {
    use super::policy_rules::{
        validate_tee_attestation_allowlists, MembershipPolicyRejection, TeeAllowlistPolicy,
        TeeAttestationClaims,
    };

    let policy = TeeAllowlistPolicy {
        allowed_mrtd: vec!["m-ok".to_owned()],
        allowed_rtmr0: vec!["r0-ok".to_owned()],
        allowed_rtmr1: vec![],
        allowed_rtmr2: vec![],
        allowed_rtmr3: vec![],
        allowed_tcb_statuses: vec!["ok".to_owned()],
    };
    let claims = TeeAttestationClaims {
        mrtd: "m-ok",
        rtmr0: "r0-ok",
        rtmr1: "anything",
        rtmr2: "anything",
        rtmr3: "anything",
        tcb_status: "ok",
    };

    assert!(validate_tee_attestation_allowlists(&policy, &claims).is_ok());

    let bad_mrtd = TeeAttestationClaims {
        mrtd: "m-bad",
        ..claims
    };
    let err = validate_tee_attestation_allowlists(&policy, &bad_mrtd).unwrap_err();
    assert_eq!(err.reason(), MembershipPolicyRejection::MrtdNotAllowed);

    let bad_rtmr0 = TeeAttestationClaims {
        rtmr0: "r0-bad",
        ..claims
    };
    let err = validate_tee_attestation_allowlists(&policy, &bad_rtmr0).unwrap_err();
    assert_eq!(err.reason(), MembershipPolicyRejection::Rtmr0NotAllowed);

    let bad_tcb = TeeAttestationClaims {
        tcb_status: "warn",
        ..claims
    };
    let err = validate_tee_attestation_allowlists(&policy, &bad_tcb).unwrap_err();
    assert_eq!(err.reason(), MembershipPolicyRejection::TcbStatusNotAllowed);
}

#[test]
fn membership_view_reports_admin_and_member_counts() {
    let store = test_store();
    let gid = test_group_id();
    let admin1 = PublicKey::from([0xD1; 32]);
    let admin2 = PublicKey::from([0xD2; 32]);
    let member = PublicKey::from([0xD3; 32]);

    add_group_member(&store, &gid, &admin1, GroupMemberRole::Admin).unwrap();
    add_group_member(&store, &gid, &admin2, GroupMemberRole::Admin).unwrap();
    add_group_member(&store, &gid, &member, GroupMemberRole::Member).unwrap();

    let view = GroupMembershipView::new(&store, gid);
    assert!(view.is_admin(&admin1).unwrap());
    assert!(!view.is_admin(&member).unwrap());
    assert_eq!(view.admin_count().unwrap(), 2);
    assert_eq!(view.member_count().unwrap(), 3);
}

#[test]
fn count_members_and_admins() {
    let store = test_store();
    let gid = test_group_id();

    assert_eq!(count_group_members(&store, &gid).unwrap(), 0);
    assert_eq!(count_group_admins(&store, &gid).unwrap(), 0);

    add_group_member(
        &store,
        &gid,
        &PublicKey::from([0x01; 32]),
        GroupMemberRole::Admin,
    )
    .unwrap();
    add_group_member(
        &store,
        &gid,
        &PublicKey::from([0x02; 32]),
        GroupMemberRole::Member,
    )
    .unwrap();
    add_group_member(
        &store,
        &gid,
        &PublicKey::from([0x03; 32]),
        GroupMemberRole::Admin,
    )
    .unwrap();

    assert_eq!(count_group_members(&store, &gid).unwrap(), 3);
    assert_eq!(count_group_admins(&store, &gid).unwrap(), 2);
}

#[test]
fn list_members_with_offset_and_limit() {
    let store = test_store();
    let gid = test_group_id();

    for i in 0u8..5 {
        let mut pk_bytes = [0u8; 32];
        pk_bytes[0] = i;
        add_group_member(
            &store,
            &gid,
            &PublicKey::from(pk_bytes),
            GroupMemberRole::Member,
        )
        .unwrap();
    }

    let all = list_group_members(&store, &gid, 0, 100).unwrap();
    assert_eq!(all.len(), 5);

    let page = list_group_members(&store, &gid, 1, 2).unwrap();
    assert_eq!(page.len(), 2);
}

#[test]
fn check_membership_open_subgroup_inherits_parent_with_default_cap() {
    use calimero_context_config::{MemberCapabilities, VisibilityMode};

    let store = test_store();
    let parent = ContextGroupId::from([0xB0; 32]);
    let child = ContextGroupId::from([0xB1; 32]);
    let alice = PublicKey::from([0x01; 32]);

    nest_for_test(&store, &parent, &child);

    // Alice is a direct member of the parent with the default cap set.
    add_group_member(&store, &parent, &alice, GroupMemberRole::Member).unwrap();
    set_member_capability(
        &store,
        &parent,
        &alice,
        MemberCapabilities::CAN_JOIN_OPEN_SUBGROUPS,
    )
    .unwrap();

    // Child is `Open`. Alice should be inherited as a member.
    set_subgroup_visibility(&store, &child, VisibilityMode::Open).unwrap();
    assert!(check_group_membership(&store, &child, &alice).unwrap());
}

#[test]
fn check_membership_path_inherited_when_member_added_after_default_caps() {
    use calimero_context_config::{MemberCapabilities, VisibilityMode};

    let store = test_store();
    let ns = ContextGroupId::from([0xB4; 32]);
    let child = ContextGroupId::from([0xB5; 32]);
    let bob = PublicKey::from([0x02; 32]);

    nest_for_test(&store, &ns, &child);
    set_subgroup_visibility(&store, &child, VisibilityMode::Open).unwrap();

    // Correct ordering: default caps set FIRST, then the member is
    // added — `add_group_member` copies the default into bob's
    // per-member capability row.
    set_default_capabilities(&store, &ns, MemberCapabilities::CAN_JOIN_OPEN_SUBGROUPS).unwrap();
    add_group_member(&store, &ns, &bob, GroupMemberRole::Member).unwrap();

    let path = check_group_membership_path(&store, &child, &bob).unwrap();
    assert!(
        matches!(path, MembershipPath::Inherited { .. }),
        "member added after default caps must inherit Open-subgroup membership, got {path:?}"
    );
}

#[test]
fn check_membership_path_none_when_member_added_before_default_caps() {
    use calimero_context_config::{MemberCapabilities, VisibilityMode};

    let store = test_store();
    let ns = ContextGroupId::from([0xB6; 32]);
    let child = ContextGroupId::from([0xB7; 32]);
    let bob = PublicKey::from([0x02; 32]);

    nest_for_test(&store, &ns, &child);
    set_subgroup_visibility(&store, &child, VisibilityMode::Open).unwrap();

    // Buggy ordering (what the pre-fix `join_group` catch-up did when a
    // node caught up on an earlier member's `MemberJoined` before its
    // own `set_default_capabilities` ran): member added while default
    // caps are still unset → no per-member capability row is written.
    add_group_member(&store, &ns, &bob, GroupMemberRole::Member).unwrap();
    set_default_capabilities(&store, &ns, MemberCapabilities::CAN_JOIN_OPEN_SUBGROUPS).unwrap();

    // The later `set_default_capabilities` does NOT retroactively
    // materialize bob's per-member cap, so the inheritance check still
    // returns `None`. This is exactly the state that produced
    // `MemberJoinedOpen rejected: no membership path` on the
    // later-joining peer; the `join_group` handler fix prevents it by
    // ordering `set_default_capabilities` before the catch-up apply.
    let path = check_group_membership_path(&store, &child, &bob).unwrap();
    assert!(
        matches!(path, MembershipPath::None),
        "member added before default caps has no per-member cap row → no inherited path, got {path:?}"
    );
}

#[test]
fn check_membership_restricted_subgroup_does_not_inherit() {
    use calimero_context_config::{MemberCapabilities, VisibilityMode};

    let store = test_store();
    let parent = ContextGroupId::from([0xB2; 32]);
    let child = ContextGroupId::from([0xB3; 32]);
    let alice = PublicKey::from([0x01; 32]);

    nest_for_test(&store, &parent, &child);
    add_group_member(&store, &parent, &alice, GroupMemberRole::Member).unwrap();
    set_member_capability(
        &store,
        &parent,
        &alice,
        MemberCapabilities::CAN_JOIN_OPEN_SUBGROUPS,
    )
    .unwrap();

    // Restricted child blocks inheritance even when the cap is set.
    set_subgroup_visibility(&store, &child, VisibilityMode::Restricted).unwrap();
    assert!(!check_group_membership(&store, &child, &alice).unwrap());
}

#[test]
fn check_membership_restricted_wall_blocks_grandparent_inheritance() {
    use calimero_context_config::{MemberCapabilities, VisibilityMode};

    // namespace -> restricted_mid -> open_leaf
    let store = test_store();
    let namespace = ContextGroupId::from([0xC0; 32]);
    let mid = ContextGroupId::from([0xC1; 32]);
    let leaf = ContextGroupId::from([0xC2; 32]);
    let alice = PublicKey::from([0x01; 32]);

    nest_for_test(&store, &namespace, &mid);
    nest_for_test(&store, &mid, &leaf);

    add_group_member(&store, &namespace, &alice, GroupMemberRole::Member).unwrap();
    set_member_capability(
        &store,
        &namespace,
        &alice,
        MemberCapabilities::CAN_JOIN_OPEN_SUBGROUPS,
    )
    .unwrap();

    set_subgroup_visibility(&store, &mid, VisibilityMode::Restricted).unwrap();
    set_subgroup_visibility(&store, &leaf, VisibilityMode::Open).unwrap();

    // The walk hits `mid` (Restricted) and stops before reaching the
    // namespace; alice is not inherited into `leaf`.
    assert!(!check_group_membership(&store, &leaf, &alice).unwrap());
}

#[test]
fn check_membership_open_chain_walks_to_root() {
    use calimero_context_config::{MemberCapabilities, VisibilityMode};

    // namespace -> open_mid -> open_leaf, member only at namespace
    let store = test_store();
    let namespace = ContextGroupId::from([0xD0; 32]);
    let mid = ContextGroupId::from([0xD1; 32]);
    let leaf = ContextGroupId::from([0xD2; 32]);
    let alice = PublicKey::from([0x01; 32]);

    nest_for_test(&store, &namespace, &mid);
    nest_for_test(&store, &mid, &leaf);

    add_group_member(&store, &namespace, &alice, GroupMemberRole::Member).unwrap();
    set_member_capability(
        &store,
        &namespace,
        &alice,
        MemberCapabilities::CAN_JOIN_OPEN_SUBGROUPS,
    )
    .unwrap();

    set_subgroup_visibility(&store, &mid, VisibilityMode::Open).unwrap();
    set_subgroup_visibility(&store, &leaf, VisibilityMode::Open).unwrap();

    assert!(check_group_membership(&store, &leaf, &alice).unwrap());
}

#[test]
fn check_membership_unset_visibility_treated_as_restricted() {
    let store = test_store();
    let parent = ContextGroupId::from([0xE0; 32]);
    let child = ContextGroupId::from([0xE1; 32]);
    let alice = PublicKey::from([0x01; 32]);

    nest_for_test(&store, &parent, &child);
    add_group_member(&store, &parent, &alice, GroupMemberRole::Member).unwrap();
    set_member_capability(
        &store,
        &parent,
        &alice,
        calimero_context_config::MemberCapabilities::CAN_JOIN_OPEN_SUBGROUPS,
    )
    .unwrap();

    // No `subgroup_visibility` set on `child` — should behave as Restricted.
    assert!(!check_group_membership(&store, &child, &alice).unwrap());
}

#[test]
fn check_membership_open_subgroup_blocked_when_cap_revoked() {
    use calimero_context_config::VisibilityMode;

    let store = test_store();
    let parent = ContextGroupId::from([0xF0; 32]);
    let child = ContextGroupId::from([0xF1; 32]);
    let alice = PublicKey::from([0x01; 32]);

    nest_for_test(&store, &parent, &child);
    add_group_member(&store, &parent, &alice, GroupMemberRole::Member).unwrap();
    // Cap explicitly cleared (admin used the deny-list).
    set_member_capability(&store, &parent, &alice, 0).unwrap();

    set_subgroup_visibility(&store, &child, VisibilityMode::Open).unwrap();
    assert!(!check_group_membership(&store, &child, &alice).unwrap());
}

#[test]
fn check_membership_open_subgroup_admin_inherits_without_cap() {
    use calimero_context_config::VisibilityMode;

    let store = test_store();
    let parent = ContextGroupId::from([0x10; 32]);
    let child = ContextGroupId::from([0x11; 32]);
    let admin = PublicKey::from([0x01; 32]);

    nest_for_test(&store, &parent, &child);
    add_group_member(&store, &parent, &admin, GroupMemberRole::Admin).unwrap();
    // Cap cleared — but admin override kicks in.
    set_member_capability(&store, &parent, &admin, 0).unwrap();

    set_subgroup_visibility(&store, &child, VisibilityMode::Open).unwrap();
    assert!(check_group_membership(&store, &child, &admin).unwrap());
}

#[test]
fn check_membership_anchor_cap_check_uses_deepest_direct_membership() {
    use calimero_context_config::{MemberCapabilities, VisibilityMode};

    // namespace -> mid -> open_leaf
    // Alice is a direct member of BOTH `namespace` (cap set) and `mid`
    // (cap cleared). For `open_leaf`, the walk anchors at `mid` (the
    // deepest direct membership), where cap is cleared → false.
    let store = test_store();
    let namespace = ContextGroupId::from([0x20; 32]);
    let mid = ContextGroupId::from([0x21; 32]);
    let leaf = ContextGroupId::from([0x22; 32]);
    let alice = PublicKey::from([0x01; 32]);

    nest_for_test(&store, &namespace, &mid);
    nest_for_test(&store, &mid, &leaf);

    add_group_member(&store, &namespace, &alice, GroupMemberRole::Member).unwrap();
    set_member_capability(
        &store,
        &namespace,
        &alice,
        MemberCapabilities::CAN_JOIN_OPEN_SUBGROUPS,
    )
    .unwrap();

    add_group_member(&store, &mid, &alice, GroupMemberRole::Member).unwrap();
    set_member_capability(&store, &mid, &alice, 0).unwrap();

    set_subgroup_visibility(&store, &mid, VisibilityMode::Open).unwrap();
    set_subgroup_visibility(&store, &leaf, VisibilityMode::Open).unwrap();

    assert!(!check_group_membership(&store, &leaf, &alice).unwrap());
}

#[test]
fn enumerate_inherited_members_includes_open_subgroup_joiner() {
    use calimero_context_config::{MemberCapabilities, VisibilityMode};

    // namespace (root) -> reports (Open subgroup).
    // Alice is a direct Admin of `reports`; Bob is a namespace member
    // who joined `reports` via inheritance — no direct row in `reports`.
    let store = test_store();
    let namespace = ContextGroupId::from([0x30; 32]);
    let reports = ContextGroupId::from([0x31; 32]);
    let alice = PublicKey::from([0x01; 32]);
    let bob = PublicKey::from([0x02; 32]);

    nest_for_test(&store, &namespace, &reports);

    // Bob is a direct member of the namespace root with the join cap.
    add_group_member(&store, &namespace, &bob, GroupMemberRole::Member).unwrap();
    set_member_capability(
        &store,
        &namespace,
        &bob,
        MemberCapabilities::CAN_JOIN_OPEN_SUBGROUPS,
    )
    .unwrap();

    // Alice is an explicit Admin of the Open `reports` subgroup.
    add_group_member(&store, &reports, &alice, GroupMemberRole::Admin).unwrap();
    set_subgroup_visibility(&store, &reports, VisibilityMode::Open).unwrap();

    // Contract sanity: Bob *is* a member of `reports` by inheritance...
    assert!(check_group_membership(&store, &reports, &bob).unwrap());
    // ...but he has no stored row, so `list_group_members` omits him —
    // this is the gap issue #2371 reports.
    let stored = list_group_members(&store, &reports, 0, usize::MAX).unwrap();
    assert!(
        stored.iter().all(|(pk, _)| *pk != bob),
        "precondition: inherited joiner has no stored GroupMember row"
    );

    // `enumerate_inherited_members` recovers Bob as an inherited member.
    let inherited = enumerate_inherited_members(&store, &reports).unwrap();
    assert!(
        inherited
            .iter()
            .any(|(pk, role)| *pk == bob && *role == GroupMemberRole::Member),
        "expected inherited joiner Bob (Member) in enumerate_inherited_members, got {inherited:?}"
    );
    // Alice is a *direct* member of `reports` — she must not be
    // double-reported as an inherited member.
    assert!(
        inherited.iter().all(|(pk, _)| *pk != alice),
        "direct member Alice must not appear as an inherited member"
    );
}

#[test]
fn enumerate_inherited_members_excludes_deny_listed_member() {
    // A member kicked from an Open subgroup via `MemberRemoved` /
    // `MemberLeft` is deny-listed on that subgroup. Because the kick
    // cannot delete a row that does not exist (the member is there by
    // namespace-level inheritance), the deny-list IS the removal.
    // `enumerate_inherited_members` — and therefore `list_group_members`
    // — must exclude the deny-listed member so an admin who kicks
    // someone no longer sees them in the channel's member list.
    use calimero_context_config::{MemberCapabilities, VisibilityMode};

    let store = test_store();
    let namespace = ContextGroupId::from([0x38; 32]);
    let reports = ContextGroupId::from([0x39; 32]);
    let bob = PublicKey::from([0x02; 32]);

    nest_for_test(&store, &namespace, &reports);
    add_group_member(&store, &namespace, &bob, GroupMemberRole::Member).unwrap();
    set_member_capability(
        &store,
        &namespace,
        &bob,
        MemberCapabilities::CAN_JOIN_OPEN_SUBGROUPS,
    )
    .unwrap();
    set_subgroup_visibility(&store, &reports, VisibilityMode::Open).unwrap();

    // Pre-kick: Bob is an inherited member of `reports`.
    let inherited = enumerate_inherited_members(&store, &reports).unwrap();
    assert!(
        inherited.iter().any(|(pk, _)| *pk == bob),
        "precondition: Bob inherits membership of the Open subgroup"
    );

    // Kick: deny-list Bob on `reports` (what `MemberRemoved` /
    // `MemberLeft` apply does for a subgroup-level removal).
    mark_denied(&store, &reports, &bob).unwrap();

    let after_kick = enumerate_inherited_members(&store, &reports).unwrap();
    assert!(
        after_kick.iter().all(|(pk, _)| *pk != bob),
        "deny-listed (kicked) member must NOT appear as an inherited member, got {after_kick:?}"
    );

    // Rejoin clears the deny-list — Bob reappears.
    clear_denied(&store, &reports, &bob).unwrap();
    let after_rejoin = enumerate_inherited_members(&store, &reports).unwrap();
    assert!(
        after_rejoin.iter().any(|(pk, _)| *pk == bob),
        "after clear_denied (rejoin) Bob must reappear as an inherited member"
    );
}

#[test]
fn enumerate_inherited_members_reports_inherited_admin_role() {
    use calimero_context_config::VisibilityMode;

    // A namespace admin who never explicitly joined the Open subgroup
    // still inherits admin authority into it (admin override). The
    // enumerated entry must carry the `Admin` role, not `Member`.
    let store = test_store();
    let namespace = ContextGroupId::from([0x32; 32]);
    let reports = ContextGroupId::from([0x33; 32]);
    let admin = PublicKey::from([0x01; 32]);

    nest_for_test(&store, &namespace, &reports);
    add_group_member(&store, &namespace, &admin, GroupMemberRole::Admin).unwrap();
    set_subgroup_visibility(&store, &reports, VisibilityMode::Open).unwrap();

    let inherited = enumerate_inherited_members(&store, &reports).unwrap();
    assert!(
        inherited
            .iter()
            .any(|(pk, role)| *pk == admin && *role == GroupMemberRole::Admin),
        "inherited admin must be reported with the Admin role, got {inherited:?}"
    );
}

#[test]
fn enumerate_inherited_members_empty_for_restricted_subgroup() {
    use calimero_context_config::{MemberCapabilities, VisibilityMode};

    // A `Restricted` subgroup is a wall: parent members are not
    // inherited, so the enumeration is empty.
    let store = test_store();
    let namespace = ContextGroupId::from([0x34; 32]);
    let reports = ContextGroupId::from([0x35; 32]);
    let bob = PublicKey::from([0x02; 32]);

    nest_for_test(&store, &namespace, &reports);
    add_group_member(&store, &namespace, &bob, GroupMemberRole::Member).unwrap();
    set_member_capability(
        &store,
        &namespace,
        &bob,
        MemberCapabilities::CAN_JOIN_OPEN_SUBGROUPS,
    )
    .unwrap();
    set_subgroup_visibility(&store, &reports, VisibilityMode::Restricted).unwrap();

    let inherited = enumerate_inherited_members(&store, &reports).unwrap();
    assert!(
        inherited.is_empty(),
        "Restricted subgroup must inherit no members, got {inherited:?}"
    );
}

#[test]
fn enumerate_inherited_members_bails_on_depth_overflow() {
    use calimero_context_config::VisibilityMode;

    use super::super::namespace::MAX_NAMESPACE_DEPTH;

    // A parent chain longer than MAX_NAMESPACE_DEPTH with no Restricted
    // wall exhausts the walk bound. Like its sibling chain-walkers
    // (`check_group_membership_path`, `is_inherited_admin`),
    // `enumerate_inherited_members` must bail on a suspected cycle
    // rather than silently return a partial member set.
    let store = test_store();
    let root = ContextGroupId::from([0x40; 32]);
    let mut prev = root;
    for i in 0..(MAX_NAMESPACE_DEPTH + 2) {
        // Encode `i` across two bytes so distinct levels never alias,
        // regardless of how large MAX_NAMESPACE_DEPTH grows — a single
        // wrapping byte would collide past 255 and build a real cycle
        // instead of a long chain, passing the test for the wrong reason.
        let mut bytes = [0x41u8; 32];
        bytes[..2].copy_from_slice(&(i as u16).to_le_bytes());
        let next = ContextGroupId::from(bytes);
        nest_for_test(&store, &prev, &next);
        set_subgroup_visibility(&store, &next, VisibilityMode::Open).unwrap();
        prev = next;
    }

    let res = enumerate_inherited_members(&store, &prev);
    assert!(
        res.is_err(),
        "enumerate_inherited_members must bail on MAX_NAMESPACE_DEPTH overflow, got {res:?}"
    );
}

#[test]
fn enumerate_inherited_members_resolves_at_max_namespace_depth_boundary() {
    use calimero_context_config::{MemberCapabilities, VisibilityMode};

    use super::super::namespace::MAX_NAMESPACE_DEPTH;

    // Refutes the claim that `enumerate_inherited_members` walks one
    // ancestor short of `check_group_membership_path`. Build a fully
    // Open chain of exactly MAX_NAMESPACE_DEPTH edges (ns -> g_1 -> ...
    // -> leaf) with the sole member at the *deepest* ancestor (`ns`).
    // If the walk fell an iteration short, `alice` would be missed; it
    // must enumerate her, agreeing with `check_group_membership` (see
    // `auth_and_crypto_walks_agree_at_max_namespace_depth_boundary`).
    let store = test_store();
    let ns = ContextGroupId::from([0x60; 32]);
    let mut nodes = vec![ns];
    for i in 1..=MAX_NAMESPACE_DEPTH {
        let g = ContextGroupId::from([0x60u8.wrapping_add(i as u8); 32]);
        nest_for_test(&store, nodes.last().unwrap(), &g);
        set_subgroup_visibility(&store, &g, VisibilityMode::Open).unwrap();
        nodes.push(g);
    }
    let leaf = *nodes.last().unwrap();

    let alice = PublicKey::from([0x01; 32]);
    add_group_member(&store, &ns, &alice, GroupMemberRole::Member).unwrap();
    set_member_capability(
        &store,
        &ns,
        &alice,
        MemberCapabilities::CAN_JOIN_OPEN_SUBGROUPS,
    )
    .unwrap();

    assert!(
        check_group_membership(&store, &leaf, &alice).unwrap(),
        "precondition: check_group_membership resolves at the boundary"
    );

    let inherited = enumerate_inherited_members(&store, &leaf).unwrap();
    assert!(
        inherited.iter().any(|(pk, _)| *pk == alice),
        "enumerate_inherited_members must reach the deepest ancestor at \
         MAX_NAMESPACE_DEPTH, matching check_group_membership; got {inherited:?}"
    );
}

#[test]
fn inherited_admin_walk_independent_of_direct_non_admin_membership() {
    use calimero_context_config::VisibilityMode;

    use super::is_inherited_admin;

    // Bugbot finding (PR #2261): the previous `is_admin` reused
    // `check_group_membership_path`, which short-circuits to `Direct`
    // when the identity has *any* direct membership row in the target
    // subgroup — even a non-admin `Member` row. That suppressed
    // inherited admin authority for parent admins who happened to
    // also be added as explicit subgroup members. The dedicated
    // `is_inherited_admin` walk is independent of non-admin direct
    // membership.
    let store = test_store();
    let parent = ContextGroupId::from([0x50; 32]);
    let child = ContextGroupId::from([0x51; 32]);
    let alice = PublicKey::from([0x01; 32]);

    nest_for_test(&store, &parent, &child);

    // Alice is namespace admin AND a non-admin direct member of the
    // child subgroup (e.g. she opted into a subgroup-specific role
    // for visibility, but her parent admin authority should still
    // apply).
    add_group_member(&store, &parent, &alice, GroupMemberRole::Admin).unwrap();
    add_group_member(&store, &child, &alice, GroupMemberRole::Member).unwrap();

    set_subgroup_visibility(&store, &child, VisibilityMode::Open).unwrap();

    // Inherited admin authority must hold despite Alice's direct
    // non-admin membership in `child`.
    assert!(
        is_inherited_admin(&store, &child, &alice).unwrap(),
        "parent admin should retain admin authority in child subgroup \
         regardless of any direct non-admin membership row"
    );
}

#[test]
fn membership_path_inherited_admin_overrides_anchor_cap_denial() {
    use calimero_context_config::{MemberCapabilities, VisibilityMode};

    // Bugbot finding (PR #2261, comment 3146210600): governance
    // (`is_inherited_admin`) and context-join
    // (`check_group_membership_path`) used to disagree when an
    // identity held admin authority at a higher ancestor but ALSO
    // happened to have a direct non-admin row at an intermediate
    // level with `CAN_JOIN_OPEN_SUBGROUPS` cleared. The old walk
    // anchored at the intermediate row and denied join — yielding
    // the confusing "can govern but cannot join" UX.
    //
    // After the fix, admin authority cascades: the walk keeps going
    // past a non-admin direct row and finds the parent admin
    // anchor, returning `Inherited { via_admin: true }`.
    let store = test_store();
    let ns = ContextGroupId::from([0xE0; 32]);
    let mid = ContextGroupId::from([0xE1; 32]);
    let leaf = ContextGroupId::from([0xE2; 32]);
    let alice = PublicKey::from([0x01; 32]);

    nest_for_test(&store, &ns, &mid);
    nest_for_test(&store, &mid, &leaf);

    // Alice: namespace admin AND a direct non-admin Member at `mid`
    // with the join cap explicitly cleared.
    add_group_member(&store, &ns, &alice, GroupMemberRole::Admin).unwrap();
    add_group_member(&store, &mid, &alice, GroupMemberRole::Member).unwrap();
    set_member_capability(&store, &mid, &alice, 0).unwrap();

    set_subgroup_visibility(&store, &mid, VisibilityMode::Open).unwrap();
    set_subgroup_visibility(&store, &leaf, VisibilityMode::Open).unwrap();

    // Both authorization surfaces must agree: Alice is authorized.
    assert!(super::is_inherited_admin(&store, &leaf, &alice).unwrap());
    let path = check_group_membership_path(&store, &leaf, &alice).unwrap();
    match path {
        super::MembershipPath::Inherited {
            anchor,
            via_admin: true,
        } => {
            assert_eq!(
                anchor, ns,
                "admin anchor should be the namespace, not the intermediate `mid` row"
            );
        }
        other => panic!(
            "expected Inherited{{ via_admin: true, anchor: ns }} for parent admin, got {:?}",
            other
        ),
    }

    // Sanity: a non-admin in the same shape must still be denied —
    // the fix does NOT widen authorization for non-admins, only
    // honors admin authority that already exists higher up.
    let bob = PublicKey::from([0x02; 32]);
    add_group_member(&store, &ns, &bob, GroupMemberRole::Member).unwrap();
    set_member_capability(
        &store,
        &ns,
        &bob,
        MemberCapabilities::CAN_JOIN_OPEN_SUBGROUPS,
    )
    .unwrap();
    add_group_member(&store, &mid, &bob, GroupMemberRole::Member).unwrap();
    set_member_capability(&store, &mid, &bob, 0).unwrap();
    assert!(
        !check_group_membership(&store, &leaf, &bob).unwrap(),
        "non-admin with cleared cap at intermediate anchor must still be denied; \
         the fix only cascades *admin* authority, not arbitrary parent membership"
    );
}

#[test]
fn auth_and_crypto_walks_agree_at_max_namespace_depth_boundary() {
    use calimero_context_config::{MemberCapabilities, VisibilityMode};

    use super::super::namespace::MAX_NAMESPACE_DEPTH;
    use super::is_inherited_admin; use super::super::is_open_chain_to_namespace;

    // Bugbot finding (PR #2261, comment 3146841673): at chain length
    // exactly `MAX_NAMESPACE_DEPTH`, `is_open_chain_to_namespace`
    // succeeds (encrypt path selects namespace key) while the
    // membership walks bailed with a spurious cycle error (auth path
    // refused). The two layers must agree on the corruption signal:
    // either both succeed or both bail. The fix bumps the membership
    // walks to `MAX_NAMESPACE_DEPTH + 1` iterations so they have the
    // same effective reach as the chain check.
    let store = test_store();

    // Build chain of length MAX_NAMESPACE_DEPTH from leaf to ns:
    // ns -> g_1 -> g_2 -> ... -> g_{MAX-1} -> leaf
    // (i.e. MAX_NAMESPACE_DEPTH parent-edges separate `leaf` from `ns`.)
    let ns = ContextGroupId::from([0xF0; 32]);
    let mut nodes = vec![ns];
    for i in 1..=MAX_NAMESPACE_DEPTH {
        let g = ContextGroupId::from([0xF0u8.wrapping_add(i as u8); 32]);
        nest_for_test(&store, nodes.last().unwrap(), &g);
        // Mark every non-root link `Open` so the chain is fully open.
        set_subgroup_visibility(&store, &g, VisibilityMode::Open).unwrap();
        nodes.push(g);
    }
    let leaf = *nodes.last().unwrap();

    // Sanity: chain check succeeds at the boundary.
    assert!(
        is_open_chain_to_namespace(&store, &leaf, &ns).unwrap(),
        "is_open_chain_to_namespace should resolve at chain length MAX_NAMESPACE_DEPTH"
    );

    // The bug: membership walks used to bail here. After the fix,
    // they must resolve to a definite answer (no cycle error).
    let alice = PublicKey::from([0x01; 32]);
    add_group_member(&store, &ns, &alice, GroupMemberRole::Member).unwrap();
    set_member_capability(
        &store,
        &ns,
        &alice,
        MemberCapabilities::CAN_JOIN_OPEN_SUBGROUPS,
    )
    .unwrap();

    // is_inherited_admin: alice is not admin anywhere → should
    // resolve to false (NOT bail).
    assert!(
        matches!(is_inherited_admin(&store, &leaf, &alice), Ok(false)),
        "is_inherited_admin must terminate at chain length MAX_NAMESPACE_DEPTH, not bail"
    );

    // check_group_membership: alice has CAN_JOIN_OPEN_SUBGROUPS at
    // the namespace, all intermediate links are Open → should
    // resolve to true via inheritance, not bail.
    assert!(
        matches!(check_group_membership(&store, &leaf, &alice), Ok(true)),
        "check_group_membership must resolve at chain length MAX_NAMESPACE_DEPTH, not bail"
    );

    // Promoting alice to admin should also be observed (governance
    // surface in agreement).
    let bob = PublicKey::from([0x02; 32]);
    add_group_member(&store, &ns, &bob, GroupMemberRole::Admin).unwrap();
    assert!(
        matches!(is_inherited_admin(&store, &leaf, &bob), Ok(true)),
        "inherited admin authority must reach the leaf at chain length MAX_NAMESPACE_DEPTH"
    );
}

#[test]
fn has_direct_group_member_ignores_open_chain_inheritance() {
    use calimero_context_config::VisibilityMode;

    use super::has_direct_group_member;

    // Bugbot finding (PR #2261): a previous version of the bootstrap /
    // dedup guards in `store_group_meta`, `apply_member_joined`, and
    // `admit_tee_node` used the inheritance-aware `check_group_membership`,
    // which would silently report `true` for an identity that holds
    // membership only via an Open parent — and skip writing the direct
    // row that those handlers exist to create. `has_direct_group_member`
    // is the direct-only counterpart that those guards must use.
    let store = test_store();
    let parent = ContextGroupId::from([0x60; 32]);
    let child = ContextGroupId::from([0x61; 32]);
    let alice = PublicKey::from([0x01; 32]);

    nest_for_test(&store, &parent, &child);
    add_group_member(&store, &parent, &alice, GroupMemberRole::Admin).unwrap();
    set_subgroup_visibility(&store, &child, VisibilityMode::Open).unwrap();

    // Inheritance-aware path *should* see Alice (admin inheritance from parent).
    assert!(check_group_membership(&store, &child, &alice).unwrap());

    // Direct-only path *must not* see her — that's exactly the signal
    // the bootstrap/dedup guards need to know they still have to write
    // the direct row.
    assert!(
        !has_direct_group_member(&store, &child, &alice).unwrap(),
        "has_direct_group_member must ignore Open-chain inheritance and \
         report only on the direct membership row"
    );

    // After explicitly adding her to the child, both views agree.
    add_group_member(&store, &child, &alice, GroupMemberRole::Member).unwrap();
    assert!(has_direct_group_member(&store, &child, &alice).unwrap());
    assert!(check_group_membership(&store, &child, &alice).unwrap());
}

#[test]
fn check_membership_direct_member_of_subgroup_always_passes() {
    use calimero_context_config::VisibilityMode;

    // Direct membership short-circuits the walk regardless of visibility
    // setting on the subgroup.
    let store = test_store();
    let parent = ContextGroupId::from([0x30; 32]);
    let child = ContextGroupId::from([0x31; 32]);
    let alice = PublicKey::from([0x01; 32]);

    nest_for_test(&store, &parent, &child);
    set_subgroup_visibility(&store, &child, VisibilityMode::Restricted).unwrap();

    // No parent membership; alice is added directly to the Restricted child.
    add_group_member(&store, &child, &alice, GroupMemberRole::Member).unwrap();
    assert!(check_group_membership(&store, &child, &alice).unwrap());
}

#[cfg(test)]
mod auto_follow_tests {
    use calimero_context_client::local_governance::{GroupOp, SignedGroupOp};
    use calimero_primitives::identity::PrivateKey;
    use rand::rngs::OsRng;

    use super::*;
    use crate::group_store::{
        add_group_member, apply_local_signed_group_op, get_group_member_value,
    };

    fn seed(
        rng: &mut OsRng,
    ) -> (
        calimero_store::Store,
        calimero_context_config::types::ContextGroupId,
        [u8; 32],
        PrivateKey,
        PrivateKey,
    ) {
        let store = test_store();
        let gid = test_group_id();
        let gid_bytes = gid.to_bytes();
        let admin_sk = PrivateKey::random(rng);
        let member_sk = PrivateKey::random(rng);
        add_group_member(&store, &gid, &admin_sk.public_key(), GroupMemberRole::Admin).unwrap();
        add_group_member(
            &store,
            &gid,
            &member_sk.public_key(),
            GroupMemberRole::Member,
        )
        .unwrap();
        (store, gid, gid_bytes, admin_sk, member_sk)
    }

    #[test]
    fn admin_can_set_member_auto_follow() {
        let mut rng = OsRng;
        let (store, gid, gid_bytes, admin_sk, member_sk) = seed(&mut rng);

        let op = SignedGroupOp::sign(
            &admin_sk,
            gid_bytes,
            vec![],
            [0u8; 32],
            1,
            GroupOp::MemberSetAutoFollow {
                target: member_sk.public_key(),
                auto_follow_contexts: true,
                auto_follow_subgroups: true,
            },
        )
        .unwrap();
        apply_local_signed_group_op(&store, &op).unwrap();

        let val = get_group_member_value(&store, &gid, &member_sk.public_key())
            .unwrap()
            .unwrap();
        assert!(val.auto_follow.contexts);
        assert!(val.auto_follow.subgroups);
    }

    #[test]
    fn member_can_set_own_auto_follow() {
        let mut rng = OsRng;
        let (store, gid, gid_bytes, _admin_sk, member_sk) = seed(&mut rng);

        let op = SignedGroupOp::sign(
            &member_sk,
            gid_bytes,
            vec![],
            [0u8; 32],
            1,
            GroupOp::MemberSetAutoFollow {
                target: member_sk.public_key(),
                auto_follow_contexts: true,
                auto_follow_subgroups: false,
            },
        )
        .unwrap();
        apply_local_signed_group_op(&store, &op).unwrap();

        let val = get_group_member_value(&store, &gid, &member_sk.public_key())
            .unwrap()
            .unwrap();
        assert!(val.auto_follow.contexts);
        assert!(!val.auto_follow.subgroups);
    }

    #[test]
    fn non_admin_cannot_set_others_auto_follow() {
        let mut rng = OsRng;
        let (store, gid, gid_bytes, _admin_sk, member_sk) = seed(&mut rng);

        // `other_sk` is a real member of the group — we add them first so
        // the authorization check is the reason the op is rejected, not a
        // missing-target lookup. If the handler's check order is ever
        // refactored to look up the target before checking auth, this
        // test would still correctly assert "non-admin, non-self rejected".
        let other_sk = PrivateKey::random(&mut rng);
        add_group_member(
            &store,
            &gid,
            &other_sk.public_key(),
            GroupMemberRole::Member,
        )
        .unwrap();

        let op = SignedGroupOp::sign(
            &member_sk,
            gid_bytes,
            vec![],
            [0u8; 32],
            1,
            GroupOp::MemberSetAutoFollow {
                target: other_sk.public_key(),
                auto_follow_contexts: true,
                auto_follow_subgroups: false,
            },
        )
        .unwrap();
        let err = apply_local_signed_group_op(&store, &op).unwrap_err();
        assert!(err.to_string().contains("auto-follow"));

        // Sanity: the target's flags were not mutated by the
        // rejected op. The target was added via the seed() helper
        // which uses `add_group_member` directly — with the new
        // default that means {contexts: true, subgroups: false}.
        // The point of this test is that the failed op didn't
        // SHIFT the values, not that they were originally false.
        let val = get_group_member_value(&store, &gid, &other_sk.public_key())
            .unwrap()
            .unwrap();
        assert!(val.auto_follow.contexts, "default contexts=true preserved");
        assert!(
            !val.auto_follow.subgroups,
            "default subgroups=false preserved"
        );
    }

    #[test]
    fn rejects_non_member_target() {
        let mut rng = OsRng;
        let (store, _gid, gid_bytes, admin_sk, _member_sk) = seed(&mut rng);
        let stranger = PrivateKey::random(&mut rng).public_key();

        let op = SignedGroupOp::sign(
            &admin_sk,
            gid_bytes,
            vec![],
            [0u8; 32],
            1,
            GroupOp::MemberSetAutoFollow {
                target: stranger,
                auto_follow_contexts: true,
                auto_follow_subgroups: true,
            },
        )
        .unwrap();
        let err = apply_local_signed_group_op(&store, &op).unwrap_err();
        assert!(err.to_string().contains("not a member"));
    }

    #[test]
    fn default_flags_match_default_impl_and_preserved_on_role_change() {
        let mut rng = OsRng;
        let (store, gid, gid_bytes, admin_sk, member_sk) = seed(&mut rng);

        // Initial state matches `AutoFollowFlags::default()`. Post-#2422
        // that's {contexts: true, subgroups: false} — explicit assertion
        // on the exact shape so a future default flip can't slip through.
        let before = get_group_member_value(&store, &gid, &member_sk.public_key())
            .unwrap()
            .unwrap();
        assert!(before.auto_follow.contexts);
        assert!(!before.auto_follow.subgroups);

        // Member turns on contexts
        let op1 = SignedGroupOp::sign(
            &member_sk,
            gid_bytes,
            vec![],
            [0u8; 32],
            1,
            GroupOp::MemberSetAutoFollow {
                target: member_sk.public_key(),
                auto_follow_contexts: true,
                auto_follow_subgroups: false,
            },
        )
        .unwrap();
        apply_local_signed_group_op(&store, &op1).unwrap();

        // Admin changes role — flags must survive
        let op2 = SignedGroupOp::sign(
            &admin_sk,
            gid_bytes,
            vec![],
            [0u8; 32],
            1,
            GroupOp::MemberRoleSet {
                member: member_sk.public_key(),
                role: GroupMemberRole::ReadOnly,
            },
        )
        .unwrap();
        apply_local_signed_group_op(&store, &op2).unwrap();

        let after = get_group_member_value(&store, &gid, &member_sk.public_key())
            .unwrap()
            .unwrap();
        assert_eq!(after.role, GroupMemberRole::ReadOnly);
        assert!(after.auto_follow.contexts);
    }

    /// End-to-end path without the actor:
    ///   add_group_member → MemberSetAutoFollow → ContextRegistered.
    ///
    /// Asserts every stage lands in the store correctly and that the
    /// op-apply event channel fires the events the Phase 3 handler
    /// depends on. Exercises the full Phase 1–4 wiring short of the
    /// actor-driven `join_context` call, which needs a full merod
    /// instance (covered by the deferred merobox e2e workflow).
    #[tokio::test(flavor = "current_thread")]
    async fn end_to_end_event_fires_after_op_apply() {
        use calimero_primitives::application::ApplicationId;
        use calimero_primitives::blobs::BlobId;
        use calimero_primitives::context::ContextId;

        use crate::op_events::{self, OpEvent};

        let mut rng = OsRng;
        let (store, gid, gid_bytes, admin_sk, member_sk) = seed(&mut rng);

        // Subscribe BEFORE applying ops so we don't miss events.
        let mut rx = op_events::subscribe();

        // 1. MemberSetAutoFollow on self
        let set_flags = SignedGroupOp::sign(
            &member_sk,
            gid_bytes,
            vec![],
            [0u8; 32],
            1,
            GroupOp::MemberSetAutoFollow {
                target: member_sk.public_key(),
                auto_follow_contexts: true,
                auto_follow_subgroups: true,
            },
        )
        .unwrap();
        apply_local_signed_group_op(&store, &set_flags).unwrap();

        // Verify state landed
        let value = get_group_member_value(&store, &gid, &member_sk.public_key())
            .unwrap()
            .unwrap();
        assert!(value.auto_follow.contexts);
        assert!(value.auto_follow.subgroups);

        // 2. ContextRegistered op (admin registers a new context).
        let context_id = ContextId::from([0x77; 32]);
        let register = SignedGroupOp::sign(
            &admin_sk,
            gid_bytes,
            vec![],
            [0u8; 32],
            1,
            GroupOp::ContextRegistered {
                context_id,
                application_id: ApplicationId::from([0xAA; 32]),
                blob_id: BlobId::from([0xBB; 32]),
                source: "test://app".to_owned(),
                service_name: None,
            },
        )
        .unwrap();
        apply_local_signed_group_op(&store, &register).unwrap();

        // 3. The handler sees two events: AutoFollowSet + ContextRegistered.
        //    Drain events and assert both fired with the right payloads.
        //    The channel is process-wide so other tests may interleave —
        //    filter by our tag.
        // Match on (group_id, member_pk) for AutoFollowSet and on
        // (group_id, context_id) for ContextRegistered — other tests
        // running in parallel share the same global event channel and
        // `test_group_id()`, so group_id alone is not a unique filter.
        let expected_member = member_sk.public_key();
        let mut saw_auto_follow = false;
        let mut saw_context_registered = false;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while std::time::Instant::now() < deadline && !(saw_auto_follow && saw_context_registered) {
            match tokio::time::timeout(std::time::Duration::from_millis(200), rx.recv()).await {
                Ok(Ok(OpEvent::AutoFollowSet {
                    group_id,
                    member,
                    contexts,
                    subgroups,
                })) if group_id == gid_bytes && member == expected_member => {
                    assert!(contexts);
                    assert!(subgroups);
                    saw_auto_follow = true;
                }
                Ok(Ok(OpEvent::ContextRegistered {
                    group_id,
                    context_id: got,
                })) if group_id == gid_bytes && got == context_id => {
                    saw_context_registered = true;
                }
                Ok(Ok(_)) => {} // other events from parallel tests
                Ok(Err(_)) => break,
                Err(_) => continue,
            }
        }

        assert!(saw_auto_follow, "AutoFollowSet event should have fired");
        assert!(
            saw_context_registered,
            "ContextRegistered event should have fired"
        );
    }

    /// #2422 Option 2: a `GroupOp::MemberAdded` apply path now ALSO
    /// emits a synthesized `OpEvent::AutoFollowSet { contexts: true }`
    /// when the freshly-written member row carries the new default
    /// (`AutoFollowFlags::default() == {contexts: true, subgroups: false}`).
    /// Without this, the auto-follow handler would only react to
    /// FUTURE `OpEvent::ContextRegistered` events — pre-existing
    /// contexts in the group at join-time would be missed.
    #[tokio::test(flavor = "current_thread")]
    async fn member_added_emits_synthesized_auto_follow_set() {
        use crate::op_events::{self, OpEvent};

        let mut rng = OsRng;
        let (store, _gid, gid_bytes, admin_sk, _existing_member_sk) = seed(&mut rng);

        // Subscribe BEFORE applying ops so the broadcast channel
        // doesn't drop events we care about.
        let mut rx = op_events::subscribe();

        // A brand-new joiner — not in the seed() pair.
        let new_member_sk = PrivateKey::random(&mut rng);
        let new_member_pk = new_member_sk.public_key();

        let op = SignedGroupOp::sign(
            &admin_sk,
            gid_bytes,
            vec![],
            [0u8; 32],
            1,
            GroupOp::MemberAdded {
                member: new_member_pk,
                role: GroupMemberRole::Member,
            },
        )
        .unwrap();
        apply_local_signed_group_op(&store, &op).unwrap();

        // Verify the storage-side fix landed first: the new member's
        // row has `auto_follow.contexts == true` via the new Default.
        let value = get_group_member_value(
            &store,
            &calimero_context_config::types::ContextGroupId::from(gid_bytes),
            &new_member_pk,
        )
        .unwrap()
        .unwrap();
        assert!(
            value.auto_follow.contexts,
            "new member should default to contexts=true post-#2422"
        );
        assert!(
            !value.auto_follow.subgroups,
            "subgroups stays false (TEE-only path until non-TEE admission op exists)"
        );

        // Now drain events and confirm both `MemberAdded` and the
        // synthesized `AutoFollowSet` fired for this exact member.
        // Other tests in the same process share the global event
        // channel, so filter on `member == new_member_pk`. The
        // deadline is generous (10s) so the test stays reliable
        // under heavy parallel-test load on CI — the events are
        // emitted synchronously from `apply_local_signed_group_op`
        // before we even start polling, so on an unloaded run the
        // first `recv()` returns immediately.
        let mut saw_member_added = false;
        let mut saw_auto_follow = false;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while std::time::Instant::now() < deadline && !(saw_member_added && saw_auto_follow) {
            match tokio::time::timeout(std::time::Duration::from_millis(200), rx.recv()).await {
                Ok(Ok(OpEvent::MemberAdded {
                    group_id, member, ..
                })) if group_id == gid_bytes && member == new_member_pk => {
                    saw_member_added = true;
                }
                Ok(Ok(OpEvent::AutoFollowSet {
                    group_id,
                    member,
                    contexts,
                    subgroups,
                })) if group_id == gid_bytes && member == new_member_pk => {
                    assert!(
                        contexts,
                        "synthesized AutoFollowSet must carry contexts=true (Option 2)"
                    );
                    assert!(
                        !subgroups,
                        "synthesized AutoFollowSet mirrors stored subgroups (false by default)"
                    );
                    saw_auto_follow = true;
                }
                Ok(Ok(_)) => {} // unrelated events from parallel tests
                Ok(Err(_)) => break,
                Err(_) => continue,
            }
        }

        assert!(
            saw_member_added,
            "MemberAdded event should have fired for the new joiner"
        );
        assert!(
            saw_auto_follow,
            "synthesized AutoFollowSet should have fired for the new joiner (#2422 Option 2)"
        );
    }

    /// Verifies the opt-out path is preserved: if a member is added
    /// and then their contexts flag is explicitly turned off via
    /// `MemberSetAutoFollow`, the stored row reflects false. The
    /// synthesized `AutoFollowSet` from `MemberAdded` carries the
    /// default true, but a subsequent explicit SetMemberAutoFollow
    /// must be honored.
    #[test]
    fn explicit_opt_out_after_member_added_is_preserved() {
        let mut rng = OsRng;
        let (store, gid, gid_bytes, admin_sk, _) = seed(&mut rng);

        let target_sk = PrivateKey::random(&mut rng);
        let target_pk = target_sk.public_key();

        // Add member — picks up the new default {true, false}
        apply_local_signed_group_op(
            &store,
            &SignedGroupOp::sign(
                &admin_sk,
                gid_bytes,
                vec![],
                [0u8; 32],
                1,
                GroupOp::MemberAdded {
                    member: target_pk,
                    role: GroupMemberRole::Member,
                },
            )
            .unwrap(),
        )
        .unwrap();

        // Explicit opt-out (member acts on self)
        apply_local_signed_group_op(
            &store,
            &SignedGroupOp::sign(
                &target_sk,
                gid_bytes,
                vec![],
                [0u8; 32],
                1,
                GroupOp::MemberSetAutoFollow {
                    target: target_pk,
                    auto_follow_contexts: false,
                    auto_follow_subgroups: false,
                },
            )
            .unwrap(),
        )
        .unwrap();

        let value = get_group_member_value(&store, &gid, &target_pk)
            .unwrap()
            .unwrap();
        assert!(
            !value.auto_follow.contexts,
            "explicit opt-out via SetMemberAutoFollow must stick"
        );
    }
}

// -----------------------------------------------------------------------
// namespace_member_pubkeys — regression for ack-verify identity set
// -----------------------------------------------------------------------

/// Regression: the namespace creator/root admin is recorded only in
/// `GroupMeta::admin_identity` at namespace genesis (no self-`MemberJoined`
/// op). `namespace_member_pubkeys` must include that identity so that
/// `verify_ack` accepts legitimate acks signed by the namespace creator.
#[test]
fn namespace_member_pubkeys_includes_meta_admin_without_member_row() {
    let store = test_store();
    let namespace_id = [0xAA; 32];
    let gid = ContextGroupId::from(namespace_id);
    let admin = PublicKey::from([0x01; 32]);

    let meta = GroupMetaValue {
        app_key: [0xBB; 32],
        target_application_id: ApplicationId::from([0xCC; 32]),
        upgrade_policy: UpgradePolicy::Automatic,
        created_at: 1_700_000_000,
        admin_identity: admin,
        owner_identity: admin,
        migration: None,
        auto_join: true,
    };
    save_group_meta(&store, &gid, &meta).unwrap();

    let pks = namespace_member_pubkeys(&store, namespace_id).unwrap();
    assert!(
        pks.contains(&admin),
        "meta admin must appear in namespace_member_pubkeys even without a self-row"
    );
}

#[test]
fn namespace_member_pubkeys_dedups_admin_with_member_row() {
    let store = test_store();
    let namespace_id = [0xAA; 32];
    let gid = ContextGroupId::from(namespace_id);
    let admin = PublicKey::from([0x01; 32]);
    let other = PublicKey::from([0x02; 32]);

    let meta = GroupMetaValue {
        app_key: [0xBB; 32],
        target_application_id: ApplicationId::from([0xCC; 32]),
        upgrade_policy: UpgradePolicy::Automatic,
        created_at: 1_700_000_000,
        admin_identity: admin,
        owner_identity: admin,
        migration: None,
        auto_join: true,
    };
    save_group_meta(&store, &gid, &meta).unwrap();
    add_group_member(&store, &gid, &admin, GroupMemberRole::Admin).unwrap();
    add_group_member(&store, &gid, &other, GroupMemberRole::Member).unwrap();

    let pks = namespace_member_pubkeys(&store, namespace_id).unwrap();
    assert_eq!(pks.iter().filter(|p| **p == admin).count(), 1);
    assert!(pks.contains(&other));
}

#[test]
fn namespace_member_pubkeys_includes_member_rows() {
    let store = test_store();
    let namespace_id = [0xAA; 32];
    let gid = ContextGroupId::from(namespace_id);
    let m1 = PublicKey::from([0x10; 32]);
    let m2 = PublicKey::from([0x20; 32]);

    add_group_member(&store, &gid, &m1, GroupMemberRole::Member).unwrap();
    add_group_member(&store, &gid, &m2, GroupMemberRole::Admin).unwrap();

    let pks = namespace_member_pubkeys(&store, namespace_id).unwrap();
    assert!(pks.contains(&m1));
    assert!(pks.contains(&m2));
}

#[test]
fn membership_status_at_branch1_member_when_heads_match_and_member_exists() {
    use calimero_context_config::types::GovernancePosition;
    let store = test_store();
    let gid = test_group_id();
    let signer = PublicKey::from([0x42; 32]);

    save_group_meta(&store, &gid, &test_meta()).unwrap();
    add_group_member(&store, &gid, &signer, GroupMemberRole::Admin).unwrap();

    // Top-level group → namespace_id == group_id, local heads == [].
    let state_hash = compute_group_state_hash(&store, &gid).unwrap();
    let position = GovernancePosition::new(gid, state_hash, vec![]).unwrap();

    let status = membership_status_at(&store, &signer, &position).unwrap();
    assert!(matches!(
        status,
        MembershipStatus::Member(GroupMemberRole::Admin)
    ));
}

#[test]
fn membership_status_at_branch1_never_member_when_signer_absent() {
    use calimero_context_config::types::GovernancePosition;
    let store = test_store();
    let gid = test_group_id();
    let admin = PublicKey::from([0x01; 32]);
    let stranger = PublicKey::from([0xFE; 32]);

    save_group_meta(&store, &gid, &test_meta()).unwrap();
    add_group_member(&store, &gid, &admin, GroupMemberRole::Admin).unwrap();

    let state_hash = compute_group_state_hash(&store, &gid).unwrap();
    let position = GovernancePosition::new(gid, state_hash, vec![]).unwrap();

    let status = membership_status_at(&store, &stranger, &position).unwrap();
    assert!(matches!(status, MembershipStatus::NeverMember));
}

#[test]
fn membership_status_at_branch1_state_hash_mismatch_bails() {
    use calimero_context_config::types::GovernancePosition;
    let store = test_store();
    let gid = test_group_id();
    let signer = PublicKey::from([0x42; 32]);

    save_group_meta(&store, &gid, &test_meta()).unwrap();
    add_group_member(&store, &gid, &signer, GroupMemberRole::Member).unwrap();

    // heads match (both empty), but state_hash is wrong — must reject.
    let position = GovernancePosition::new(gid, [0xFF; 32], vec![]).unwrap();

    let err = membership_status_at(&store, &signer, &position).unwrap_err();
    let msg = format!("{err}");
    assert!(
        msg.contains("group_state_hash mismatch"),
        "expected hash-mismatch error, got: {msg}"
    );
}

#[test]
fn membership_status_at_branch2_unknown_when_heads_not_in_op_log() {
    use calimero_context_config::types::GovernancePosition;
    let store = test_store();
    let gid = test_group_id();
    let signer = PublicKey::from([0x42; 32]);

    save_group_meta(&store, &gid, &test_meta()).unwrap();
    add_group_member(&store, &gid, &signer, GroupMemberRole::Member).unwrap();

    // Position references a head that's not in the local op log.
    let unknown_head = [0xCC; 32];
    let state_hash = compute_group_state_hash(&store, &gid).unwrap();
    let position = GovernancePosition::new(gid, state_hash, vec![unknown_head]).unwrap();

    let status = membership_status_at(&store, &signer, &position).unwrap();
    match status {
        MembershipStatus::Unknown { needed } => {
            assert_eq!(needed, vec![unknown_head]);
        }
        other => panic!("expected Unknown, got {other:?}"),
    }
}

#[test]
fn membership_status_at_branch2_unknown_collects_all_missing_heads() {
    use calimero_context_config::types::GovernancePosition;
    let store = test_store();
    let gid = test_group_id();
    let signer = PublicKey::from([0x42; 32]);

    save_group_meta(&store, &gid, &test_meta()).unwrap();
    add_group_member(&store, &gid, &signer, GroupMemberRole::Member).unwrap();

    // Multiple unknown heads — all should be reported (round-2 fix).
    let h1 = [0xC1; 32];
    let h2 = [0xC2; 32];
    let h3 = [0xC3; 32];
    let state_hash = compute_group_state_hash(&store, &gid).unwrap();
    let position = GovernancePosition::new(gid, state_hash, vec![h1, h2, h3]).unwrap();

    let status = membership_status_at(&store, &signer, &position).unwrap();
    match status {
        MembershipStatus::Unknown { needed } => {
            assert_eq!(needed.len(), 3);
            assert!(needed.contains(&h1));
            assert!(needed.contains(&h2));
            assert!(needed.contains(&h3));
        }
        other => panic!("expected Unknown with 3 heads, got {other:?}"),
    }
}

#[test]
fn membership_status_at_rejects_position_with_mismatched_group() {
    use calimero_context_config::types::GovernancePosition;
    let store = test_store();
    let gid_a = ContextGroupId::from([0xAA; 32]);
    let gid_b = ContextGroupId::from([0xBB; 32]);
    let signer = PublicKey::from([0x42; 32]);

    save_group_meta(&store, &gid_a, &test_meta()).unwrap();
    add_group_member(&store, &gid_a, &signer, GroupMemberRole::Member).unwrap();

    // Position references group B, but B isn't set up. resolve_namespace
    // will fail or return its own ID, and the lookup proceeds against the
    // wrong group's empty member set → NeverMember.
    let state_hash_b = compute_group_state_hash(&store, &gid_a).unwrap();
    let position = GovernancePosition::new(gid_b, state_hash_b, vec![]).unwrap();

    // The behaviour here depends on whether the group_b lookup errors or
    // returns empty. Either is a correct rejection — assert it's NOT
    // returning Member (which would be the security failure).
    let result = membership_status_at(&store, &signer, &position);
    if let Ok(MembershipStatus::Member(_)) = result {
        panic!("must not return Member for a position pointing at a different group");
    }
}

#[test]
fn membership_status_at_oversized_governance_dag_heads_runtime_guard() {
    use calimero_context_config::types::{GovernancePosition, MAX_GOVERNANCE_DAG_HEADS};
    let store = test_store();
    let gid = test_group_id();
    let signer = PublicKey::from([0x42; 32]);

    save_group_meta(&store, &gid, &test_meta()).unwrap();

    // Bypass the constructor by direct field-init (mimics what would
    // happen if a deserialized position somehow exceeded the bound —
    // the runtime check is defense-in-depth).
    let oversized_heads: Vec<[u8; 32]> = (0..MAX_GOVERNANCE_DAG_HEADS + 1)
        .map(|i| {
            let mut h = [0u8; 32];
            h[0] = i as u8;
            h
        })
        .collect();
    let position = GovernancePosition {
        group_id: gid,
        group_state_hash: [0u8; 32],
        governance_dag_heads: oversized_heads,
    };

    let err = membership_status_at(&store, &signer, &position).unwrap_err();
    assert!(
        format!("{err}").contains("MAX_GOVERNANCE_DAG_HEADS"),
        "expected oversize error, got: {err}"
    );
}

#[test]
fn fast_path_current_member_resolves_to_member() {
    // Baseline: a member who exists in the materialized set resolves
    // to `Member(role)` when the position's heads equal local heads.
    // Required precondition for the other Branch 1 tests — if this
    // baseline fails, the others say nothing.
    use calimero_context_config::types::GovernancePosition;
    let store = test_store();
    let gid = test_group_id();
    let signer = PublicKey::from([0x77; 32]);

    save_group_meta(&store, &gid, &test_meta()).unwrap();
    add_group_member(&store, &gid, &signer, GroupMemberRole::Member).unwrap();

    let state_hash = compute_group_state_hash(&store, &gid).unwrap();
    let position = GovernancePosition::new(gid, state_hash, vec![]).unwrap();

    let status = membership_status_at(&store, &signer, &position).unwrap();
    assert!(
        matches!(status, MembershipStatus::Member(GroupMemberRole::Member)),
        "current member must resolve to Member, got {status:?}"
    );
}

#[test]
fn fast_path_removed_member_conflates_to_nevermember() {
    // Documented Branch 1 conflation: with heads equal, the resolver
    // only consults the materialized member set, which has no row for
    // a removed signer. It returns `NeverMember` — it cannot
    // distinguish "removed" from "was never a member" without the DAG.
    // The distinction is recovered by Branch 3 (prefix walk) when the
    // sender's position predates the removal. The apply-time check
    // treats both `Removed` and `NeverMember` as rejection, so the
    // practical security outcome is identical on this path.
    use calimero_context_config::types::GovernancePosition;
    let store = test_store();
    let gid = test_group_id();
    let signer = PublicKey::from([0x78; 32]);

    save_group_meta(&store, &gid, &test_meta()).unwrap();
    add_group_member(&store, &gid, &signer, GroupMemberRole::Member).unwrap();
    remove_group_member(&store, &gid, &signer).unwrap();

    let state_hash = compute_group_state_hash(&store, &gid).unwrap();
    let position = GovernancePosition::new(gid, state_hash, vec![]).unwrap();

    let status = membership_status_at(&store, &signer, &position).unwrap();
    assert!(
        matches!(status, MembershipStatus::NeverMember),
        "removed signer on heads-equal fast path is NeverMember, got {status:?}"
    );
}

#[test]
fn fast_path_re_added_member_resolves_to_member() {
    // Add → Remove → Add: the materialized set contains the signer
    // again, so the fast path returns Member with the latest role.
    // The resolver doesn't remember that they were ever removed —
    // the deny-list elsewhere handles "currently removed" semantics.
    use calimero_context_config::types::GovernancePosition;
    let store = test_store();
    let gid = test_group_id();
    let signer = PublicKey::from([0x79; 32]);

    save_group_meta(&store, &gid, &test_meta()).unwrap();
    add_group_member(&store, &gid, &signer, GroupMemberRole::Member).unwrap();
    remove_group_member(&store, &gid, &signer).unwrap();
    add_group_member(&store, &gid, &signer, GroupMemberRole::Admin).unwrap();

    let state_hash = compute_group_state_hash(&store, &gid).unwrap();
    let position = GovernancePosition::new(gid, state_hash, vec![]).unwrap();

    let status = membership_status_at(&store, &signer, &position).unwrap();
    assert!(
        matches!(status, MembershipStatus::Member(GroupMemberRole::Admin)),
        "re-added signer resolves with new role on fast path, got {status:?}"
    );
}

#[test]
fn fast_path_role_promotion_picks_current_role() {
    // Role changes between Add and present don't cause spurious
    // rejection on the fast path — the materialized set reflects the
    // latest role.
    use calimero_context_config::types::GovernancePosition;
    let store = test_store();
    let gid = test_group_id();
    let signer = PublicKey::from([0x7A; 32]);

    save_group_meta(&store, &gid, &test_meta()).unwrap();
    add_group_member(&store, &gid, &signer, GroupMemberRole::Member).unwrap();
    // Re-add with Admin role (simulates a role change in the
    // materialized layer — at the namespace governance layer this
    // would be a `MemberRoleSet`).
    add_group_member(&store, &gid, &signer, GroupMemberRole::Admin).unwrap();

    let state_hash = compute_group_state_hash(&store, &gid).unwrap();
    let position = GovernancePosition::new(gid, state_hash, vec![]).unwrap();

    let status = membership_status_at(&store, &signer, &position).unwrap();
    assert!(
        matches!(status, MembershipStatus::Member(GroupMemberRole::Admin)),
        "current role wins on fast path, got {status:?}"
    );
}

#[test]
fn remove_group_member_clears_member_metadata() {
    use calimero_primitives::metadata::MetadataRecord;

    let store = test_store();
    let gid = test_group_id();
    let member = PublicKey::from([0x42; 32]);
    save_group_meta(&store, &gid, &test_meta()).unwrap();
    add_group_member(&store, &gid, &member, GroupMemberRole::Member).unwrap();
    set_member_metadata(
        &store,
        &gid,
        &member,
        &MetadataRecord {
            name: Some("departing".to_owned()),
            ..Default::default()
        },
    )
    .unwrap();
    assert!(get_member_metadata(&store, &gid, &member)
        .unwrap()
        .is_some());

    remove_group_member(&store, &gid, &member).unwrap();
    assert!(get_member_metadata(&store, &gid, &member)
        .unwrap()
        .is_none());
}

#[test]
fn trusted_anchors_empty_when_meta_absent() {
    // No `save_group_meta` call — no group exists. Should return an
    // empty set, not error. Caller falls back to random selection.
    let store = test_store();
    let gid = test_group_id();
    let anchors = trusted_anchors_for_group(&store, &gid).unwrap();
    assert!(anchors.is_empty(), "expected empty set, got {anchors:?}");
}

#[test]
fn trusted_anchors_includes_owner_and_legacy_admin() {
    // Fresh group: owner_identity == admin_identity (the creator).
    // Anchor set contains just that one pubkey.
    let store = test_store();
    let gid = test_group_id();
    let creator = PublicKey::from([0x01; 32]);
    save_group_meta(&store, &gid, &sample_meta_with_admin(creator)).unwrap();

    let anchors = trusted_anchors_for_group(&store, &gid).unwrap();
    assert!(anchors.contains(&creator), "creator must be an anchor");
    assert_eq!(
        anchors.len(),
        1,
        "fresh group: owner == admin, exactly one anchor expected, got {anchors:?}"
    );
}

#[test]
fn trusted_anchors_includes_owner_distinct_from_legacy_admin() {
    // Post-`TransferOwnership` shape: owner_identity != admin_identity.
    // Both must be in the anchor set — admin_identity is the legacy
    // fallback creator marker that `is_group_admin` still honors.
    use calimero_store::key::GroupMetaValue;
    let store = test_store();
    let gid = test_group_id();
    let creator = PublicKey::from([0x01; 32]);
    let new_owner = PublicKey::from([0x02; 32]);
    let meta = GroupMetaValue {
        app_key: [0xBB; 32],
        target_application_id: calimero_primitives::application::ApplicationId::from([0xCC; 32]),
        upgrade_policy: calimero_primitives::context::UpgradePolicy::Automatic,
        created_at: 1_700_000_000,
        admin_identity: creator,
        owner_identity: new_owner,
        migration: None,
        auto_join: true,
    };
    save_group_meta(&store, &gid, &meta).unwrap();

    let anchors = trusted_anchors_for_group(&store, &gid).unwrap();
    assert!(anchors.contains(&creator), "legacy admin must be an anchor");
    assert!(
        anchors.contains(&new_owner),
        "owner (post-transfer) must be an anchor"
    );
    assert_eq!(anchors.len(), 2);
}

#[test]
fn trusted_anchors_includes_admin_members() {
    let store = test_store();
    let gid = test_group_id();
    let creator = PublicKey::from([0x01; 32]);
    let admin_a = PublicKey::from([0xA1; 32]);
    let admin_b = PublicKey::from([0xA2; 32]);

    save_group_meta(&store, &gid, &sample_meta_with_admin(creator)).unwrap();
    add_group_member(&store, &gid, &admin_a, GroupMemberRole::Admin).unwrap();
    add_group_member(&store, &gid, &admin_b, GroupMemberRole::Admin).unwrap();

    let anchors = trusted_anchors_for_group(&store, &gid).unwrap();
    assert!(anchors.contains(&creator));
    assert!(anchors.contains(&admin_a));
    assert!(anchors.contains(&admin_b));
    assert_eq!(anchors.len(), 3);
}

#[test]
fn trusted_anchors_includes_read_only_tee_members() {
    // ReadOnlyTee members are in the anchor set because the role is
    // gated at apply-time by `MemberJoinedViaTeeAttestation` (see the
    // `apply_group_op_mutations` carve-out): a peer cannot
    // self-declare `ReadOnlyTee` without admission. The role in the
    // store IS the admission proof.
    let store = test_store();
    let gid = test_group_id();
    let creator = PublicKey::from([0x01; 32]);
    let tee_attested = PublicKey::from([0xC1; 32]);

    save_group_meta(&store, &gid, &sample_meta_with_admin(creator)).unwrap();
    add_group_member(&store, &gid, &tee_attested, GroupMemberRole::ReadOnlyTee).unwrap();

    let anchors = trusted_anchors_for_group(&store, &gid).unwrap();
    assert!(anchors.contains(&tee_attested));
    assert_eq!(anchors.len(), 2);
}

#[test]
fn trusted_anchors_excludes_plain_members_and_read_only() {
    // Plain `Member` and `ReadOnly` peers can still serve sync if
    // asked, but clients should NOT preferentially target them.
    let store = test_store();
    let gid = test_group_id();
    let creator = PublicKey::from([0x01; 32]);
    let member = PublicKey::from([0xB1; 32]);
    let read_only = PublicKey::from([0xB2; 32]);

    save_group_meta(&store, &gid, &sample_meta_with_admin(creator)).unwrap();
    add_group_member(&store, &gid, &member, GroupMemberRole::Member).unwrap();
    add_group_member(&store, &gid, &read_only, GroupMemberRole::ReadOnly).unwrap();

    let anchors = trusted_anchors_for_group(&store, &gid).unwrap();
    assert!(
        !anchors.contains(&member),
        "plain Member must not be an anchor"
    );
    assert!(
        !anchors.contains(&read_only),
        "plain ReadOnly must not be an anchor"
    );
    assert_eq!(anchors.len(), 1, "only the creator should be in the set");
}

#[test]
fn trusted_anchors_mixed_roles() {
    // Full mix: owner, separate legacy admin, two admin members, one
    // ReadOnlyTee, one plain Member, one ReadOnly. Anchor set should
    // be {owner, legacy_admin, admin_a, admin_b, tee} — size 5.
    use calimero_store::key::GroupMetaValue;
    let store = test_store();
    let gid = test_group_id();
    let owner = PublicKey::from([0x01; 32]);
    let legacy_admin = PublicKey::from([0x02; 32]);
    let admin_a = PublicKey::from([0xA1; 32]);
    let admin_b = PublicKey::from([0xA2; 32]);
    let tee = PublicKey::from([0xC1; 32]);
    let member = PublicKey::from([0xB1; 32]);
    let read_only = PublicKey::from([0xB2; 32]);

    let meta = GroupMetaValue {
        app_key: [0xBB; 32],
        target_application_id: calimero_primitives::application::ApplicationId::from([0xCC; 32]),
        upgrade_policy: calimero_primitives::context::UpgradePolicy::Automatic,
        created_at: 1_700_000_000,
        admin_identity: legacy_admin,
        owner_identity: owner,
        migration: None,
        auto_join: true,
    };
    save_group_meta(&store, &gid, &meta).unwrap();
    add_group_member(&store, &gid, &admin_a, GroupMemberRole::Admin).unwrap();
    add_group_member(&store, &gid, &admin_b, GroupMemberRole::Admin).unwrap();
    add_group_member(&store, &gid, &tee, GroupMemberRole::ReadOnlyTee).unwrap();
    add_group_member(&store, &gid, &member, GroupMemberRole::Member).unwrap();
    add_group_member(&store, &gid, &read_only, GroupMemberRole::ReadOnly).unwrap();

    let anchors = trusted_anchors_for_group(&store, &gid).unwrap();
    let expected: std::collections::BTreeSet<_> = [owner, legacy_admin, admin_a, admin_b, tee]
        .into_iter()
        .collect();
    assert_eq!(anchors, expected, "anchor set mismatch");
}

#[test]
fn get_effective_member_capabilities_includes_inherited_open_subgroup_joiner() {
    use calimero_context_config::{MemberCapabilities, VisibilityMode};

    // namespace (root) -> reports (Open subgroup). Bob is a namespace
    // member with the join cap; he joined `reports` via inheritance and
    // holds no direct row there.
    let store = test_store();
    let namespace = ContextGroupId::from([0x50; 32]);
    let reports = ContextGroupId::from([0x51; 32]);
    let bob = PublicKey::from([0x02; 32]);

    nest_for_test(&store, &namespace, &reports);
    add_group_member(&store, &namespace, &bob, GroupMemberRole::Member).unwrap();
    set_member_capability(
        &store,
        &namespace,
        &bob,
        MemberCapabilities::CAN_JOIN_OPEN_SUBGROUPS,
    )
    .unwrap();
    set_subgroup_visibility(&store, &reports, VisibilityMode::Open).unwrap();

    // Contract sanity: Bob *is* an effective member of `reports` by
    // inheritance, but he has no stored row there.
    assert!(check_group_membership(&store, &reports, &bob).unwrap());
    assert!(
        get_group_member_role(&store, &reports, &bob)
            .unwrap()
            .is_none(),
        "precondition: inherited joiner has no direct GroupMember row"
    );

    // The gate behind `get_member_capabilities` must surface Bob as a
    // member with an effective bitmask of 0 — before the #2378 fix it
    // returned `None` and the handler bailed "identity is not a member".
    assert_eq!(
        get_effective_member_capabilities(&store, &reports, &bob).unwrap(),
        Some(0),
        "inherited Open-subgroup joiner must resolve to Some(0), not None"
    );
}

#[test]
fn get_effective_member_capabilities_reports_inherited_admin() {
    use calimero_context_config::VisibilityMode;

    // A namespace admin who never explicitly joined the Open subgroup
    // still inherits admin authority into it. The gate must accept them
    // (Some, not None); inherited members hold no explicit per-member
    // bitmask in the subgroup, so the value is 0.
    let store = test_store();
    let namespace = ContextGroupId::from([0x52; 32]);
    let reports = ContextGroupId::from([0x53; 32]);
    let admin = PublicKey::from([0x01; 32]);

    nest_for_test(&store, &namespace, &reports);
    add_group_member(&store, &namespace, &admin, GroupMemberRole::Admin).unwrap();
    set_subgroup_visibility(&store, &reports, VisibilityMode::Open).unwrap();

    assert!(check_group_membership(&store, &reports, &admin).unwrap());
    assert_eq!(
        get_effective_member_capabilities(&store, &reports, &admin).unwrap(),
        Some(0),
        "inherited admin must resolve to Some(0), not None"
    );
}

#[test]
fn get_effective_member_capabilities_returns_stored_bits_for_direct_member() {
    use calimero_context_config::MemberCapabilities;

    // Regression guard: a direct member's stored per-member capability
    // row must still flow through unchanged.
    let store = test_store();
    let group = ContextGroupId::from([0x54; 32]);
    let carol = PublicKey::from([0x03; 32]);

    add_group_member(&store, &group, &carol, GroupMemberRole::Member).unwrap();
    set_member_capability(
        &store,
        &group,
        &carol,
        MemberCapabilities::CAN_JOIN_OPEN_SUBGROUPS,
    )
    .unwrap();

    assert_eq!(
        get_effective_member_capabilities(&store, &group, &carol).unwrap(),
        Some(MemberCapabilities::CAN_JOIN_OPEN_SUBGROUPS),
        "direct member's stored capability bitmask must be returned verbatim"
    );
}

#[test]
fn get_effective_member_capabilities_some_zero_for_direct_member_without_cap_row() {
    // Regression guard for the `unwrap_or(0)` branch via the *direct*
    // path: a member added with `add_group_member` and no explicit
    // `set_member_capability` grant holds no per-member capability row.
    // They are still a member, so the gate must resolve them to
    // `Some(0)` ("member, no delegated bits") — not `None`. The
    // inherited-member tests above also reach `unwrap_or(0)`, but only
    // through the inherited-path resolution; this pins the direct-row
    // case, the common shape for a plain member.
    let store = test_store();
    let group = ContextGroupId::from([0x58; 32]);
    let dave = PublicKey::from([0x05; 32]);

    add_group_member(&store, &group, &dave, GroupMemberRole::Member).unwrap();
    // No `set_member_capability` call — no capability row is stored.

    assert_eq!(
        get_effective_member_capabilities(&store, &group, &dave).unwrap(),
        Some(0),
        "direct member with no capability row must resolve to Some(0)"
    );
}

#[test]
fn get_effective_member_capabilities_none_for_non_member() {
    // Regression guard: a true non-member still resolves to `None` so the
    // handler keeps rejecting them.
    let store = test_store();
    let group = ContextGroupId::from([0x55; 32]);
    let stranger = PublicKey::from([0x04; 32]);

    assert_eq!(
        get_effective_member_capabilities(&store, &group, &stranger).unwrap(),
        None,
        "non-member must resolve to None"
    );
}

#[test]
fn get_effective_member_capabilities_none_for_restricted_wall() {
    use calimero_context_config::{MemberCapabilities, VisibilityMode};

    // A `Restricted` subgroup is a wall: a parent member is NOT an
    // effective member of it, so the gate must return `None`. Guards the
    // fix against over-reaching past the inheritance boundary.
    let store = test_store();
    let namespace = ContextGroupId::from([0x56; 32]);
    let reports = ContextGroupId::from([0x57; 32]);
    let bob = PublicKey::from([0x02; 32]);

    nest_for_test(&store, &namespace, &reports);
    add_group_member(&store, &namespace, &bob, GroupMemberRole::Member).unwrap();
    set_member_capability(
        &store,
        &namespace,
        &bob,
        MemberCapabilities::CAN_JOIN_OPEN_SUBGROUPS,
    )
    .unwrap();
    set_subgroup_visibility(&store, &reports, VisibilityMode::Restricted).unwrap();

    assert!(!check_group_membership(&store, &reports, &bob).unwrap());
    assert_eq!(
        get_effective_member_capabilities(&store, &reports, &bob).unwrap(),
        None,
        "Restricted subgroup is a wall — parent member must resolve to None"
    );
}

#[test]
fn get_effective_member_capabilities_none_for_denied_inherited_member() {
    use calimero_context_config::{MemberCapabilities, VisibilityMode};

    // A member kicked from an Open subgroup keeps their namespace-level
    // inheritance but is deny-listed on the subgroup — the deny-list IS
    // the removal (#2371), since the kick has no direct row to delete.
    // `enumerate_inherited_members` filters them, so `list_group_members`
    // omits them; the gate behind `get_member_capabilities` must agree
    // and resolve them to `None`, not `Some(0)`. Sibling of
    // `enumerate_inherited_members_excludes_deny_listed_member`.
    let store = test_store();
    let namespace = ContextGroupId::from([0x59; 32]);
    let reports = ContextGroupId::from([0x5A; 32]);
    let bob = PublicKey::from([0x02; 32]);

    nest_for_test(&store, &namespace, &reports);
    add_group_member(&store, &namespace, &bob, GroupMemberRole::Member).unwrap();
    set_member_capability(
        &store,
        &namespace,
        &bob,
        MemberCapabilities::CAN_JOIN_OPEN_SUBGROUPS,
    )
    .unwrap();
    set_subgroup_visibility(&store, &reports, VisibilityMode::Open).unwrap();

    // Pre-kick: Bob inherits membership and the gate accepts him.
    assert_eq!(
        get_effective_member_capabilities(&store, &reports, &bob).unwrap(),
        Some(0),
        "precondition: inherited joiner resolves to Some(0) before the kick"
    );

    // Kick: deny-list Bob on `reports` (what `MemberRemoved` / `MemberLeft`
    // apply does for a subgroup-level removal).
    mark_denied(&store, &reports, &bob).unwrap();

    assert_eq!(
        get_effective_member_capabilities(&store, &reports, &bob).unwrap(),
        None,
        "deny-listed (kicked) inherited member must resolve to None — \
         consistent with their absence from list_group_members"
    );
}

#[test]
fn subgroup_visible_to_open_child_is_public_to_everyone() {
    use calimero_context_config::VisibilityMode;

    let store = test_store();
    let parent = ContextGroupId::from([0xD0; 32]);
    let child = ContextGroupId::from([0xD1; 32]);
    let stranger = PublicKey::from([0x09; 32]);

    nest_for_test(&store, &parent, &child);
    set_subgroup_visibility(&store, &child, VisibilityMode::Open).unwrap();

    // An `Open` subgroup's existence is public by design — it is listed
    // even for a caller that cannot be identified and one that is a
    // total non-member.
    assert!(subgroup_visible_to(&store, &parent, &child, None).unwrap());
    assert!(subgroup_visible_to(&store, &parent, &child, Some(&stranger)).unwrap());
}

#[test]
fn subgroup_visible_to_restricted_child_hidden_from_non_member() {
    use calimero_context_config::VisibilityMode;

    let store = test_store();
    let parent = ContextGroupId::from([0xD2; 32]);
    let child = ContextGroupId::from([0xD3; 32]);
    let stranger = PublicKey::from([0x09; 32]);

    nest_for_test(&store, &parent, &child);
    set_subgroup_visibility(&store, &child, VisibilityMode::Restricted).unwrap();

    // The caller is neither a parent admin nor a member of the
    // restricted child — its existence must not leak through the list.
    assert!(!subgroup_visible_to(&store, &parent, &child, Some(&stranger)).unwrap());
}

#[test]
fn subgroup_visible_to_restricted_child_hidden_when_caller_unknown() {
    use calimero_context_config::VisibilityMode;

    let store = test_store();
    let parent = ContextGroupId::from([0xD4; 32]);
    let child = ContextGroupId::from([0xD5; 32]);

    nest_for_test(&store, &parent, &child);
    set_subgroup_visibility(&store, &child, VisibilityMode::Restricted).unwrap();

    // `caller == None` (node has no namespace identity for the parent):
    // membership cannot be verified, so the conservative choice hides
    // the restricted child.
    assert!(!subgroup_visible_to(&store, &parent, &child, None).unwrap());
}

#[test]
fn subgroup_visible_to_restricted_child_visible_to_parent_admin() {
    use calimero_context_config::VisibilityMode;

    let store = test_store();
    let parent = ContextGroupId::from([0xD6; 32]);
    let child = ContextGroupId::from([0xD7; 32]);
    let admin = PublicKey::from([0x0A; 32]);

    nest_for_test(&store, &parent, &child);
    set_subgroup_visibility(&store, &child, VisibilityMode::Restricted).unwrap();

    // An admin of the parent group governs the space and must be able
    // to enumerate every child — even restricted ones it is not itself
    // a member of.
    add_group_member(&store, &parent, &admin, GroupMemberRole::Admin).unwrap();
    assert!(subgroup_visible_to(&store, &parent, &child, Some(&admin)).unwrap());
}

#[test]
fn subgroup_visible_to_restricted_child_visible_to_its_member() {
    use calimero_context_config::VisibilityMode;

    let store = test_store();
    let parent = ContextGroupId::from([0xD8; 32]);
    let child = ContextGroupId::from([0xD9; 32]);
    let member = PublicKey::from([0x0B; 32]);

    nest_for_test(&store, &parent, &child);
    set_subgroup_visibility(&store, &child, VisibilityMode::Restricted).unwrap();

    // A direct member of the restricted child sees it, even though it
    // is not an admin of the parent group.
    add_group_member(&store, &child, &member, GroupMemberRole::Member).unwrap();
    assert!(subgroup_visible_to(&store, &parent, &child, Some(&member)).unwrap());
}

#[test]
fn is_authoritative_namespace_identity_recognizes_owner_admin_tee() {
    let store = test_store();
    let mut rng = rand::thread_rng();
    let namespace_id = [0xAA; 32];
    let gid = ContextGroupId::from(namespace_id);
    let owner = PublicKey::from([0x01; 32]);
    let admin_member = PublicKey::from([0x02; 32]);
    let tee_node = PublicKey::from([0x03; 32]);
    let ordinary = PublicKey::from([0x04; 32]);
    let stranger = PublicKey::from([0x05; 32]);

    let meta = GroupMetaValue {
        app_key: [0xBB; 32],
        target_application_id: ApplicationId::from([0xCC; 32]),
        upgrade_policy: UpgradePolicy::Automatic,
        created_at: 1_700_000_000,
        admin_identity: owner,
        owner_identity: owner,
        migration: None,
        auto_join: true,
    };
    save_group_meta(&store, &gid, &meta).unwrap();
    add_group_member(&store, &gid, &admin_member, GroupMemberRole::Admin).unwrap();
    add_group_member(&store, &gid, &ordinary, GroupMemberRole::Member).unwrap();

    let signer_sk = PrivateKey::random(&mut rng);
    let tee_op = SignedGroupOp::sign(
        &signer_sk,
        gid.to_bytes(),
        vec![],
        [0u8; 32],
        1,
        GroupOp::MemberJoinedViaTeeAttestation {
            member: tee_node,
            quote_hash: [0x09; 32],
            mrtd: "m".to_owned(),
            rtmr0: "0".to_owned(),
            rtmr1: "1".to_owned(),
            rtmr2: "2".to_owned(),
            rtmr3: "3".to_owned(),
            tcb_status: "ok".to_owned(),
            role: GroupMemberRole::Member,
        },
    )
    .unwrap();
    append_op_log_entry(&store, &gid, 1, &borsh::to_vec(&tee_op).unwrap()).unwrap();

    assert!(is_authoritative_namespace_identity(&store, namespace_id, &owner).unwrap());
    assert!(is_authoritative_namespace_identity(&store, namespace_id, &admin_member).unwrap());
    assert!(is_authoritative_namespace_identity(&store, namespace_id, &tee_node).unwrap());
    assert!(!is_authoritative_namespace_identity(&store, namespace_id, &ordinary).unwrap());
    assert!(!is_authoritative_namespace_identity(&store, namespace_id, &stranger).unwrap());
}

