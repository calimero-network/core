//! Fold-equivalence: the unified-op **projection** must resolve the same
//! membership the **live** governance resolver does, across the open-subgroup
//! INHERITANCE lifecycle (join → inherit → remove-from-root revokes).
//!
//! This is the deterministic harness that drives the grant-direction fidelity
//! work (the e2e `group-remove-from-root-revokes-inherited` over-grant, reduced
//! to a unit test): one store is the live reference (`MembershipRepository` /
//! `check_path`); the same ops are folded into a `ScopeProjections` and read via
//! `member_at_cut`. Any divergence between live and projection fails here —
//! no CI roulette.

use std::sync::Arc;

use calimero_context::scope_projection::{op_from_namespace_op, ScopeProjections};
use calimero_context_client::local_governance::{
    EncryptedGroupOp, GroupOp, NamespaceOp, RootOp, SignedNamespaceOp,
};
use calimero_context_config::types::{
    ContextGroupId, GroupInvitationFromAdmin, SignedGroupOpenInvitation, SignerId,
};
use calimero_context_config::{MemberCapabilities, VisibilityMode};
use calimero_governance_store::{
    self, CapabilitiesRepository, MembershipRepository, MetaRepository, NamespaceRepository,
};
use calimero_primitives::context::GroupMemberRole;
use calimero_primitives::identity::{PrivateKey, PublicKey};
use calimero_storage::logical_clock::{HybridTimestamp, Timestamp, ID, NTP64};
use calimero_store::db::InMemoryDB;
use calimero_store::key::GroupMetaValue;
use calimero_store::Store;
use core::num::NonZeroU128;
use rand::rngs::OsRng;
use sha2::{Digest, Sha256};

/// A joiner credential. Note this test asserts the projection's fold matches the
/// LIVE membership resolver — and the projection now keys `MemberAdded` by
/// `cert.account` rather than a key-derived stand-in, so the account here is
/// load-bearing, not filler.
/// The joiner's credential, derived DETERMINISTICALLY from its signing key.
///
/// Deterministic because the op names the account this certifies and later
/// assertions have to name the same one; a fresh random root per call would make
/// every mention a different principal.
fn test_join_account_for(
    sign_pk: &calimero_primitives::identity::PublicKey,
) -> Box<calimero_context_client::local_governance::JoinAccountCredential> {
    calimero_context::test_support::credential(sign_pk)
}

fn store() -> Store {
    Store::new(Arc::new(InMemoryDB::owned()))
}

fn hlc(ns: u64) -> HybridTimestamp {
    HybridTimestamp::new(Timestamp::new(
        NTP64(ns),
        ID::from(NonZeroU128::new(1).unwrap()),
    ))
}

fn meta(admin: calimero_account::AccountId) -> GroupMetaValue {
    GroupMetaValue {
        app_key: [0xBB; 32],
        target_application_id: calimero_primitives::application::ApplicationId::from([0xCC; 32]),
        upgrade_policy: calimero_primitives::context::UpgradePolicy::Automatic,
        created_at: 1_700_000_000,
        admin_identity: admin,
        owner_identity: admin,
        migration: None,
        auto_join: true,
    }
}

/// `nonce` identifies the invitation. It is a parameter rather than a constant
/// because an invitation is spent once an identity joins with it: presenting the
/// same one again after they exit cannot readmit them, so a re-invite has to
/// carry a fresh nonce. Real invitations get a random one per issue.
fn sign_invitation(
    admin_sk: &PrivateKey,
    group: ContextGroupId,
    role: u8,
    nonce: [u8; 32],
) -> SignedGroupOpenInvitation {
    let invitation = GroupInvitationFromAdmin {
        inviter_identity: SignerId::from(*admin_sk.public_key().digest()),
        group_id: group,
        // 0 is the canonical "no expiry" sentinel — MemberJoined carries no
        // joined_at, so any non-zero expiration causes the apply gate to reject it.
        expiration_timestamp: 0,
        invitation_nonce: nonce,
        invited_role: role,
    };
    let inv_bytes = borsh::to_vec(&invitation).expect("borsh invitation");
    let inv_sig = admin_sk
        .sign(&Sha256::digest(&inv_bytes))
        .expect("sign invitation");
    SignedGroupOpenInvitation {
        inviter_account: None,
        invitation,
        inviter_signature: hex::encode(inv_sig.to_bytes()),
        application_id: None,
        app_key: None,
    }
}

/// Fold the structural ops the projection needs for the inheritance walk — the
/// subgroup TREE (`GroupCreated`) and its OPEN visibility (`SubgroupVisibilitySet`)
/// — into `proj`, chained from `prev` (genesis when `None`). In production these
/// are emitted as governance ops; this mirrors that so the projection's
/// `subgroups` map is populated (live gets the same state via repo writes in the
/// test setup). Returns the id of the last folded op (to chain the next from).
fn fold_subgroup_structure(
    proj: &mut ScopeProjections,
    namespace: [u8; 32],
    admin: PublicKey,
    subgroup: ContextGroupId,
    created_id: [u8; 32],
    visibility_id: [u8; 32],
) -> [u8; 32] {
    let created = SignedNamespaceOp {
        version: 1,
        namespace_id: namespace.into(),
        parent_op_hashes: Vec::new(),
        signer: admin,
        nonce: 0,
        op: NamespaceOp::Root(RootOp::GroupCreated {
            group_id: subgroup.to_bytes().into(),
            parent_id: namespace.into(),
            restricted: true,
        }),
        signature: [0u8; 64],
    };
    proj.ingest_op(&op_from_namespace_op(
        &created,
        None,
        created_id,
        hlc(0),
        &[],
    ));
    let vis = ns_group_envelope(namespace, admin, subgroup);
    proj.ingest_op(&op_from_namespace_op(
        &vis,
        Some(&GroupOp::SubgroupVisibilitySet {
            mode: calimero_context_config::VisibilityMode::Open,
        }),
        visibility_id,
        hlc(0),
        &[created_id],
    ));
    visibility_id
}

/// A `NamespaceOp::Group` envelope for folding; the cleartext op is supplied
/// separately to `op_from_namespace_op` (the projection decrypts post-apply).
fn ns_group_envelope(
    namespace: [u8; 32],
    signer: PublicKey,
    group: ContextGroupId,
) -> SignedNamespaceOp {
    SignedNamespaceOp {
        version: 1,
        namespace_id: namespace.into(),
        parent_op_hashes: Vec::new(),
        signer,
        nonce: 0,
        op: NamespaceOp::Group {
            group_id: group.to_bytes().into(),
            key_id: [0u8; 32].into(),
            encrypted: EncryptedGroupOp {
                nonce: [0u8; 12],
                ciphertext: Vec::new(),
            },
            key_rotation: None,
        },
        signature: [0u8; 64],
    }
}

/// The scenario reduced to a unit test: an OPEN subgroup, a member who inherits
/// access from the namespace root (`CAN_JOIN_OPEN_SUBGROUPS`) via
/// `MemberJoinedOpen` (no persistent direct row), then is removed from the root.
/// Live revokes the subgroup access (no anchor); the projection must too.
#[test]
fn projection_matches_live_across_inherited_join_and_root_removal() {
    let store = store();
    let admin_sk = PrivateKey::random(&mut OsRng);
    let admin = admin_sk.public_key();
    let joiner_sk = PrivateKey::random(&mut OsRng);
    let joiner = joiner_sk.public_key();

    let ns = ContextGroupId::from([0x11; 32]);
    let subgroup = ContextGroupId::from([0x22; 32]);

    // Genesis base state (store seeds, NOT ops — exactly as create_group writes):
    // root + subgroup meta/admin, the subgroup nested + Open, root default cap.
    // Enrolled at the ROOT: bindings live at the anchor and every reader
    // resolves up to it, so a row written against the subgroup would be
    // invisible the moment it is nested.
    let admin_account = calimero_context::test_support::enrol(&store, &ns, &admin);
    for g in [&ns, &subgroup] {
        MetaRepository::new(&store)
            .save(g, &meta(admin_account))
            .unwrap();
        MembershipRepository::new(&store)
            .add_member(g, &admin_account, GroupMemberRole::Admin)
            .unwrap();
    }
    NamespaceRepository::new(&store)
        .nest(&ns, &subgroup)
        .unwrap();
    CapabilitiesRepository::new(&store)
        .set_subgroup_visibility(&subgroup, VisibilityMode::Open)
        .unwrap();
    CapabilitiesRepository::new(&store)
        .set_default_capabilities(&ns, MemberCapabilities::CAN_JOIN_OPEN_SUBGROUPS.bits())
        .unwrap();

    let mut proj = ScopeProjections::new();
    // Fold the subgroup tree + Open visibility so the inheritance walk can
    // traverse subgroup → root (the structural ops the projection needs).
    let s2 = fold_subgroup_structure(
        &mut proj,
        ns.to_bytes(),
        admin,
        subgroup,
        [0xA0; 32],
        [0xAF; 32],
    );

    // (1) joiner joins the namespace root via invitation — a DIRECT membership.
    let join_ns = SignedNamespaceOp::sign(
        &joiner_sk,
        ns.to_bytes().into(),
        vec![],
        1,
        NamespaceOp::Root(RootOp::MemberJoined {
            member: calimero_context::test_support::account_for(&joiner),
            signed_invitation: sign_invitation(&admin_sk, ns, 1, [0x42; 32]),
            account: test_join_account_for(&joiner),
        }),
    )
    .expect("sign join_ns");
    calimero_governance_store::apply_signed_namespace_op(&store, &join_ns).unwrap();
    let id1 = [0xA1; 32];
    proj.ingest_op(&op_from_namespace_op(&join_ns, None, id1, hlc(1), &[s2]));

    // (2) joiner joins the OPEN subgroup via inheritance (MemberJoinedOpen) —
    // live writes NO direct row; membership is re-derived from the anchor.
    let join_sub = SignedNamespaceOp::sign(
        &joiner_sk,
        ns.to_bytes().into(),
        vec![],
        2,
        NamespaceOp::Root(RootOp::MemberJoinedOpen {
            member: calimero_context::test_support::account_for(&joiner),
            group_id: subgroup.to_bytes().into(),
            account: test_join_account_for(&joiner),
        }),
    )
    .expect("sign join_sub");
    calimero_governance_store::apply_signed_namespace_op(&store, &join_sub).unwrap();
    let id2 = [0xA2; 32];
    proj.ingest_op(&op_from_namespace_op(&join_sub, None, id2, hlc(2), &[id1]));

    // After the joins: both authorities must see the joiner in the subgroup
    // (live by inheritance walk; projection likewise).
    let live_member_after_join = MembershipRepository::new(&store)
        .is_member(
            &subgroup,
            &calimero_context::test_support::account_for(&joiner),
        )
        .unwrap();
    assert!(
        live_member_after_join,
        "live: joiner inherits subgroup access"
    );
    // This `Some(true)` is driven by the at-cut FOLD, not the materialized
    // fallback: `MemberJoinedOpen` writes no direct subgroup row, so
    // `member_at_cut`'s `role_of(subgroup, joiner)` fallback finds nothing and the
    // inheritance walk over the folded ancestry is what must resolve membership.
    assert_eq!(
        proj.member_at_cut(&store, subgroup, &joiner, &[id2]),
        Some(true),
        "projection must agree the joiner is a member after the inherited join"
    );
    // The GRANT resolver (sole-authority path) must also see the member: complete
    // ancestry is folded, so it returns the at-cut verdict.
    assert_eq!(
        proj.member_at_cut_authoritative(&store, subgroup, &joiner, &[id2]),
        Some(true),
        "authoritative grant resolver agrees the joiner is a member after the join"
    );

    // (3) admin removes the joiner from the NAMESPACE ROOT only (a GroupOp on the
    // root). Live: removes the root row; the subgroup has no direct row, so the
    // inheritance walk now finds no anchor → revoked.
    //
    // The store side uses the repo write directly rather than
    // `apply_signed_namespace_op`: a `MemberRemoved` is an ENCRYPTED `GroupOp`, and
    // round-tripping it through the signed-apply path needs a real per-group key +
    // ciphertext this harness doesn't model (the joins above are cleartext
    // `RootOp`s, hence applied through the signed path). The equivalence under test
    // is fold-vs-materialized-membership; `remove_member` yields the same
    // `is_member` result a real node's apply would, which is all `is_member` reads.
    let removal = GroupOp::MemberRemoved {
        member: calimero_context::test_support::account_for(&joiner),
        expected_group_state_hash: [0u8; 32],
        expected_context_state_hashes: Vec::new(),
    };
    MembershipRepository::new(&store)
        .remove_member(&ns, &calimero_context::test_support::account_for(&joiner))
        .unwrap();
    let id3 = [0xA3; 32];
    let removal_env = ns_group_envelope(ns.to_bytes(), admin, ns);
    proj.ingest_op(&op_from_namespace_op(
        &removal_env,
        Some(&removal),
        id3,
        hlc(3),
        &[id2],
    ));

    // THE equivalence: after root removal, live revokes the inherited subgroup
    // access; the projection must NOT keep granting it (the over-grant).
    let live_member_after_removal = MembershipRepository::new(&store)
        .is_member(
            &subgroup,
            &calimero_context::test_support::account_for(&joiner),
        )
        .unwrap();
    assert!(
        !live_member_after_removal,
        "live: removing from root revokes inherited subgroup access"
    );
    assert_eq!(
        proj.member_at_cut(&store, subgroup, &joiner, &[id3]),
        Some(false),
        "projection must revoke the inherited subgroup access after root removal \
         (matching live) — granting here is the over-grant"
    );
    // The GRANT resolver MUST NOT grant where live rejects — this is the
    // sole-authority safety property (it can never over-authorize).
    assert_ne!(
        proj.member_at_cut_authoritative(&store, subgroup, &joiner, &[id3]),
        Some(true),
        "authoritative grant resolver must NOT grant a write live rejected (over-grant)"
    );
}

/// Symmetric guard for the UNDER-grant: a member who leaves the namespace and
/// then REJOINS must regain inherited subgroup access — both in live and in the
/// projection. This isolates walk-logic correctness (all ops folded here) from
/// the e2e feed-completeness gap that previously broke this case.
#[test]
fn projection_matches_live_across_leave_and_rejoin_inheritance() {
    let store = store();
    let admin_sk = PrivateKey::random(&mut OsRng);
    let admin = admin_sk.public_key();
    let joiner_sk = PrivateKey::random(&mut OsRng);
    let joiner = joiner_sk.public_key();

    let ns = ContextGroupId::from([0x31; 32]);
    let subgroup = ContextGroupId::from([0x32; 32]);

    // Enrolled at the ROOT: bindings live at the anchor and every reader
    // resolves up to it, so a row written against the subgroup would be
    // invisible the moment it is nested.
    let admin_account = calimero_context::test_support::enrol(&store, &ns, &admin);
    for g in [&ns, &subgroup] {
        MetaRepository::new(&store)
            .save(g, &meta(admin_account))
            .unwrap();
        MembershipRepository::new(&store)
            .add_member(g, &admin_account, GroupMemberRole::Admin)
            .unwrap();
    }
    NamespaceRepository::new(&store)
        .nest(&ns, &subgroup)
        .unwrap();
    CapabilitiesRepository::new(&store)
        .set_subgroup_visibility(&subgroup, VisibilityMode::Open)
        .unwrap();
    CapabilitiesRepository::new(&store)
        .set_default_capabilities(&ns, MemberCapabilities::CAN_JOIN_OPEN_SUBGROUPS.bits())
        .unwrap();

    let mut proj = ScopeProjections::new();
    let s2 = fold_subgroup_structure(
        &mut proj,
        ns.to_bytes(),
        admin,
        subgroup,
        [0xB0; 32],
        [0xBF; 32],
    );

    // join ns (nonce 1) + inherit subgroup (nonce 2).
    let join_ns = SignedNamespaceOp::sign(
        &joiner_sk,
        ns.to_bytes().into(),
        vec![],
        1,
        NamespaceOp::Root(RootOp::MemberJoined {
            member: calimero_context::test_support::account_for(&joiner),
            signed_invitation: sign_invitation(&admin_sk, ns, 1, [0x42; 32]),
            account: test_join_account_for(&joiner),
        }),
    )
    .unwrap();
    calimero_governance_store::apply_signed_namespace_op(&store, &join_ns).unwrap();
    proj.ingest_op(&op_from_namespace_op(
        &join_ns,
        None,
        [0xB1; 32],
        hlc(1),
        &[s2],
    ));

    let join_sub = SignedNamespaceOp::sign(
        &joiner_sk,
        ns.to_bytes().into(),
        vec![],
        2,
        NamespaceOp::Root(RootOp::MemberJoinedOpen {
            member: calimero_context::test_support::account_for(&joiner),
            group_id: subgroup.to_bytes().into(),
            account: test_join_account_for(&joiner),
        }),
    )
    .unwrap();
    calimero_governance_store::apply_signed_namespace_op(&store, &join_sub).unwrap();
    proj.ingest_op(&op_from_namespace_op(
        &join_sub,
        None,
        [0xB2; 32],
        hlc(2),
        &[[0xB1; 32]],
    ));

    // leave ns: remove the root row (GroupOp on root, folded).
    //
    // The live side drops the row with a direct `remove_member` write rather than
    // by applying the op, so no re-entry block is recorded — which is why the
    // rejoin below is admitted at all. That is deliberate: this test is about
    // fold equivalence between the projection and the live resolver, not about
    // re-entry policy. Run the removal through `MemberRemoved` apply and the
    // rejoin would be rejected outright, no invitation able to readmit them;
    // that path is covered in `governance-store`.
    let leave = GroupOp::MemberRemoved {
        member: calimero_context::test_support::account_for(&joiner),
        expected_group_state_hash: [0u8; 32],
        expected_context_state_hashes: Vec::new(),
    };
    MembershipRepository::new(&store)
        .remove_member(&ns, &calimero_context::test_support::account_for(&joiner))
        .unwrap();
    proj.ingest_op(&op_from_namespace_op(
        &ns_group_envelope(ns.to_bytes(), admin, ns),
        Some(&leave),
        [0xB3; 32],
        hlc(3),
        &[[0xB2; 32]],
    ));
    // After leaving: not a member (both).
    assert!(!MembershipRepository::new(&store)
        .is_member(
            &subgroup,
            &calimero_context::test_support::account_for(&joiner)
        )
        .unwrap());
    assert_eq!(
        proj.member_at_cut(&store, subgroup, &joiner, &[[0xB3; 32]]),
        Some(false)
    );

    // REJOIN ns via a FRESHLY ISSUED invitation (direct root membership again).
    // The nonce differs from the one they joined with above, and it has to: an
    // invitation is spent for the identity that used it, so presenting the same
    // one again after exiting cannot readmit them. Coming back means being
    // re-invited.
    let rejoin_ns = SignedNamespaceOp::sign(
        &joiner_sk,
        ns.to_bytes().into(),
        vec![],
        3,
        NamespaceOp::Root(RootOp::MemberJoined {
            member: calimero_context::test_support::account_for(&joiner),
            signed_invitation: sign_invitation(&admin_sk, ns, 1, [0x43; 32]),
            account: test_join_account_for(&joiner),
        }),
    )
    .unwrap();
    calimero_governance_store::apply_signed_namespace_op(&store, &rejoin_ns).unwrap();
    proj.ingest_op(&op_from_namespace_op(
        &rejoin_ns,
        None,
        [0xB4; 32],
        hlc(4),
        &[[0xB3; 32]],
    ));

    // After rejoin: inherited subgroup access is restored — both authorities.
    let live = MembershipRepository::new(&store)
        .is_member(
            &subgroup,
            &calimero_context::test_support::account_for(&joiner),
        )
        .unwrap();
    assert!(
        live,
        "live: rejoining the root restores inherited subgroup access"
    );
    assert_eq!(
        proj.member_at_cut(&store, subgroup, &joiner, &[[0xB4; 32]]),
        Some(true),
        "projection must restore inherited access on rejoin (the under-grant guard)"
    );
    // The GRANT resolver also restores access on rejoin (complete ancestry folded).
    assert_eq!(
        proj.member_at_cut_authoritative(&store, subgroup, &joiner, &[[0xB4; 32]]),
        Some(true),
        "authoritative grant resolver restores inherited access on rejoin"
    );
}

/// Backfill-lag deferral: when the cut's ancestry is only PARTIALLY folded — the
/// write arrived before a proactive governance backfill folded the author's
/// membership chain — the deny co-authorizer must DEFER to live (`None`), not
/// reject (`Some(false)`). An inherited open-subgroup membership is the exposed
/// case: deriving it needs the whole chain folded (anchor membership + subgroup
/// edge + visibility + cap), so a truncated fold reads not-a-member. Live (with
/// its materialized rows) still authorizes; the projection must not contradict it
/// on a partial view. This reproduces the single transient divergence the e2e
/// cutover gate caught in `group-leave-then-rejoin-via-inheritance` (one marker,
/// emitted mid-backfill, gone once the ancestry completed).
#[test]
fn projection_defers_when_cut_ancestry_incomplete() {
    let store = store();
    let admin_sk = PrivateKey::random(&mut OsRng);
    let admin = admin_sk.public_key();
    let joiner_sk = PrivateKey::random(&mut OsRng);
    let joiner = joiner_sk.public_key();

    let ns = ContextGroupId::from([0x41; 32]);
    let subgroup = ContextGroupId::from([0x42; 32]);

    // Enrolled at the ROOT: bindings live at the anchor and every reader
    // resolves up to it, so a row written against the subgroup would be
    // invisible the moment it is nested.
    let admin_account = calimero_context::test_support::enrol(&store, &ns, &admin);
    for g in [&ns, &subgroup] {
        MetaRepository::new(&store)
            .save(g, &meta(admin_account))
            .unwrap();
        MembershipRepository::new(&store)
            .add_member(g, &admin_account, GroupMemberRole::Admin)
            .unwrap();
    }
    NamespaceRepository::new(&store)
        .nest(&ns, &subgroup)
        .unwrap();
    CapabilitiesRepository::new(&store)
        .set_subgroup_visibility(&subgroup, VisibilityMode::Open)
        .unwrap();
    CapabilitiesRepository::new(&store)
        .set_default_capabilities(&ns, MemberCapabilities::CAN_JOIN_OPEN_SUBGROUPS.bits())
        .unwrap();

    // LIVE applies the full chain — root join + inherited subgroup join — so the
    // live resolver authoritatively sees the joiner as an inherited member.
    let join_ns = SignedNamespaceOp::sign(
        &joiner_sk,
        ns.to_bytes().into(),
        vec![],
        1,
        NamespaceOp::Root(RootOp::MemberJoined {
            member: calimero_context::test_support::account_for(&joiner),
            signed_invitation: sign_invitation(&admin_sk, ns, 1, [0x42; 32]),
            account: test_join_account_for(&joiner),
        }),
    )
    .unwrap();
    calimero_governance_store::apply_signed_namespace_op(&store, &join_ns).unwrap();
    let join_sub = SignedNamespaceOp::sign(
        &joiner_sk,
        ns.to_bytes().into(),
        vec![],
        2,
        NamespaceOp::Root(RootOp::MemberJoinedOpen {
            member: calimero_context::test_support::account_for(&joiner),
            group_id: subgroup.to_bytes().into(),
            account: test_join_account_for(&joiner),
        }),
    )
    .unwrap();
    calimero_governance_store::apply_signed_namespace_op(&store, &join_sub).unwrap();
    assert!(
        MembershipRepository::new(&store)
            .is_member(
                &subgroup,
                &calimero_context::test_support::account_for(&joiner)
            )
            .unwrap(),
        "live: joiner inherits subgroup access"
    );

    // The PROJECTION has only folded the subgroup structure + the rejoin op
    // itself, NOT the joiner's root-membership ancestor (`[0xC1; 32]` is never
    // ingested) — exactly the mid-backfill state on the node that caught the
    // divergence. The cited head `[0xC2; 32]` is present, but its ancestry is
    // truncated, so the inheritance walk can find no anchor membership.
    let mut proj = ScopeProjections::new();
    fold_subgroup_structure(
        &mut proj,
        ns.to_bytes(),
        admin,
        subgroup,
        [0xC0; 32],
        [0xCF; 32],
    );
    proj.ingest_op(&op_from_namespace_op(
        &join_sub,
        None,
        [0xC2; 32],
        hlc(2),
        &[[0xC1; 32]], // parent (the root join) deliberately NOT ingested
    ));

    // Pre-fix this returned `Some(false)` (the walk fails + no direct row for the
    // materialized fallback) → a false deny that tripped the cutover gate. The
    // completeness guard makes it abstain instead.
    assert_eq!(
        proj.member_at_cut(&store, subgroup, &joiner, &[[0xC2; 32]]),
        None,
        "projection must DEFER to live (None) on a partially-folded cut, not deny"
    );
}

/// The governance-pending drain authorizes a buffered delta with
/// `member_at_cut_authoritative` against the in-memory projection. That
/// projection can lag the durable op-store: the apply path folds an op into the
/// projection only on a real `Applied`, not on the "already applied" dedup path,
/// and ops arriving via the namespace-governance backfill pull take that dedup
/// path. So the author's membership op can be durably present in the op-store
/// yet absent from the projection — the drain then reads `None` ("cut ancestry
/// not fully folded") on every pass and eventually drops a valid delta.
///
/// The fix refreshes the projection from the op-store at the cut before
/// authorizing (`refresh_projection_for_cut`, the same step the gossip path runs
/// via `resolve_cut_membership`). This test pins the mechanism that refresh
/// relies on: with the author's root-membership ancestor missing from the
/// projection the authoritative resolver abstains, and folding that ancestor —
/// exactly what a refresh-from-store does — flips the verdict to a grant.
#[test]
fn refreshing_the_missing_ancestor_unblocks_the_authoritative_grant() {
    let store = store();
    let admin_sk = PrivateKey::random(&mut OsRng);
    let admin = admin_sk.public_key();
    let joiner_sk = PrivateKey::random(&mut OsRng);
    let joiner = joiner_sk.public_key();

    let ns = ContextGroupId::from([0x51; 32]);
    let subgroup = ContextGroupId::from([0x52; 32]);

    // Genesis base state (store seeds, as `create_group` writes).
    // Enrolled at the ROOT: bindings live at the anchor and every reader
    // resolves up to it, so a row written against the subgroup would be
    // invisible the moment it is nested.
    let admin_account = calimero_context::test_support::enrol(&store, &ns, &admin);
    for g in [&ns, &subgroup] {
        MetaRepository::new(&store)
            .save(g, &meta(admin_account))
            .unwrap();
        MembershipRepository::new(&store)
            .add_member(g, &admin_account, GroupMemberRole::Admin)
            .unwrap();
    }
    NamespaceRepository::new(&store)
        .nest(&ns, &subgroup)
        .unwrap();
    CapabilitiesRepository::new(&store)
        .set_subgroup_visibility(&subgroup, VisibilityMode::Open)
        .unwrap();
    CapabilitiesRepository::new(&store)
        .set_default_capabilities(&ns, MemberCapabilities::CAN_JOIN_OPEN_SUBGROUPS.bits())
        .unwrap();

    // Both join ops are durably applied to the op-store — the state a node holds
    // after the backfill pull delivered them.
    let join_ns = SignedNamespaceOp::sign(
        &joiner_sk,
        ns.to_bytes().into(),
        vec![],
        1,
        NamespaceOp::Root(RootOp::MemberJoined {
            member: calimero_context::test_support::account_for(&joiner),
            signed_invitation: sign_invitation(&admin_sk, ns, 1, [0x42; 32]),
            account: test_join_account_for(&joiner),
        }),
    )
    .unwrap();
    calimero_governance_store::apply_signed_namespace_op(&store, &join_ns).unwrap();
    let join_sub = SignedNamespaceOp::sign(
        &joiner_sk,
        ns.to_bytes().into(),
        vec![],
        2,
        NamespaceOp::Root(RootOp::MemberJoinedOpen {
            member: calimero_context::test_support::account_for(&joiner),
            group_id: subgroup.to_bytes().into(),
            account: test_join_account_for(&joiner),
        }),
    )
    .unwrap();
    calimero_governance_store::apply_signed_namespace_op(&store, &join_sub).unwrap();

    let id_root_join = [0x5A; 32];
    let id_sub_join = [0x5B; 32];

    // Stale projection: the subgroup structure and the joiner's subgroup-join are
    // folded, but the joiner's ROOT-membership ancestor (`id_root_join`) is not —
    // the drain's view when the backfill applied the ops via the dedup path.
    let mut proj = ScopeProjections::new();
    let s2 = fold_subgroup_structure(
        &mut proj,
        ns.to_bytes(),
        admin,
        subgroup,
        [0x50; 32],
        [0x5F; 32],
    );
    proj.ingest_op(&op_from_namespace_op(
        &join_sub,
        None,
        id_sub_join,
        hlc(2),
        &[id_root_join], // parent is the root join — deliberately not folded yet
    ));

    // The drain's authorization fails: the cut's ancestry is incomplete, so it
    // abstains and re-buffers — even though `join_ns` is durable in the store.
    assert_eq!(
        proj.member_at_cut_authoritative(&store, subgroup, &joiner, &[id_sub_join]),
        None,
        "stale projection: authoritative resolver must abstain while the ancestor is unfolded"
    );

    // Refresh folds the missing ancestor from the op-store (what
    // `refresh_projection_for_cut` does before the drain authorizes).
    proj.ingest_op(&op_from_namespace_op(
        &join_ns,
        None,
        id_root_join,
        hlc(1),
        &[s2],
    ));

    // Now the cut's ancestry is complete and the inherited membership resolves —
    // the drain authorizes and re-applies the delta instead of dropping it.
    assert_eq!(
        proj.member_at_cut_authoritative(&store, subgroup, &joiner, &[id_sub_join]),
        Some(true),
        "after refreshing the durable ancestor, the authoritative resolver must grant"
    );
}

/// A credential that VERIFIES and is certified for `sign_pk`, so folding it
/// actually inserts a device. The filler fixture above cannot: `fold_device_link`
/// checks the certificate, so a `[0x11; 64]` signature is dropped before any
/// device lands.
fn real_join_account_for(
    sign_pk: &PublicKey,
    device: [u8; 32],
) -> Box<calimero_context_client::local_governance::JoinAccountCredential> {
    let root_sk = PrivateKey::random(&mut OsRng);
    let genesis = calimero_account::AccountGenesis::new(root_sk.public_key(), [0x5A; 16]);
    let cert = calimero_account::sign_device_cert(
        &root_sk,
        genesis.account_id(),
        calimero_account::DeviceId::from(device),
        sign_pk,
        &calimero_account::KemPublicKey::from([0x2B; 32]),
        0,
        0,
    )
    .expect("sign the device cert");
    Box::new(
        calimero_context_client::local_governance::JoinAccountCredential {
            genesis,
            chain: vec![],
            cert,
        },
    )
}

/// Folding a joiner's DEVICE must not disturb who is a MEMBER.
///
/// The dm-subgroup-privacy shape: the namespace admin holds no direct row in an
/// Open subgroup and reaches it purely by the inheritance walk, while a joiner in
/// the same namespace carries a credential that folds. If the device half
/// perturbs the membership half — through `account_for_author`'s precedence, the
/// enumeration, or the walk — the admin stops resolving as a member and every
/// projection-backed membership read for that subgroup denies.
#[test]
fn a_folded_join_device_does_not_hide_an_inherited_admin() {
    let store = store();
    let admin_sk = PrivateKey::random(&mut OsRng);
    let admin = admin_sk.public_key();
    let joiner_sk = PrivateKey::random(&mut OsRng);
    let joiner = joiner_sk.public_key();

    let ns = ContextGroupId::from([0x31; 32]);
    let subgroup = ContextGroupId::from([0x32; 32]);

    // The admin is a direct member of the ROOT ONLY — its subgroup access is
    // inherited, exactly like the namespace owner in the scenario.
    // Keyed by the KEY-derived account throughout this test, unlike the suites
    // above. Its whole premise is that the admin has no folded presence, so the
    // projection can only reach it through the out-of-band root — and that path
    // compares the ROOT's `admin_identity` against what a key resolves to with
    // nothing folded, which is this derivation. An enrolment-derived account
    // would be unreachable there, and the test would fail for its setup rather
    // than for the behaviour it guards.
    MetaRepository::new(&store)
        .save(&ns, &meta(calimero_op_adapter::legacy_account_id(&admin)))
        .unwrap();
    MetaRepository::new(&store)
        .save(
            &subgroup,
            &meta(calimero_op_adapter::legacy_account_id(&admin)),
        )
        .unwrap();
    MembershipRepository::new(&store)
        .add_member(
            &ns,
            &calimero_op_adapter::legacy_account_id(&admin),
            GroupMemberRole::Admin,
        )
        .unwrap();
    NamespaceRepository::new(&store)
        .nest(&ns, &subgroup)
        .unwrap();
    CapabilitiesRepository::new(&store)
        .set_subgroup_visibility(&subgroup, VisibilityMode::Open)
        .unwrap();
    CapabilitiesRepository::new(&store)
        .set_default_capabilities(&ns, MemberCapabilities::CAN_JOIN_OPEN_SUBGROUPS.bits())
        .unwrap();

    let mut proj = ScopeProjections::new();
    // The SUBGROUP IS CREATED BY THE JOINER, not the admin — as in the scenario,
    // where node-2 creates the Open subgroup. That matters: the creator becomes
    // the folded `group_admin`, so the namespace admin ends up with NO folded
    // presence at all and is reachable only through the out-of-band `root`.
    let s2 = fold_subgroup_structure(
        &mut proj,
        ns.to_bytes(),
        joiner,
        subgroup,
        [0xB0; 32],
        [0xBF; 32],
    );

    // Baseline: before any join folds, the admin inherits into the subgroup.
    assert_eq!(
        proj.member_at_cut(&store, subgroup, &admin, &[s2]),
        Some(true),
        "baseline: the root admin reaches an Open child by inheritance"
    );

    // The ADMIN's OWN device folds — the case that matters. The genesis admin has
    // no membership OP anywhere: it is seeded as a store row and reaches the
    // folded view only through the out-of-band `root` parameter, keyed by
    // `legacy_account_id`. So it is exactly the principal `account_for_author`'s
    // own precedence guard cannot see.
    let admin_join_open = SignedNamespaceOp::sign(
        &admin_sk,
        ns.to_bytes().into(),
        vec![],
        7,
        NamespaceOp::Root(RootOp::MemberJoinedOpen {
            member: calimero_op_adapter::legacy_account_id(&admin),
            group_id: subgroup.to_bytes().into(),
            account: real_join_account_for(&admin, [0x7A; 32]),
        }),
    )
    .expect("sign admin open-join");
    let id0 = [0xB7; 32];
    proj.ingest_op(&op_from_namespace_op(
        &admin_join_open,
        None,
        id0,
        hlc(1),
        &[s2],
    ));

    assert_eq!(
        proj.member_at_cut(&store, subgroup, &admin, &[id0]),
        Some(true),
        "folding the admin's OWN device must not un-member it: membership on this \
         plane is keyed by the stand-in, and the genesis admin is known only \
         through the out-of-band root"
    );

    // The joiner joins the namespace carrying a credential that really folds.
    let join_ns = SignedNamespaceOp::sign(
        &joiner_sk,
        ns.to_bytes().into(),
        vec![],
        1,
        NamespaceOp::Root(RootOp::MemberJoined {
            member: calimero_context::test_support::account_for(&joiner),
            signed_invitation: sign_invitation(&admin_sk, ns, 1, [0x42; 32]),
            account: real_join_account_for(&joiner, [0x3E; 32]),
        }),
    )
    .expect("sign join_ns");
    let id1 = [0xB1; 32];
    proj.ingest_op(&op_from_namespace_op(&join_ns, None, id1, hlc(1), &[s2]));

    // ...and then into the Open subgroup by inheritance, credential and all.
    let join_sub = SignedNamespaceOp::sign(
        &joiner_sk,
        ns.to_bytes().into(),
        vec![],
        2,
        NamespaceOp::Root(RootOp::MemberJoinedOpen {
            member: calimero_context::test_support::account_for(&joiner),
            group_id: subgroup.to_bytes().into(),
            account: real_join_account_for(&joiner, [0x3F; 32]),
        }),
    )
    .expect("sign join_sub");
    let id2 = [0xB2; 32];
    proj.ingest_op(&op_from_namespace_op(&join_sub, None, id2, hlc(2), &[id1]));

    assert_eq!(
        proj.member_at_cut(&store, subgroup, &admin, &[id2]),
        Some(true),
        "the admin's inherited membership must survive another member's device \
         folding — this is what dm-subgroup-privacy waits on"
    );
    assert_eq!(
        proj.member_at_cut(&store, ns, &admin, &[id2]),
        Some(true),
        "and its direct root membership too"
    );
}
