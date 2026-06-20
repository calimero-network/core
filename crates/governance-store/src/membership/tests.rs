//! Tests for `group_store::membership::*`. Extracted from the monolithic
//! `group_store/tests.rs` as part of issue #2306 (epic #2300).
//!
//! Test order is preserved from `tests.rs` to keep `git blame` useful;
//! helpers that are only used by membership tests came along (e.g.
//! `nest_for_test`). Helpers shared with non-membership tests
//! (`test_store`, `test_group_id`, `test_meta`, `dummy_member_removed_op`)
//! are imported from the parent `group_store::test_fixtures` module.

use crate::{
    CapabilitiesRepository, DenyListRepository, MembershipRepository, MetaRepository,
    MetadataRepository,
};
use calimero_context_client::local_governance::GroupOp;
use calimero_context_config::types::ContextGroupId;
use calimero_primitives::application::ApplicationId;
use calimero_primitives::context::{GroupMemberRole, UpgradePolicy};
use calimero_primitives::identity::PublicKey;
use calimero_store::key::GroupMetaValue;

use super::super::test_fixtures::{
    nest_for_test, nest_for_test_unchecked, sample_meta_with_admin, test_group_id, test_meta,
    test_store,
};
use super::super::*;
use super::TeeAttestationClaims;

#[test]
fn add_and_check_membership() {
    let store = test_store();
    let gid = test_group_id();
    let pk = PublicKey::from([0x01; 32]);

    assert!(!MembershipRepository::new(&store)
        .is_member(&gid, &pk)
        .unwrap());

    MembershipRepository::new(&store)
        .add_member(&gid, &pk, GroupMemberRole::Admin)
        .unwrap();
    assert!(MembershipRepository::new(&store)
        .is_member(&gid, &pk)
        .unwrap());
    assert!(MembershipRepository::new(&store)
        .is_admin(&gid, &pk)
        .unwrap());
}

#[test]
fn remove_member() {
    let store = test_store();
    let gid = test_group_id();
    let pk = PublicKey::from([0x02; 32]);

    MembershipRepository::new(&store)
        .add_member(&gid, &pk, GroupMemberRole::Member)
        .unwrap();
    assert!(MembershipRepository::new(&store)
        .is_member(&gid, &pk)
        .unwrap());

    MembershipRepository::new(&store)
        .remove_member(&gid, &pk)
        .unwrap();
    assert!(!MembershipRepository::new(&store)
        .is_member(&gid, &pk)
        .unwrap());
}

#[test]
fn get_member_role() {
    let store = test_store();
    let gid = test_group_id();
    let admin = PublicKey::from([0x01; 32]);
    let member = PublicKey::from([0x02; 32]);

    MembershipRepository::new(&store)
        .add_member(&gid, &admin, GroupMemberRole::Admin)
        .unwrap();
    MembershipRepository::new(&store)
        .add_member(&gid, &member, GroupMemberRole::Member)
        .unwrap();

    assert_eq!(
        MembershipRepository::new(&store)
            .role_of(&gid, &admin)
            .unwrap(),
        Some(GroupMemberRole::Admin)
    );
    assert_eq!(
        MembershipRepository::new(&store)
            .role_of(&gid, &member)
            .unwrap(),
        Some(GroupMemberRole::Member)
    );
    assert!(!MembershipRepository::new(&store)
        .is_admin(&gid, &member)
        .unwrap());
}

#[test]
fn require_group_admin_rejects_non_admin() {
    let store = test_store();
    let gid = test_group_id();
    let member = PublicKey::from([0x03; 32]);

    MembershipRepository::new(&store)
        .add_member(&gid, &member, GroupMemberRole::Member)
        .unwrap();
    assert!(MembershipRepository::new(&store)
        .require_admin(&gid, &member)
        .is_err());
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

    MembershipRepository::new(&store)
        .add_member(&gid, &admin, GroupMemberRole::Admin)
        .unwrap();
    MembershipRepository::new(&store)
        .add_member(&gid, &member, GroupMemberRole::Member)
        .unwrap();

    let membership = MembershipPolicy::new(&store, gid);
    assert!(membership.ensure_not_last_admin_removal(&admin).is_err());
    assert!(membership
        .ensure_not_last_admin_demotion(&admin, &GroupMemberRole::Member)
        .is_err());
    assert!(membership
        .ensure_not_last_admin_demotion(&admin, &GroupMemberRole::Admin)
        .is_ok());

    MembershipRepository::new(&store)
        .add_member(&gid, &admin2, GroupMemberRole::Admin)
        .unwrap();
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
        .validate_tee_attestation_allowlists(
            &policy,
            &TeeAttestationClaims {
                mrtd: "m1",
                rtmr0: "r0",
                rtmr1: "x",
                rtmr2: "y",
                rtmr3: "z",
                tcb_status: "ok"
            },
        )
        .is_ok());
    assert!(membership
        .validate_tee_attestation_allowlists(
            &policy,
            &TeeAttestationClaims {
                mrtd: "wrong",
                rtmr0: "r0",
                rtmr1: "x",
                rtmr2: "y",
                rtmr3: "z",
                tcb_status: "ok"
            },
        )
        .is_err());
    assert!(membership
        .validate_tee_attestation_allowlists(
            &policy,
            &TeeAttestationClaims {
                mrtd: "m1",
                rtmr0: "wrong",
                rtmr1: "x",
                rtmr2: "y",
                rtmr3: "z",
                tcb_status: "ok"
            },
        )
        .is_err());

    let tee_joined = PrivateKey::random(&mut rng).public_key();
    assert!(!MembershipRepository::new(&store)
        .is_member(&gid, &tee_joined)
        .unwrap());
    membership
        .admit_member_if_absent(&tee_joined, &GroupMemberRole::Member)
        .unwrap();
    assert!(MembershipRepository::new(&store)
        .is_member(&gid, &tee_joined)
        .unwrap());
    membership
        .admit_member_if_absent(&tee_joined, &GroupMemberRole::Member)
        .unwrap();
    assert!(MembershipRepository::new(&store)
        .is_member(&gid, &tee_joined)
        .unwrap());
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

    MembershipRepository::new(&store)
        .add_member(&gid, &admin1, GroupMemberRole::Admin)
        .unwrap();
    MembershipRepository::new(&store)
        .add_member(&gid, &admin2, GroupMemberRole::Admin)
        .unwrap();
    MembershipRepository::new(&store)
        .add_member(&gid, &member, GroupMemberRole::Member)
        .unwrap();

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

    assert_eq!(MembershipRepository::new(&store).count(&gid).unwrap(), 0);
    assert_eq!(
        MembershipRepository::new(&store)
            .count_admins(&gid)
            .unwrap(),
        0
    );

    MembershipRepository::new(&store)
        .add_member(&gid, &PublicKey::from([0x01; 32]), GroupMemberRole::Admin)
        .unwrap();
    MembershipRepository::new(&store)
        .add_member(&gid, &PublicKey::from([0x02; 32]), GroupMemberRole::Member)
        .unwrap();
    MembershipRepository::new(&store)
        .add_member(&gid, &PublicKey::from([0x03; 32]), GroupMemberRole::Admin)
        .unwrap();

    assert_eq!(MembershipRepository::new(&store).count(&gid).unwrap(), 3);
    assert_eq!(
        MembershipRepository::new(&store)
            .count_admins(&gid)
            .unwrap(),
        2
    );
}

#[test]
fn list_members_with_offset_and_limit() {
    let store = test_store();
    let gid = test_group_id();

    for i in 0u8..5 {
        let mut pk_bytes = [0u8; 32];
        pk_bytes[0] = i;
        MembershipRepository::new(&store)
            .add_member(&gid, &PublicKey::from(pk_bytes), GroupMemberRole::Member)
            .unwrap();
    }

    let all = MembershipRepository::new(&store)
        .list(&gid, 0, 100)
        .unwrap();
    assert_eq!(all.len(), 5);

    let page = MembershipRepository::new(&store).list(&gid, 1, 2).unwrap();
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
    MembershipRepository::new(&store)
        .add_member(&parent, &alice, GroupMemberRole::Member)
        .unwrap();
    CapabilitiesRepository::new(&store)
        .set_member_capability(&parent, &alice, MemberCapabilities::CAN_JOIN_OPEN_SUBGROUPS)
        .unwrap();

    // Child is `Open`. Alice should be inherited as a member.
    CapabilitiesRepository::new(&store)
        .set_subgroup_visibility(&child, VisibilityMode::Open)
        .unwrap();
    assert!(MembershipRepository::new(&store)
        .is_member(&child, &alice)
        .unwrap());
}

#[test]
fn check_membership_path_inherited_when_member_added_after_default_caps() {
    use calimero_context_config::{MemberCapabilities, VisibilityMode};

    let store = test_store();
    let ns = ContextGroupId::from([0xB4; 32]);
    let child = ContextGroupId::from([0xB5; 32]);
    let bob = PublicKey::from([0x02; 32]);

    nest_for_test(&store, &ns, &child);
    CapabilitiesRepository::new(&store)
        .set_subgroup_visibility(&child, VisibilityMode::Open)
        .unwrap();

    // Correct ordering: default caps set FIRST, then the member is
    // added — `add_group_member` copies the default into bob's
    // per-member capability row.
    CapabilitiesRepository::new(&store)
        .set_default_capabilities(&ns, MemberCapabilities::CAN_JOIN_OPEN_SUBGROUPS)
        .unwrap();
    MembershipRepository::new(&store)
        .add_member(&ns, &bob, GroupMemberRole::Member)
        .unwrap();

    let path = MembershipRepository::new(&store)
        .check_path(&child, &bob)
        .unwrap();
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
    CapabilitiesRepository::new(&store)
        .set_subgroup_visibility(&child, VisibilityMode::Open)
        .unwrap();

    // Buggy ordering (what the pre-fix `join_group` catch-up did when a
    // node caught up on an earlier member's `MemberJoined` before its
    // own `set_default_capabilities` ran): member added while default
    // caps are still unset → no per-member capability row is written.
    MembershipRepository::new(&store)
        .add_member(&ns, &bob, GroupMemberRole::Member)
        .unwrap();
    CapabilitiesRepository::new(&store)
        .set_default_capabilities(&ns, MemberCapabilities::CAN_JOIN_OPEN_SUBGROUPS)
        .unwrap();

    // The later `set_default_capabilities` does NOT retroactively
    // materialize bob's per-member cap, so the inheritance check still
    // returns `None`. This is exactly the state that produced
    // `MemberJoinedOpen rejected: no membership path` on the
    // later-joining peer; the `join_group` handler fix prevents it by
    // ordering `set_default_capabilities` before the catch-up apply.
    let path = MembershipRepository::new(&store)
        .check_path(&child, &bob)
        .unwrap();
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
    MembershipRepository::new(&store)
        .add_member(&parent, &alice, GroupMemberRole::Member)
        .unwrap();
    CapabilitiesRepository::new(&store)
        .set_member_capability(&parent, &alice, MemberCapabilities::CAN_JOIN_OPEN_SUBGROUPS)
        .unwrap();

    // Restricted child blocks inheritance even when the cap is set.
    CapabilitiesRepository::new(&store)
        .set_subgroup_visibility(&child, VisibilityMode::Restricted)
        .unwrap();
    assert!(!MembershipRepository::new(&store)
        .is_member(&child, &alice)
        .unwrap());
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

    MembershipRepository::new(&store)
        .add_member(&namespace, &alice, GroupMemberRole::Member)
        .unwrap();
    CapabilitiesRepository::new(&store)
        .set_member_capability(
            &namespace,
            &alice,
            MemberCapabilities::CAN_JOIN_OPEN_SUBGROUPS,
        )
        .unwrap();

    CapabilitiesRepository::new(&store)
        .set_subgroup_visibility(&mid, VisibilityMode::Restricted)
        .unwrap();
    CapabilitiesRepository::new(&store)
        .set_subgroup_visibility(&leaf, VisibilityMode::Open)
        .unwrap();

    // The walk hits `mid` (Restricted) and stops before reaching the
    // namespace; alice is not inherited into `leaf`.
    assert!(!MembershipRepository::new(&store)
        .is_member(&leaf, &alice)
        .unwrap());
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

    MembershipRepository::new(&store)
        .add_member(&namespace, &alice, GroupMemberRole::Member)
        .unwrap();
    CapabilitiesRepository::new(&store)
        .set_member_capability(
            &namespace,
            &alice,
            MemberCapabilities::CAN_JOIN_OPEN_SUBGROUPS,
        )
        .unwrap();

    CapabilitiesRepository::new(&store)
        .set_subgroup_visibility(&mid, VisibilityMode::Open)
        .unwrap();
    CapabilitiesRepository::new(&store)
        .set_subgroup_visibility(&leaf, VisibilityMode::Open)
        .unwrap();

    assert!(MembershipRepository::new(&store)
        .is_member(&leaf, &alice)
        .unwrap());
}

#[test]
fn check_membership_unset_visibility_treated_as_restricted() {
    let store = test_store();
    let parent = ContextGroupId::from([0xE0; 32]);
    let child = ContextGroupId::from([0xE1; 32]);
    let alice = PublicKey::from([0x01; 32]);

    nest_for_test(&store, &parent, &child);
    MembershipRepository::new(&store)
        .add_member(&parent, &alice, GroupMemberRole::Member)
        .unwrap();
    CapabilitiesRepository::new(&store)
        .set_member_capability(
            &parent,
            &alice,
            calimero_context_config::MemberCapabilities::CAN_JOIN_OPEN_SUBGROUPS,
        )
        .unwrap();

    // No `subgroup_visibility` set on `child` — should behave as Restricted.
    assert!(!MembershipRepository::new(&store)
        .is_member(&child, &alice)
        .unwrap());
}

#[test]
fn check_membership_open_subgroup_blocked_when_cap_revoked() {
    use calimero_context_config::VisibilityMode;

    let store = test_store();
    let parent = ContextGroupId::from([0xF0; 32]);
    let child = ContextGroupId::from([0xF1; 32]);
    let alice = PublicKey::from([0x01; 32]);

    nest_for_test(&store, &parent, &child);
    MembershipRepository::new(&store)
        .add_member(&parent, &alice, GroupMemberRole::Member)
        .unwrap();
    // Cap explicitly cleared (admin used the deny-list).
    CapabilitiesRepository::new(&store)
        .set_member_capability(&parent, &alice, 0)
        .unwrap();

    CapabilitiesRepository::new(&store)
        .set_subgroup_visibility(&child, VisibilityMode::Open)
        .unwrap();
    assert!(!MembershipRepository::new(&store)
        .is_member(&child, &alice)
        .unwrap());
}

#[test]
fn check_membership_open_subgroup_admin_inherits_without_cap() {
    use calimero_context_config::VisibilityMode;

    let store = test_store();
    let parent = ContextGroupId::from([0x10; 32]);
    let child = ContextGroupId::from([0x11; 32]);
    let admin = PublicKey::from([0x01; 32]);

    nest_for_test(&store, &parent, &child);
    MembershipRepository::new(&store)
        .add_member(&parent, &admin, GroupMemberRole::Admin)
        .unwrap();
    // Cap cleared — but admin override kicks in.
    CapabilitiesRepository::new(&store)
        .set_member_capability(&parent, &admin, 0)
        .unwrap();

    CapabilitiesRepository::new(&store)
        .set_subgroup_visibility(&child, VisibilityMode::Open)
        .unwrap();
    assert!(MembershipRepository::new(&store)
        .is_member(&child, &admin)
        .unwrap());
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

    MembershipRepository::new(&store)
        .add_member(&namespace, &alice, GroupMemberRole::Member)
        .unwrap();
    CapabilitiesRepository::new(&store)
        .set_member_capability(
            &namespace,
            &alice,
            MemberCapabilities::CAN_JOIN_OPEN_SUBGROUPS,
        )
        .unwrap();

    MembershipRepository::new(&store)
        .add_member(&mid, &alice, GroupMemberRole::Member)
        .unwrap();
    CapabilitiesRepository::new(&store)
        .set_member_capability(&mid, &alice, 0)
        .unwrap();

    CapabilitiesRepository::new(&store)
        .set_subgroup_visibility(&mid, VisibilityMode::Open)
        .unwrap();
    CapabilitiesRepository::new(&store)
        .set_subgroup_visibility(&leaf, VisibilityMode::Open)
        .unwrap();

    assert!(!MembershipRepository::new(&store)
        .is_member(&leaf, &alice)
        .unwrap());
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
    MembershipRepository::new(&store)
        .add_member(&namespace, &bob, GroupMemberRole::Member)
        .unwrap();
    CapabilitiesRepository::new(&store)
        .set_member_capability(
            &namespace,
            &bob,
            MemberCapabilities::CAN_JOIN_OPEN_SUBGROUPS,
        )
        .unwrap();

    // Alice is an explicit Admin of the Open `reports` subgroup.
    MembershipRepository::new(&store)
        .add_member(&reports, &alice, GroupMemberRole::Admin)
        .unwrap();
    CapabilitiesRepository::new(&store)
        .set_subgroup_visibility(&reports, VisibilityMode::Open)
        .unwrap();

    // Contract sanity: Bob *is* a member of `reports` by inheritance...
    assert!(MembershipRepository::new(&store)
        .is_member(&reports, &bob)
        .unwrap());
    // ...but he has no stored row, so `list_group_members` omits him —
    // this is the gap issue #2371 reports.
    let stored = MembershipRepository::new(&store)
        .list(&reports, 0, usize::MAX)
        .unwrap();
    assert!(
        stored.iter().all(|(pk, _)| *pk != bob),
        "precondition: inherited joiner has no stored GroupMember row"
    );

    // `enumerate_inherited_members` recovers Bob as an inherited member.
    let inherited = MembershipRepository::new(&store)
        .enumerate_inherited(&reports)
        .unwrap();
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
fn enumerate_inherited_members_preserves_read_only_tee_role() {
    use calimero_context_config::{MemberCapabilities, VisibilityMode};

    // A `ReadOnlyTee` admitted at the namespace root (direct row there) and
    // granted CAN_JOIN_OPEN_SUBGROUPS inherits into an Open subgroup with no
    // direct row of its own. `enumerate_inherited` must surface it with its
    // real anchor role (`ReadOnlyTee`), NOT collapse it to plain `Member` —
    // the live-desktop bug where an inherited TEE node showed as `Member`.
    let store = test_store();
    let namespace = ContextGroupId::from([0x40; 32]);
    let reports = ContextGroupId::from([0x41; 32]);
    let tee = PublicKey::from([0x03; 32]);

    nest_for_test(&store, &namespace, &reports);

    // TEE node is a direct ReadOnlyTee member of the namespace root with the
    // join cap, but has NO direct row in the Open subgroup.
    MembershipRepository::new(&store)
        .add_member(&namespace, &tee, GroupMemberRole::ReadOnlyTee)
        .unwrap();
    CapabilitiesRepository::new(&store)
        .set_member_capability(
            &namespace,
            &tee,
            MemberCapabilities::CAN_JOIN_OPEN_SUBGROUPS,
        )
        .unwrap();
    CapabilitiesRepository::new(&store)
        .set_subgroup_visibility(&reports, VisibilityMode::Open)
        .unwrap();

    // Precondition: the TEE node has no stored row in the Open subgroup.
    let stored = MembershipRepository::new(&store)
        .list(&reports, 0, usize::MAX)
        .unwrap();
    assert!(
        stored.iter().all(|(pk, _)| *pk != tee),
        "precondition: inherited TEE node has no stored GroupMember row"
    );

    let inherited = MembershipRepository::new(&store)
        .enumerate_inherited(&reports)
        .unwrap();
    assert!(
        inherited
            .iter()
            .any(|(pk, role)| *pk == tee && *role == GroupMemberRole::ReadOnlyTee),
        "inherited ReadOnlyTee must keep the ReadOnlyTee role, not collapse to Member, got {inherited:?}"
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
    MembershipRepository::new(&store)
        .add_member(&namespace, &bob, GroupMemberRole::Member)
        .unwrap();
    CapabilitiesRepository::new(&store)
        .set_member_capability(
            &namespace,
            &bob,
            MemberCapabilities::CAN_JOIN_OPEN_SUBGROUPS,
        )
        .unwrap();
    CapabilitiesRepository::new(&store)
        .set_subgroup_visibility(&reports, VisibilityMode::Open)
        .unwrap();

    // Pre-kick: Bob is an inherited member of `reports`.
    let inherited = MembershipRepository::new(&store)
        .enumerate_inherited(&reports)
        .unwrap();
    assert!(
        inherited.iter().any(|(pk, _)| *pk == bob),
        "precondition: Bob inherits membership of the Open subgroup"
    );

    // Kick: deny-list Bob on `reports` (what `MemberRemoved` /
    // `MemberLeft` apply does for a subgroup-level removal).
    DenyListRepository::new(&store)
        .mark(&reports, &bob)
        .unwrap();

    let after_kick = MembershipRepository::new(&store)
        .enumerate_inherited(&reports)
        .unwrap();
    assert!(
        after_kick.iter().all(|(pk, _)| *pk != bob),
        "deny-listed (kicked) member must NOT appear as an inherited member, got {after_kick:?}"
    );

    // Rejoin clears the deny-list — Bob reappears.
    DenyListRepository::new(&store)
        .clear(&reports, &bob)
        .unwrap();
    let after_rejoin = MembershipRepository::new(&store)
        .enumerate_inherited(&reports)
        .unwrap();
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
    MembershipRepository::new(&store)
        .add_member(&namespace, &admin, GroupMemberRole::Admin)
        .unwrap();
    CapabilitiesRepository::new(&store)
        .set_subgroup_visibility(&reports, VisibilityMode::Open)
        .unwrap();

    let inherited = MembershipRepository::new(&store)
        .enumerate_inherited(&reports)
        .unwrap();
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
    MembershipRepository::new(&store)
        .add_member(&namespace, &bob, GroupMemberRole::Member)
        .unwrap();
    CapabilitiesRepository::new(&store)
        .set_member_capability(
            &namespace,
            &bob,
            MemberCapabilities::CAN_JOIN_OPEN_SUBGROUPS,
        )
        .unwrap();
    CapabilitiesRepository::new(&store)
        .set_subgroup_visibility(&reports, VisibilityMode::Restricted)
        .unwrap();

    let inherited = MembershipRepository::new(&store)
        .enumerate_inherited(&reports)
        .unwrap();
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
        nest_for_test_unchecked(&store, &prev, &next);
        CapabilitiesRepository::new(&store)
            .set_subgroup_visibility(&next, VisibilityMode::Open)
            .unwrap();
        prev = next;
    }

    let res = MembershipRepository::new(&store).enumerate_inherited(&prev);
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
        CapabilitiesRepository::new(&store)
            .set_subgroup_visibility(&g, VisibilityMode::Open)
            .unwrap();
        nodes.push(g);
    }
    let leaf = *nodes.last().unwrap();

    let alice = PublicKey::from([0x01; 32]);
    MembershipRepository::new(&store)
        .add_member(&ns, &alice, GroupMemberRole::Member)
        .unwrap();
    CapabilitiesRepository::new(&store)
        .set_member_capability(&ns, &alice, MemberCapabilities::CAN_JOIN_OPEN_SUBGROUPS)
        .unwrap();

    assert!(
        MembershipRepository::new(&store)
            .is_member(&leaf, &alice)
            .unwrap(),
        "precondition: check_group_membership resolves at the boundary"
    );

    let inherited = MembershipRepository::new(&store)
        .enumerate_inherited(&leaf)
        .unwrap();
    assert!(
        inherited.iter().any(|(pk, _)| *pk == alice),
        "enumerate_inherited_members must reach the deepest ancestor at \
         MAX_NAMESPACE_DEPTH, matching check_group_membership; got {inherited:?}"
    );
}

#[test]
fn inherited_admin_walk_independent_of_direct_non_admin_membership() {
    use calimero_context_config::VisibilityMode;
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
    MembershipRepository::new(&store)
        .add_member(&parent, &alice, GroupMemberRole::Admin)
        .unwrap();
    MembershipRepository::new(&store)
        .add_member(&child, &alice, GroupMemberRole::Member)
        .unwrap();

    CapabilitiesRepository::new(&store)
        .set_subgroup_visibility(&child, VisibilityMode::Open)
        .unwrap();

    // Inherited admin authority must hold despite Alice's direct
    // non-admin membership in `child`.
    assert!(
        MembershipRepository::new(&store)
            .is_inherited_admin(&child, &alice)
            .unwrap(),
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
    MembershipRepository::new(&store)
        .add_member(&ns, &alice, GroupMemberRole::Admin)
        .unwrap();
    MembershipRepository::new(&store)
        .add_member(&mid, &alice, GroupMemberRole::Member)
        .unwrap();
    CapabilitiesRepository::new(&store)
        .set_member_capability(&mid, &alice, 0)
        .unwrap();

    CapabilitiesRepository::new(&store)
        .set_subgroup_visibility(&mid, VisibilityMode::Open)
        .unwrap();
    CapabilitiesRepository::new(&store)
        .set_subgroup_visibility(&leaf, VisibilityMode::Open)
        .unwrap();

    // Both authorization surfaces must agree: Alice is authorized.
    assert!(MembershipRepository::new(&store)
        .is_inherited_admin(&leaf, &alice)
        .unwrap());
    let path = MembershipRepository::new(&store)
        .check_path(&leaf, &alice)
        .unwrap();
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
            "expected Inherited{{ via_admin: true, anchor: ns }} for parent admin, got {other:?}"
        ),
    }

    // Sanity: a non-admin in the same shape must still be denied —
    // the fix does NOT widen authorization for non-admins, only
    // honors admin authority that already exists higher up.
    let bob = PublicKey::from([0x02; 32]);
    MembershipRepository::new(&store)
        .add_member(&ns, &bob, GroupMemberRole::Member)
        .unwrap();
    CapabilitiesRepository::new(&store)
        .set_member_capability(&ns, &bob, MemberCapabilities::CAN_JOIN_OPEN_SUBGROUPS)
        .unwrap();
    MembershipRepository::new(&store)
        .add_member(&mid, &bob, GroupMemberRole::Member)
        .unwrap();
    CapabilitiesRepository::new(&store)
        .set_member_capability(&mid, &bob, 0)
        .unwrap();
    assert!(
        !MembershipRepository::new(&store)
            .is_member(&leaf, &bob)
            .unwrap(),
        "non-admin with cleared cap at intermediate anchor must still be denied; \
         the fix only cascades *admin* authority, not arbitrary parent membership"
    );
}

#[test]
fn auth_and_crypto_walks_agree_at_max_namespace_depth_boundary() {
    use super::super::namespace::MAX_NAMESPACE_DEPTH;
    use calimero_context_config::{MemberCapabilities, VisibilityMode};
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
        CapabilitiesRepository::new(&store)
            .set_subgroup_visibility(&g, VisibilityMode::Open)
            .unwrap();
        nodes.push(g);
    }
    let leaf = *nodes.last().unwrap();

    // Sanity: chain check succeeds at the boundary.
    assert!(
        CapabilitiesRepository::new(&store)
            .is_open_chain_to_namespace(&leaf, &ns)
            .unwrap(),
        "is_open_chain_to_namespace should resolve at chain length MAX_NAMESPACE_DEPTH"
    );

    // The bug: membership walks used to bail here. After the fix,
    // they must resolve to a definite answer (no cycle error).
    let alice = PublicKey::from([0x01; 32]);
    MembershipRepository::new(&store)
        .add_member(&ns, &alice, GroupMemberRole::Member)
        .unwrap();
    CapabilitiesRepository::new(&store)
        .set_member_capability(&ns, &alice, MemberCapabilities::CAN_JOIN_OPEN_SUBGROUPS)
        .unwrap();

    // is_inherited_admin: alice is not admin anywhere → should
    // resolve to false (NOT bail).
    assert!(
        matches!(
            MembershipRepository::new(&store).is_inherited_admin(&leaf, &alice),
            Ok(false)
        ),
        "is_inherited_admin must terminate at chain length MAX_NAMESPACE_DEPTH, not bail"
    );

    // check_group_membership: alice has CAN_JOIN_OPEN_SUBGROUPS at
    // the namespace, all intermediate links are Open → should
    // resolve to true via inheritance, not bail.
    assert!(
        matches!(
            MembershipRepository::new(&store).is_member(&leaf, &alice),
            Ok(true)
        ),
        "check_group_membership must resolve at chain length MAX_NAMESPACE_DEPTH, not bail"
    );

    // Promoting alice to admin should also be observed (governance
    // surface in agreement).
    let bob = PublicKey::from([0x02; 32]);
    MembershipRepository::new(&store)
        .add_member(&ns, &bob, GroupMemberRole::Admin)
        .unwrap();
    assert!(
        matches!(
            MembershipRepository::new(&store).is_inherited_admin(&leaf, &bob),
            Ok(true)
        ),
        "inherited admin authority must reach the leaf at chain length MAX_NAMESPACE_DEPTH"
    );
}

#[test]
fn has_direct_group_member_ignores_open_chain_inheritance() {
    use calimero_context_config::VisibilityMode;
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
    MembershipRepository::new(&store)
        .add_member(&parent, &alice, GroupMemberRole::Admin)
        .unwrap();
    CapabilitiesRepository::new(&store)
        .set_subgroup_visibility(&child, VisibilityMode::Open)
        .unwrap();

    // Inheritance-aware path *should* see Alice (admin inheritance from parent).
    assert!(MembershipRepository::new(&store)
        .is_member(&child, &alice)
        .unwrap());

    // Direct-only path *must not* see her — that's exactly the signal
    // the bootstrap/dedup guards need to know they still have to write
    // the direct row.
    assert!(
        !MembershipRepository::new(&store)
            .has_direct_member(&child, &alice)
            .unwrap(),
        "has_direct_group_member must ignore Open-chain inheritance and \
         report only on the direct membership row"
    );

    // After explicitly adding her to the child, both views agree.
    MembershipRepository::new(&store)
        .add_member(&child, &alice, GroupMemberRole::Member)
        .unwrap();
    assert!(MembershipRepository::new(&store)
        .has_direct_member(&child, &alice)
        .unwrap());
    assert!(MembershipRepository::new(&store)
        .is_member(&child, &alice)
        .unwrap());
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
    CapabilitiesRepository::new(&store)
        .set_subgroup_visibility(&child, VisibilityMode::Restricted)
        .unwrap();

    // No parent membership; alice is added directly to the Restricted child.
    MembershipRepository::new(&store)
        .add_member(&child, &alice, GroupMemberRole::Member)
        .unwrap();
    assert!(MembershipRepository::new(&store)
        .is_member(&child, &alice)
        .unwrap());
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
    MetaRepository::new(&store).save(&gid, &meta).unwrap();

    let pks = MembershipRepository::new(&store)
        .namespace_pubkeys(namespace_id)
        .unwrap();
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
    MetaRepository::new(&store).save(&gid, &meta).unwrap();
    MembershipRepository::new(&store)
        .add_member(&gid, &admin, GroupMemberRole::Admin)
        .unwrap();
    MembershipRepository::new(&store)
        .add_member(&gid, &other, GroupMemberRole::Member)
        .unwrap();

    let pks = MembershipRepository::new(&store)
        .namespace_pubkeys(namespace_id)
        .unwrap();
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

    MembershipRepository::new(&store)
        .add_member(&gid, &m1, GroupMemberRole::Member)
        .unwrap();
    MembershipRepository::new(&store)
        .add_member(&gid, &m2, GroupMemberRole::Admin)
        .unwrap();

    let pks = MembershipRepository::new(&store)
        .namespace_pubkeys(namespace_id)
        .unwrap();
    assert!(pks.contains(&m1));
    assert!(pks.contains(&m2));
}

#[test]
fn acl_view_at_branch1_member_when_heads_match_and_member_exists() {
    let store = test_store();
    let gid = test_group_id();
    let signer = PublicKey::from([0x42; 32]);

    MetaRepository::new(&store)
        .save(&gid, &test_meta())
        .unwrap();
    MembershipRepository::new(&store)
        .add_member(&gid, &signer, GroupMemberRole::Admin)
        .unwrap();

    // Top-level group → namespace_id == group_id, local heads == [].
    let status = acl_view_at(&store, gid, &signer, &[]).unwrap();
    assert!(matches!(
        status,
        MembershipStatus::Member(GroupMemberRole::Admin)
    ));
}

#[test]
fn acl_view_at_branch1_never_member_when_signer_absent() {
    let store = test_store();
    let gid = test_group_id();
    let admin = PublicKey::from([0x01; 32]);
    let stranger = PublicKey::from([0xFE; 32]);

    MetaRepository::new(&store)
        .save(&gid, &test_meta())
        .unwrap();
    MembershipRepository::new(&store)
        .add_member(&gid, &admin, GroupMemberRole::Admin)
        .unwrap();

    let status = acl_view_at(&store, gid, &stranger, &[]).unwrap();
    assert!(matches!(status, MembershipStatus::NeverMember));
}

#[test]
fn acl_view_at_branch2_unknown_when_heads_not_in_op_log() {
    let store = test_store();
    let gid = test_group_id();
    let signer = PublicKey::from([0x42; 32]);

    MetaRepository::new(&store)
        .save(&gid, &test_meta())
        .unwrap();
    MembershipRepository::new(&store)
        .add_member(&gid, &signer, GroupMemberRole::Member)
        .unwrap();

    // Edge references a head that's not in the local op log.
    let unknown_head = [0xCC; 32];
    let status = acl_view_at(&store, gid, &signer, &[unknown_head]).unwrap();
    match status {
        MembershipStatus::Unknown { needed } => {
            assert_eq!(needed, vec![unknown_head]);
        }
        other => panic!("expected Unknown, got {other:?}"),
    }
}

#[test]
fn acl_view_at_branch2_unknown_collects_all_missing_heads() {
    let store = test_store();
    let gid = test_group_id();
    let signer = PublicKey::from([0x42; 32]);

    MetaRepository::new(&store)
        .save(&gid, &test_meta())
        .unwrap();
    MembershipRepository::new(&store)
        .add_member(&gid, &signer, GroupMemberRole::Member)
        .unwrap();

    // Multiple unknown heads — all should be reported (round-2 fix).
    let h1 = [0xC1; 32];
    let h2 = [0xC2; 32];
    let h3 = [0xC3; 32];
    let status = acl_view_at(&store, gid, &signer, &[h1, h2, h3]).unwrap();
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
fn acl_view_at_rejects_membership_in_a_different_group() {
    let store = test_store();
    let gid_a = ContextGroupId::from([0xAA; 32]);
    let gid_b = ContextGroupId::from([0xBB; 32]);
    let signer = PublicKey::from([0x42; 32]);

    MetaRepository::new(&store)
        .save(&gid_a, &test_meta())
        .unwrap();
    MembershipRepository::new(&store)
        .add_member(&gid_a, &signer, GroupMemberRole::Member)
        .unwrap();

    // The signer is a member of group A but the caller resolves against
    // group B (the parent-edge model: the group comes from the context, not
    // a signer claim). B isn't set up, so resolution proceeds against its
    // empty member set. Either an error or a non-Member result is a correct
    // rejection — returning Member would be the security failure.
    let result = acl_view_at(&store, gid_b, &signer, &[]);
    if let Ok(MembershipStatus::Member(_)) = result {
        panic!("must not return Member when authorizing against a different group");
    }
}

#[test]
fn acl_view_at_oversized_governance_dag_heads_runtime_guard() {
    use calimero_context_config::types::MAX_GOVERNANCE_DAG_HEADS;
    let store = test_store();
    let gid = test_group_id();
    let signer = PublicKey::from([0x42; 32]);

    MetaRepository::new(&store)
        .save(&gid, &test_meta())
        .unwrap();

    // Defense-in-depth: `acl_view_at` enforces the bound on the heads slice
    // before any store work, even when the edge was hand-built past the
    // constructor's check.
    let oversized_heads: Vec<[u8; 32]> = (0..MAX_GOVERNANCE_DAG_HEADS + 1)
        .map(|i| {
            let mut h = [0u8; 32];
            h[0] = i as u8;
            h
        })
        .collect();

    let err = acl_view_at(&store, gid, &signer, &oversized_heads).unwrap_err();
    assert!(
        matches!(
            err.downcast_ref::<ApplyError>(),
            Some(ApplyError::DagHeadsExceeded)
        ),
        "expected oversize error, got: {err}"
    );
}

#[test]
fn fast_path_current_member_resolves_to_member() {
    // Baseline: a member who exists in the materialized set resolves
    // to `Member(role)` when the position's heads equal local heads.
    // Required precondition for the other Branch 1 tests — if this
    // baseline fails, the others say nothing.
    let store = test_store();
    let gid = test_group_id();
    let signer = PublicKey::from([0x77; 32]);

    MetaRepository::new(&store)
        .save(&gid, &test_meta())
        .unwrap();
    MembershipRepository::new(&store)
        .add_member(&gid, &signer, GroupMemberRole::Member)
        .unwrap();

    let status = acl_view_at(&store, gid, &signer, &[]).unwrap();
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
    let store = test_store();
    let gid = test_group_id();
    let signer = PublicKey::from([0x78; 32]);

    MetaRepository::new(&store)
        .save(&gid, &test_meta())
        .unwrap();
    MembershipRepository::new(&store)
        .add_member(&gid, &signer, GroupMemberRole::Member)
        .unwrap();
    MembershipRepository::new(&store)
        .remove_member(&gid, &signer)
        .unwrap();

    let status = acl_view_at(&store, gid, &signer, &[]).unwrap();
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
    let store = test_store();
    let gid = test_group_id();
    let signer = PublicKey::from([0x79; 32]);

    MetaRepository::new(&store)
        .save(&gid, &test_meta())
        .unwrap();
    MembershipRepository::new(&store)
        .add_member(&gid, &signer, GroupMemberRole::Member)
        .unwrap();
    MembershipRepository::new(&store)
        .remove_member(&gid, &signer)
        .unwrap();
    MembershipRepository::new(&store)
        .add_member(&gid, &signer, GroupMemberRole::Admin)
        .unwrap();

    let status = acl_view_at(&store, gid, &signer, &[]).unwrap();
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
    let store = test_store();
    let gid = test_group_id();
    let signer = PublicKey::from([0x7A; 32]);

    MetaRepository::new(&store)
        .save(&gid, &test_meta())
        .unwrap();
    MembershipRepository::new(&store)
        .add_member(&gid, &signer, GroupMemberRole::Member)
        .unwrap();
    // Re-add with Admin role (simulates a role change in the
    // materialized layer — at the namespace governance layer this
    // would be a `MemberRoleSet`).
    MembershipRepository::new(&store)
        .add_member(&gid, &signer, GroupMemberRole::Admin)
        .unwrap();

    let status = acl_view_at(&store, gid, &signer, &[]).unwrap();
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
    MetaRepository::new(&store)
        .save(&gid, &test_meta())
        .unwrap();
    MembershipRepository::new(&store)
        .add_member(&gid, &member, GroupMemberRole::Member)
        .unwrap();
    MetadataRepository::new(&store)
        .set_member(
            &gid,
            &member,
            &MetadataRecord {
                name: Some("departing".to_owned()),
                ..Default::default()
            },
        )
        .unwrap();
    assert!(MetadataRepository::new(&store)
        .member_metadata(&gid, &member)
        .unwrap()
        .is_some());

    MembershipRepository::new(&store)
        .remove_member(&gid, &member)
        .unwrap();
    assert!(MetadataRepository::new(&store)
        .member_metadata(&gid, &member)
        .unwrap()
        .is_none());
}

#[test]
fn trusted_anchors_empty_when_meta_absent() {
    // No `save_group_meta` call — no group exists. Should return an
    // empty set, not error. Caller falls back to random selection.
    let store = test_store();
    let gid = test_group_id();
    let anchors = MembershipRepository::new(&store)
        .trusted_anchors(&gid)
        .unwrap();
    assert!(anchors.is_empty(), "expected empty set, got {anchors:?}");
}

#[test]
fn trusted_anchors_includes_owner_and_legacy_admin() {
    // Fresh group: owner_identity == admin_identity (the creator).
    // Anchor set contains just that one pubkey.
    let store = test_store();
    let gid = test_group_id();
    let creator = PublicKey::from([0x01; 32]);
    MetaRepository::new(&store)
        .save(&gid, &sample_meta_with_admin(creator))
        .unwrap();

    let anchors = MembershipRepository::new(&store)
        .trusted_anchors(&gid)
        .unwrap();
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
    MetaRepository::new(&store).save(&gid, &meta).unwrap();

    let anchors = MembershipRepository::new(&store)
        .trusted_anchors(&gid)
        .unwrap();
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

    MetaRepository::new(&store)
        .save(&gid, &sample_meta_with_admin(creator))
        .unwrap();
    MembershipRepository::new(&store)
        .add_member(&gid, &admin_a, GroupMemberRole::Admin)
        .unwrap();
    MembershipRepository::new(&store)
        .add_member(&gid, &admin_b, GroupMemberRole::Admin)
        .unwrap();

    let anchors = MembershipRepository::new(&store)
        .trusted_anchors(&gid)
        .unwrap();
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

    MetaRepository::new(&store)
        .save(&gid, &sample_meta_with_admin(creator))
        .unwrap();
    MembershipRepository::new(&store)
        .add_member(&gid, &tee_attested, GroupMemberRole::ReadOnlyTee)
        .unwrap();

    let anchors = MembershipRepository::new(&store)
        .trusted_anchors(&gid)
        .unwrap();
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

    MetaRepository::new(&store)
        .save(&gid, &sample_meta_with_admin(creator))
        .unwrap();
    MembershipRepository::new(&store)
        .add_member(&gid, &member, GroupMemberRole::Member)
        .unwrap();
    MembershipRepository::new(&store)
        .add_member(&gid, &read_only, GroupMemberRole::ReadOnly)
        .unwrap();

    let anchors = MembershipRepository::new(&store)
        .trusted_anchors(&gid)
        .unwrap();
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
    MetaRepository::new(&store).save(&gid, &meta).unwrap();
    MembershipRepository::new(&store)
        .add_member(&gid, &admin_a, GroupMemberRole::Admin)
        .unwrap();
    MembershipRepository::new(&store)
        .add_member(&gid, &admin_b, GroupMemberRole::Admin)
        .unwrap();
    MembershipRepository::new(&store)
        .add_member(&gid, &tee, GroupMemberRole::ReadOnlyTee)
        .unwrap();
    MembershipRepository::new(&store)
        .add_member(&gid, &member, GroupMemberRole::Member)
        .unwrap();
    MembershipRepository::new(&store)
        .add_member(&gid, &read_only, GroupMemberRole::ReadOnly)
        .unwrap();

    let anchors = MembershipRepository::new(&store)
        .trusted_anchors(&gid)
        .unwrap();
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
    MembershipRepository::new(&store)
        .add_member(&namespace, &bob, GroupMemberRole::Member)
        .unwrap();
    CapabilitiesRepository::new(&store)
        .set_member_capability(
            &namespace,
            &bob,
            MemberCapabilities::CAN_JOIN_OPEN_SUBGROUPS,
        )
        .unwrap();
    CapabilitiesRepository::new(&store)
        .set_subgroup_visibility(&reports, VisibilityMode::Open)
        .unwrap();

    // Contract sanity: Bob *is* an effective member of `reports` by
    // inheritance, but he has no stored row there.
    assert!(MembershipRepository::new(&store)
        .is_member(&reports, &bob)
        .unwrap());
    assert!(
        MembershipRepository::new(&store)
            .role_of(&reports, &bob)
            .unwrap()
            .is_none(),
        "precondition: inherited joiner has no direct GroupMember row"
    );

    // The gate behind `get_member_capabilities` must surface Bob as a
    // member with an effective bitmask of 0 — before the #2378 fix it
    // returned `None` and the handler bailed "identity is not a member".
    assert_eq!(
        MembershipRepository::new(&store)
            .effective_capabilities(&reports, &bob)
            .unwrap(),
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
    MembershipRepository::new(&store)
        .add_member(&namespace, &admin, GroupMemberRole::Admin)
        .unwrap();
    CapabilitiesRepository::new(&store)
        .set_subgroup_visibility(&reports, VisibilityMode::Open)
        .unwrap();

    assert!(MembershipRepository::new(&store)
        .is_member(&reports, &admin)
        .unwrap());
    assert_eq!(
        MembershipRepository::new(&store)
            .effective_capabilities(&reports, &admin)
            .unwrap(),
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

    MembershipRepository::new(&store)
        .add_member(&group, &carol, GroupMemberRole::Member)
        .unwrap();
    CapabilitiesRepository::new(&store)
        .set_member_capability(&group, &carol, MemberCapabilities::CAN_JOIN_OPEN_SUBGROUPS)
        .unwrap();

    assert_eq!(
        MembershipRepository::new(&store)
            .effective_capabilities(&group, &carol)
            .unwrap(),
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

    MembershipRepository::new(&store)
        .add_member(&group, &dave, GroupMemberRole::Member)
        .unwrap();
    // No `set_member_capability` call — no capability row is stored.

    assert_eq!(
        MembershipRepository::new(&store)
            .effective_capabilities(&group, &dave)
            .unwrap(),
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
        MembershipRepository::new(&store)
            .effective_capabilities(&group, &stranger)
            .unwrap(),
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
    MembershipRepository::new(&store)
        .add_member(&namespace, &bob, GroupMemberRole::Member)
        .unwrap();
    CapabilitiesRepository::new(&store)
        .set_member_capability(
            &namespace,
            &bob,
            MemberCapabilities::CAN_JOIN_OPEN_SUBGROUPS,
        )
        .unwrap();
    CapabilitiesRepository::new(&store)
        .set_subgroup_visibility(&reports, VisibilityMode::Restricted)
        .unwrap();

    assert!(!MembershipRepository::new(&store)
        .is_member(&reports, &bob)
        .unwrap());
    assert_eq!(
        MembershipRepository::new(&store)
            .effective_capabilities(&reports, &bob)
            .unwrap(),
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
    MembershipRepository::new(&store)
        .add_member(&namespace, &bob, GroupMemberRole::Member)
        .unwrap();
    CapabilitiesRepository::new(&store)
        .set_member_capability(
            &namespace,
            &bob,
            MemberCapabilities::CAN_JOIN_OPEN_SUBGROUPS,
        )
        .unwrap();
    CapabilitiesRepository::new(&store)
        .set_subgroup_visibility(&reports, VisibilityMode::Open)
        .unwrap();

    // Pre-kick: Bob inherits membership and the gate accepts him.
    assert_eq!(
        MembershipRepository::new(&store)
            .effective_capabilities(&reports, &bob)
            .unwrap(),
        Some(0),
        "precondition: inherited joiner resolves to Some(0) before the kick"
    );

    // Kick: deny-list Bob on `reports` (what `MemberRemoved` / `MemberLeft`
    // apply does for a subgroup-level removal).
    DenyListRepository::new(&store)
        .mark(&reports, &bob)
        .unwrap();

    assert_eq!(
        MembershipRepository::new(&store)
            .effective_capabilities(&reports, &bob)
            .unwrap(),
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
    CapabilitiesRepository::new(&store)
        .set_subgroup_visibility(&child, VisibilityMode::Open)
        .unwrap();

    // An `Open` subgroup's existence is public by design — it is listed
    // even for a caller that cannot be identified and one that is a
    // total non-member.
    assert!(MembershipRepository::new(&store)
        .subgroup_visible_to(&parent, &child, None)
        .unwrap());
    assert!(MembershipRepository::new(&store)
        .subgroup_visible_to(&parent, &child, Some(&stranger))
        .unwrap());
}

#[test]
fn subgroup_visible_to_restricted_child_hidden_from_non_member() {
    use calimero_context_config::VisibilityMode;

    let store = test_store();
    let parent = ContextGroupId::from([0xD2; 32]);
    let child = ContextGroupId::from([0xD3; 32]);
    let stranger = PublicKey::from([0x09; 32]);

    nest_for_test(&store, &parent, &child);
    CapabilitiesRepository::new(&store)
        .set_subgroup_visibility(&child, VisibilityMode::Restricted)
        .unwrap();

    // The caller is neither a parent admin nor a member of the
    // restricted child — its existence must not leak through the list.
    assert!(!MembershipRepository::new(&store)
        .subgroup_visible_to(&parent, &child, Some(&stranger))
        .unwrap());
}

#[test]
fn subgroup_visible_to_restricted_child_hidden_when_caller_unknown() {
    use calimero_context_config::VisibilityMode;

    let store = test_store();
    let parent = ContextGroupId::from([0xD4; 32]);
    let child = ContextGroupId::from([0xD5; 32]);

    nest_for_test(&store, &parent, &child);
    CapabilitiesRepository::new(&store)
        .set_subgroup_visibility(&child, VisibilityMode::Restricted)
        .unwrap();

    // `caller == None` (node has no namespace identity for the parent):
    // membership cannot be verified, so the conservative choice hides
    // the restricted child.
    assert!(!MembershipRepository::new(&store)
        .subgroup_visible_to(&parent, &child, None)
        .unwrap());
}

#[test]
fn subgroup_visible_to_restricted_child_visible_to_parent_admin() {
    use calimero_context_config::VisibilityMode;

    let store = test_store();
    let parent = ContextGroupId::from([0xD6; 32]);
    let child = ContextGroupId::from([0xD7; 32]);
    let admin = PublicKey::from([0x0A; 32]);

    nest_for_test(&store, &parent, &child);
    CapabilitiesRepository::new(&store)
        .set_subgroup_visibility(&child, VisibilityMode::Restricted)
        .unwrap();

    // An admin of the parent group governs the space and must be able
    // to enumerate every child — even restricted ones it is not itself
    // a member of.
    MembershipRepository::new(&store)
        .add_member(&parent, &admin, GroupMemberRole::Admin)
        .unwrap();
    assert!(MembershipRepository::new(&store)
        .subgroup_visible_to(&parent, &child, Some(&admin))
        .unwrap());
}

#[test]
fn subgroup_visible_to_restricted_child_visible_to_its_member() {
    use calimero_context_config::VisibilityMode;

    let store = test_store();
    let parent = ContextGroupId::from([0xD8; 32]);
    let child = ContextGroupId::from([0xD9; 32]);
    let member = PublicKey::from([0x0B; 32]);

    nest_for_test(&store, &parent, &child);
    CapabilitiesRepository::new(&store)
        .set_subgroup_visibility(&child, VisibilityMode::Restricted)
        .unwrap();

    // A direct member of the restricted child sees it, even though it
    // is not an admin of the parent group.
    MembershipRepository::new(&store)
        .add_member(&child, &member, GroupMemberRole::Member)
        .unwrap();
    assert!(MembershipRepository::new(&store)
        .subgroup_visible_to(&parent, &child, Some(&member))
        .unwrap());
}

#[test]
fn is_authoritative_namespace_identity_recognizes_owner_admin_tee() {
    use calimero_context_client::local_governance::SignedGroupOp;
    use calimero_primitives::identity::PrivateKey;
    use rand::rngs::OsRng;

    let store = test_store();
    let mut rng = OsRng;
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
    MetaRepository::new(&store).save(&gid, &meta).unwrap();
    MembershipRepository::new(&store)
        .add_member(&gid, &admin_member, GroupMemberRole::Admin)
        .unwrap();
    MembershipRepository::new(&store)
        .add_member(&gid, &ordinary, GroupMemberRole::Member)
        .unwrap();

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

    assert!(MembershipRepository::new(&store)
        .is_authoritative_namespace_identity(namespace_id, &owner)
        .unwrap());
    assert!(MembershipRepository::new(&store)
        .is_authoritative_namespace_identity(namespace_id, &admin_member)
        .unwrap());
    assert!(MembershipRepository::new(&store)
        .is_authoritative_namespace_identity(namespace_id, &tee_node)
        .unwrap());
    assert!(!MembershipRepository::new(&store)
        .is_authoritative_namespace_identity(namespace_id, &ordinary)
        .unwrap());
    assert!(!MembershipRepository::new(&store)
        .is_authoritative_namespace_identity(namespace_id, &stranger)
        .unwrap());
}
