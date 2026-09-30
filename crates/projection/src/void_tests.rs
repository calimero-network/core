//! A removal voids the concurrent ops of the account it removes.

use calimero_account::AccountId;
use calimero_authz::AclView;
use calimero_context_config::types::ContextGroupId;
use calimero_context_config::MemberCapabilities;
use calimero_op::{Authorship, Op, OpPayload, ScopeId};
use calimero_primitives::context::GroupMemberRole;
use calimero_storage::logical_clock::{HybridTimestamp, Timestamp, ID, NTP64};
use core::num::NonZeroU128;
use std::collections::BTreeSet;

use crate::{AuthorityBase, ScopeState};

const OWNER: u8 = 0x10;
const ALICE: u8 = 0x11;
const SAM: u8 = 0x12;
const BOB: u8 = 0x13;
const XAVIER: u8 = 0x14;
const YARA: u8 = 0x15;
const ZED: u8 = 0x16;
const WILL: u8 = 0x17;

fn acct(n: u8) -> AccountId {
    AccountId::from([n; 32])
}

fn group() -> ContextGroupId {
    ContextGroupId::from([3u8; 32])
}

fn base() -> AuthorityBase {
    AuthorityBase {
        root: Some((group(), acct(OWNER))),
        default_cap_base: 0,
    }
}

fn authorship(n: u8) -> Authorship {
    Authorship {
        account: acct(n),
        device: calimero_account::DeviceId::from([n; 32]),
        device_key: calimero_primitives::identity::PublicKey::from([n; 32]),
    }
}

fn hlc() -> HybridTimestamp {
    HybridTimestamp::new(Timestamp::new(
        NTP64(0),
        ID::from(NonZeroU128::new(1).unwrap()),
    ))
}

/// A governance op by `author` citing `parents`, as the bridge builds them.
fn gov(author: u8, parents: &[&Op], payload: OpPayload) -> Op {
    Op::new(
        ScopeId::from([0u8; 32]),
        parents.iter().map(|p| p.id()).collect(),
        authorship(author),
        hlc(),
        payload,
        [0u8; 32],
        [0u8; 64],
    )
}

fn add(author: u8, parents: &[&Op], member: u8, role: GroupMemberRole) -> Op {
    add_in(group(), author, parents, member, role)
}

fn add_in(
    group: ContextGroupId,
    author: u8,
    parents: &[&Op],
    member: u8,
    role: GroupMemberRole,
) -> Op {
    gov(
        author,
        parents,
        OpPayload::MemberAdded {
            group,
            member: acct(member),
            role,
        },
    )
}

fn subgroup(n: u8) -> ContextGroupId {
    ContextGroupId::from([n; 32])
}

/// The owner creates subgroup `n` under `group()`, and Sam is its admin.
fn create_subgroup(parents: &[&Op], n: u8) -> Op {
    gov(
        OWNER,
        parents,
        OpPayload::SubgroupCreated {
            child: ScopeId::from([n; 32]),
            parent: ScopeId::from(group().to_bytes()),
            restricted: false,
            admin: acct(SAM),
        },
    )
}

fn remove(author: u8, parents: &[&Op], member: u8) -> Op {
    gov(
        author,
        parents,
        OpPayload::MemberRemoved {
            group: group(),
            member: acct(member),
        },
    )
}

fn grant(author: u8, parents: &[&Op], member: u8, capabilities: MemberCapabilities) -> Op {
    gov(
        author,
        parents,
        OpPayload::MemberCapabilitySet {
            group: group(),
            member: acct(member),
            capabilities,
        },
    )
}

/// Owner adds Alice, Sam and Bob as admins, one op after another.
fn admins() -> Vec<Op> {
    let a = add(OWNER, &[], ALICE, GroupMemberRole::Admin);
    let s = add(OWNER, &[&a], SAM, GroupMemberRole::Admin);
    let b = add(OWNER, &[&s], BOB, GroupMemberRole::Admin);
    vec![a, s, b]
}

fn ids(ops: &[&Op]) -> Vec<[u8; 32]> {
    ops.iter().map(|o| o.id()).collect()
}

fn view(log: &[Op], heads: &[&Op]) -> AclView {
    ScopeState::acl_view_at_with_base(log, &ids(heads), base())
}

fn void(log: &[Op]) -> BTreeSet<[u8; 32]> {
    ScopeState::void_ops(log, base())
}

fn is_member(view: &AclView, who: u8) -> bool {
    view.groups
        .get(&group())
        .is_some_and(|members| members.contains_key(&acct(who)))
}

fn is_admin(view: &AclView, who: u8) -> bool {
    view.is_group_admin(&acct(who), group())
}

#[test]
fn a_removed_admin_cannot_re_add_itself_from_a_cut_before_its_removal() {
    let ad = admins();
    let head = &ad[2];
    let mut log = ad.clone();

    // Sam has not seen his removal and re-adds himself; his chain is deeper
    // than the removal, so by stamps alone it would win.
    let removal = remove(ALICE, &[head], SAM);
    let s1 = add(SAM, &[head], YARA, GroupMemberRole::Member);
    let s2 = add(SAM, &[&s1], ZED, GroupMemberRole::Member);
    let readd = add(SAM, &[&s2], SAM, GroupMemberRole::Admin);
    log.extend([removal.clone(), s1.clone(), s2.clone(), readd.clone()]);

    let voided = void(&log);
    for op in [&s1, &s2, &readd] {
        assert!(
            voided.contains(&op.id()),
            "an op by Sam concurrent with his removal is void"
        );
    }
    assert!(!voided.contains(&removal.id()));

    let v = view(&log, &[&removal, &readd]);
    assert!(!is_member(&v, SAM), "the removal stands over the re-add");
    assert!(
        !is_member(&v, YARA),
        "a member Sam added from the old cut is not added"
    );
    assert!(!is_member(&v, ZED));
    assert!(is_admin(&v, ALICE));
}

#[test]
fn a_removed_admin_cannot_add_admins_or_grant_capabilities_from_a_cut_before_its_removal() {
    let ad = admins();
    let head = &ad[2];
    let mut log = ad.clone();

    let removal = remove(ALICE, &[head], SAM);
    let promote = add(SAM, &[head], XAVIER, GroupMemberRole::Admin);
    // What Xavier then does with an authority that came from a void op.
    let by_xavier = add(XAVIER, &[&promote], ZED, GroupMemberRole::Admin);
    let by_zed = add(ZED, &[&by_xavier], WILL, GroupMemberRole::Member);
    log.extend([
        removal.clone(),
        promote.clone(),
        by_xavier.clone(),
        by_zed.clone(),
    ]);

    let voided = void(&log);
    for op in [&promote, &by_xavier, &by_zed] {
        assert!(
            voided.contains(&op.id()),
            "an admin added on a void branch acts for nothing"
        );
    }

    let v = view(&log, &[&removal, &by_zed]);
    for who in [SAM, XAVIER, ZED, WILL] {
        assert!(
            !is_member(&v, who),
            "{who:#x} comes only from a void branch"
        );
    }
}

#[test]
fn a_capability_granted_by_a_void_op_is_recomputed_without_it() {
    let ad = admins();
    let head = &ad[2];
    let mut log = ad.clone();

    let joined = add(ALICE, &[head], YARA, GroupMemberRole::Member);
    let removal = remove(ALICE, &[&joined], SAM);
    let granted = grant(
        SAM,
        &[&joined],
        YARA,
        MemberCapabilities::CAN_INVITE_MEMBERS,
    );
    // Yara invites with the capability Sam gave her.
    let invited = add(YARA, &[&granted], WILL, GroupMemberRole::Member);
    log.extend([
        joined.clone(),
        removal.clone(),
        granted.clone(),
        invited.clone(),
    ]);

    let voided = void(&log);
    assert!(voided.contains(&granted.id()));
    assert!(
        voided.contains(&invited.id()),
        "an op that rests on a capability a void op granted is void"
    );

    let v = view(&log, &[&removal, &invited]);
    assert!(is_member(&v, YARA), "Yara herself was added by Alice");
    assert!(!is_member(&v, WILL));
    assert!(
        !v.member_caps.contains_key(&(group(), acct(YARA))),
        "the grant is not in the view"
    );
}

#[test]
fn another_admins_concurrent_op_survives_the_removal() {
    let ad = admins();
    let head = &ad[2];
    let mut log = ad.clone();

    let removal = remove(ALICE, &[head], SAM);
    let by_bob = add(BOB, &[head], YARA, GroupMemberRole::Member);
    let by_bob_again = add(BOB, &[&by_bob], ZED, GroupMemberRole::Admin);
    log.extend([removal.clone(), by_bob.clone(), by_bob_again.clone()]);

    assert!(void(&log).is_empty(), "nothing by Bob or Alice is void");
    let v = view(&log, &[&removal, &by_bob_again]);
    assert!(is_member(&v, YARA));
    assert!(is_admin(&v, ZED));
    assert!(!is_member(&v, SAM));
}

#[test]
fn ops_before_the_removal_stand_and_ops_after_it_are_left_to_the_cut_check() {
    let ad = admins();
    let head = &ad[2];
    let mut log = ad.clone();

    let before = add(SAM, &[head], YARA, GroupMemberRole::Member);
    let removal = remove(ALICE, &[&before], SAM);
    let after = add(SAM, &[&removal], ZED, GroupMemberRole::Member);
    log.extend([before.clone(), removal.clone(), after.clone()]);

    let voided = void(&log);
    assert!(
        !voided.contains(&before.id()),
        "an ancestor of the removal stands"
    );
    assert!(
        !voided.contains(&after.id()),
        "a descendant is not concurrent: the existing cut check refuses it"
    );
    let v = view(&log, &[&after]);
    assert!(is_member(&v, YARA));
    assert!(
        !is_admin(&v, SAM),
        "Sam holds no authority at a cut after his removal"
    );
}

#[test]
fn a_mutual_removal_removes_both() {
    let ad = admins();
    let head = &ad[2];
    let mut log = ad.clone();

    let alice_removes_sam = remove(ALICE, &[head], SAM);
    let sam_removes_alice = remove(SAM, &[head], ALICE);
    log.extend([alice_removes_sam.clone(), sam_removes_alice.clone()]);

    assert!(void(&log).is_empty(), "both removals take effect");
    let v = view(&log, &[&alice_removes_sam, &sam_removes_alice]);
    assert!(!is_member(&v, SAM));
    assert!(!is_member(&v, ALICE));
    assert!(is_admin(&v, BOB));
}

#[test]
fn a_cycle_of_three_removals_voids_every_removal_and_removes_nobody() {
    let ad = admins();
    let head = &ad[2];
    let mut log = ad.clone();

    let a_removes_s = remove(ALICE, &[head], SAM);
    let s_removes_b = remove(SAM, &[head], BOB);
    let b_removes_a = remove(BOB, &[head], ALICE);
    log.extend([
        a_removes_s.clone(),
        s_removes_b.clone(),
        b_removes_a.clone(),
    ]);

    let voided = void(&log);
    for op in [&a_removes_s, &s_removes_b, &b_removes_a] {
        assert!(voided.contains(&op.id()));
    }
    let v = view(&log, &[&a_removes_s, &s_removes_b, &b_removes_a]);
    for who in [ALICE, SAM, BOB] {
        assert!(is_admin(&v, who));
    }
}

#[test]
fn a_removal_that_is_itself_void_voids_nothing() {
    let ad = admins();
    let head = &ad[2];
    let mut log = ad.clone();

    // Alice removes Sam. Sam, concurrently, removes Bob: void, because Sam is
    // removed and Bob did not remove Alice. Bob's concurrent op therefore stands.
    let a_removes_s = remove(ALICE, &[head], SAM);
    let s_removes_b = remove(SAM, &[head], BOB);
    let by_bob = add(BOB, &[head], YARA, GroupMemberRole::Member);
    log.extend([a_removes_s.clone(), s_removes_b.clone(), by_bob.clone()]);

    let voided = void(&log);
    assert!(voided.contains(&s_removes_b.id()));
    assert!(!voided.contains(&by_bob.id()));
    let v = view(&log, &[&a_removes_s, &s_removes_b, &by_bob]);
    assert!(is_admin(&v, BOB));
    assert!(is_member(&v, YARA));
}

#[test]
fn the_namespace_owner_survives_a_concurrent_removal() {
    let ad = admins();
    let head = &ad[2];
    let mut log = ad.clone();

    // A removal of the owner, as a log built by an older node may hold.
    let removal = remove(ALICE, &[head], OWNER);
    let by_owner = add(OWNER, &[head], YARA, GroupMemberRole::Member);
    let by_owner_again = add(OWNER, &[&by_owner], ZED, GroupMemberRole::Admin);
    log.extend([removal.clone(), by_owner.clone(), by_owner_again.clone()]);

    assert!(void(&log).is_empty(), "the owner's ops are never void");
    let v = view(&log, &[&removal, &by_owner_again]);
    assert!(is_member(&v, YARA));
    assert!(is_admin(&v, ZED));
}

#[test]
fn a_demoted_admin_loses_its_concurrent_ops() {
    let ad = admins();
    let head = &ad[2];
    let mut log = ad.clone();

    let demotion = add(ALICE, &[head], SAM, GroupMemberRole::Member);
    let by_sam = add(SAM, &[head], YARA, GroupMemberRole::Member);
    let by_sam_again = add(SAM, &[&by_sam], ZED, GroupMemberRole::Admin);
    log.extend([demotion.clone(), by_sam.clone(), by_sam_again.clone()]);

    let voided = void(&log);
    assert!(voided.contains(&by_sam.id()));
    assert!(voided.contains(&by_sam_again.id()));
    assert!(!voided.contains(&demotion.id()));
    let v = view(&log, &[&demotion, &by_sam_again]);
    assert!(is_member(&v, SAM) && !is_admin(&v, SAM));
    assert!(!is_member(&v, YARA) && !is_member(&v, ZED));
}

#[test]
fn a_plain_member_added_again_as_a_member_voids_nothing() {
    let ad = admins();
    let head = &ad[2];
    let mut log = ad.clone();

    let joined = add(ALICE, &[head], YARA, GroupMemberRole::Member);
    let same_role = add(ALICE, &[&joined], YARA, GroupMemberRole::Member);
    let by_yara = add(YARA, &[&joined], WILL, GroupMemberRole::Member);
    log.extend([joined, same_role, by_yara]);

    assert!(
        void(&log).is_empty(),
        "no removal, demotion or revocation is in the log"
    );
}

#[test]
fn the_void_set_and_the_views_are_the_same_in_any_log_order() {
    let ad = admins();
    let head = &ad[2];
    let mut log = ad.clone();

    let removal = remove(ALICE, &[head], SAM);
    let promote = add(SAM, &[head], XAVIER, GroupMemberRole::Admin);
    let by_xavier = add(XAVIER, &[&promote], ZED, GroupMemberRole::Admin);
    let by_bob = add(BOB, &[head], YARA, GroupMemberRole::Member);
    log.extend([removal.clone(), promote, by_xavier.clone(), by_bob.clone()]);

    let forward = void(&log);
    let forward_view = view(&log, &[&removal, &by_xavier, &by_bob]);
    assert!(!forward.is_empty());

    let mut reversed = log.clone();
    reversed.reverse();
    let mut rotated = log.clone();
    rotated.rotate_left(3);
    for shuffled in [reversed, rotated] {
        assert_eq!(void(&shuffled), forward);
        assert_eq!(
            view(&shuffled, &[&removal, &by_xavier, &by_bob]),
            forward_view
        );
    }
}

#[test]
fn an_op_arriving_before_or_after_its_removal_is_judged_alike() {
    let ad = admins();
    let head = &ad[2];
    let removal = remove(ALICE, &[head], SAM);
    let by_sam = add(SAM, &[head], XAVIER, GroupMemberRole::Admin);

    let mut op_first = ad.clone();
    op_first.push(by_sam.clone());
    assert!(
        void(&op_first).is_empty(),
        "with the removal unknown, the op is judged at its own cut and stands"
    );
    op_first.push(removal.clone());

    let mut removal_first = ad.clone();
    removal_first.push(removal.clone());
    removal_first.push(by_sam.clone());

    assert_eq!(void(&op_first), void(&removal_first));
    assert!(void(&op_first).contains(&by_sam.id()));
    assert_eq!(
        view(&op_first, &[&removal, &by_sam]),
        view(&removal_first, &[&removal, &by_sam])
    );

    // The same op judged before it is stored, as the apply path does.
    let candidate = ScopeState::void_ops_with(&removal_first[..4], base(), Some((&by_sam, None)));
    assert!(candidate.contains(&by_sam.id()));
}

#[test]
fn a_removal_from_a_subgroup_leaves_the_signers_ops_above_and_beside_it() {
    let ad = admins();
    let mut log = ad.clone();
    let h = create_subgroup(&[&ad[2]], 0x41);
    let k = create_subgroup(&[&h], 0x42);
    log.extend([h.clone(), k.clone()]);

    // Alice removes Sam from subgroup H only.
    let removal = gov(
        ALICE,
        &[&k],
        OpPayload::MemberRemoved {
            group: subgroup(0x41),
            member: acct(SAM),
        },
    );
    let in_parent = add_in(group(), SAM, &[&k], YARA, GroupMemberRole::Member);
    let in_sibling = add_in(subgroup(0x42), SAM, &[&k], ZED, GroupMemberRole::Member);
    let in_removed_from = add_in(subgroup(0x41), SAM, &[&k], WILL, GroupMemberRole::Member);
    log.extend([
        removal,
        in_parent.clone(),
        in_sibling.clone(),
        in_removed_from.clone(),
    ]);

    let voided = void(&log);
    assert!(
        !voided.contains(&in_parent.id()),
        "the parent is not where Sam was removed"
    );
    assert!(!voided.contains(&in_sibling.id()), "nor is a sibling");
    assert!(voided.contains(&in_removed_from.id()));
}

#[test]
fn a_removal_from_a_group_voids_the_signers_concurrent_ops_in_its_subgroups() {
    let ad = admins();
    let mut log = ad.clone();
    let h = create_subgroup(&[&ad[2]], 0x41);
    log.push(h.clone());

    let removal = remove(ALICE, &[&h], SAM);
    let below = add_in(subgroup(0x41), SAM, &[&h], WILL, GroupMemberRole::Member);
    log.extend([removal, below.clone()]);

    assert!(void(&log).contains(&below.id()));
}

#[test]
fn a_role_change_for_an_account_that_was_no_admin_at_its_cut_demotes_nobody() {
    let ad = admins();
    let head = &ad[2];
    let mut log = ad.clone();

    // Alice promotes Xavier. Bob, who has not seen that, sets him to Member: at
    // Bob's cut Xavier is no admin, so this takes nothing away.
    let promotion = add(ALICE, &[head], XAVIER, GroupMemberRole::Admin);
    let role_set = add(BOB, &[head], XAVIER, GroupMemberRole::Member);
    let by_xavier = add(XAVIER, &[&promotion], YARA, GroupMemberRole::Member);
    log.extend([promotion, role_set, by_xavier.clone()]);

    assert!(!void(&log).contains(&by_xavier.id()));
}

#[test]
fn past_the_fold_budget_the_candidates_whose_standing_rests_on_a_void_op_are_void() {
    let ad = admins();
    let head = &ad[2];
    let mut log = ad.clone();

    // Xavier is an admin twice over: Alice promotes him, and Sam, concurrently
    // with his own removal, does too. What Xavier does stands on Alice's grant.
    let removal = remove(ALICE, &[head], SAM);
    let by_alice = add(ALICE, &[head], XAVIER, GroupMemberRole::Admin);
    let by_sam = add(SAM, &[head], XAVIER, GroupMemberRole::Admin);
    let by_xavier = add(XAVIER, &[&by_alice, &by_sam], YARA, GroupMemberRole::Member);
    log.extend([removal, by_alice, by_sam.clone(), by_xavier.clone()]);

    let judged = ScopeState::void_ops_bounded(&log, base(), None, &[], usize::MAX);
    assert!(judged.contains(&by_sam.id()));
    assert!(
        !judged.contains(&by_xavier.id()),
        "judged, Xavier's op stands on Alice's grant"
    );

    let unjudged = ScopeState::void_ops_bounded(&log, base(), None, &[], 0);
    assert!(
        unjudged.contains(&by_xavier.id()),
        "with no budget to judge it, an op of an account a void op promoted is void"
    );
}

#[test]
fn an_op_the_projection_models_nothing_about_is_judged_by_the_group_it_acted_in() {
    let ad = admins();
    let head = &ad[2];
    let mut log = ad.clone();

    let removal = remove(ALICE, &[head], SAM);
    let rotation = gov(SAM, &[head], OpPayload::Noop);
    let by_bob = gov(BOB, &[head], OpPayload::Noop);
    log.extend([removal, rotation.clone(), by_bob.clone()]);

    let held = [(rotation.id(), group()), (by_bob.id(), group())];
    let judged = ScopeState::void_ops_judging(&log, base(), None, &held);
    assert!(judged.contains(&rotation.id()));
    assert!(!judged.contains(&by_bob.id()), "Bob was not removed");

    assert!(
        !ScopeState::void_ops(&log, base()).contains(&rotation.id()),
        "with no group named, the log alone cannot say where it acted"
    );
}

#[test]
fn a_tee_policy_a_removed_admin_set_concurrently_is_void() {
    let ad = admins();
    let head = &ad[2];
    let mut log = ad.clone();

    let removal = remove(ALICE, &[head], SAM);
    let policy = gov(
        SAM,
        &[head],
        OpPayload::TeeAuthoringPolicySet {
            group: group(),
            allowed_mrtd: vec!["abc".to_owned()],
        },
    );
    log.extend([removal, policy.clone()]);

    assert!(ScopeState::void_ops(&log, base()).contains(&policy.id()));
}

#[test]
fn verified_tee_evidence_a_removed_admin_submitted_concurrently_is_void() {
    let ad = admins();
    let head = &ad[2];
    let mut log = ad.clone();

    let removal = remove(ALICE, &[head], SAM);
    let evidence = gov(
        SAM,
        &[head],
        OpPayload::TeeAuthorityEvidence {
            group: group(),
            member: acct(XAVIER),
            attested_key: calimero_primitives::identity::PublicKey::from([9; 32]),
            mrtd: "abc".to_owned(),
            attested_at: 1,
        },
    );
    log.extend([removal, evidence.clone()]);

    assert!(ScopeState::void_ops(&log, base()).contains(&evidence.id()));
}

#[test]
fn a_role_change_by_a_non_admin_is_no_demotion() {
    let ad = admins();
    let head = &ad[2];
    let mut log = ad.clone();

    // Yara, who is no admin, names the admin Bob with a non-admin role: the apply
    // logs that without acting on it (as for a TEE admitted over an existing member).
    let member = add(OWNER, &[head], YARA, GroupMemberRole::Member);
    let no_effect = add(YARA, &[&member], BOB, GroupMemberRole::Member);
    let by_bob = add(BOB, &[head], ZED, GroupMemberRole::Member);
    log.extend([member, no_effect, by_bob.clone()]);

    assert!(!ScopeState::void_ops(&log, base()).contains(&by_bob.id()));
}

#[test]
fn a_demotion_by_an_admin_who_reaches_the_group_through_an_open_chain_counts() {
    let ad = admins();
    let head = &ad[2];
    let mut log = ad.clone();

    // Sam administers subgroup 1; Yara administers its open child, subgroup 2.
    let first = create_subgroup(&[head], 1);
    let second = gov(
        OWNER,
        &[&first],
        OpPayload::SubgroupCreated {
            child: ScopeId::from([2u8; 32]),
            parent: ScopeId::from([1u8; 32]),
            restricted: false,
            admin: acct(YARA),
        },
    );
    // Sam demotes Yara in subgroup 2 while Yara acts there.
    let demotion = add_in(subgroup(2), SAM, &[&second], YARA, GroupMemberRole::Member);
    let by_yara = add_in(subgroup(2), YARA, &[&second], ZED, GroupMemberRole::Member);
    log.extend([first, second, demotion, by_yara.clone()]);

    assert!(ScopeState::void_ops(&log, base()).contains(&by_yara.id()));
}

#[test]
fn a_payload_the_void_rule_reads_outlives_the_bytes_of_its_op() {
    let root = ScopeId::from(group().to_bytes());
    let payloads = [
        OpPayload::MemberAdded {
            group: group(),
            member: acct(ALICE),
            role: GroupMemberRole::Member,
        },
        OpPayload::MemberRemoved {
            group: group(),
            member: acct(ALICE),
        },
        OpPayload::MemberCapabilitySet {
            group: group(),
            member: acct(ALICE),
            capabilities: MemberCapabilities::CAN_INVITE_MEMBERS,
        },
        OpPayload::DefaultCapabilitiesSet {
            group: group(),
            capabilities: MemberCapabilities::CAN_INVITE_MEMBERS,
        },
        OpPayload::AdminChanged {
            new_admin: acct(ALICE),
        },
        OpPayload::PolicyUpdated {
            policy_bytes: vec![1],
        },
        OpPayload::SubgroupVisibilitySet {
            scope: root,
            restricted: true,
        },
        OpPayload::SubgroupCreated {
            child: ScopeId::from([9u8; 32]),
            parent: root,
            restricted: false,
            admin: acct(ALICE),
        },
        OpPayload::SubgroupReparented {
            child: ScopeId::from([9u8; 32]),
            new_parent: root,
        },
        OpPayload::SubgroupDeleted { scope: root },
        OpPayload::TeeAuthoringPolicySet {
            group: group(),
            allowed_mrtd: Vec::new(),
        },
    ];
    for payload in payloads {
        let op = gov(ALICE, &[], payload);
        assert!(
            super::void::payload_group(&op).is_some(),
            "the list is of payloads the rule can void"
        );
        assert!(
            op.payload.outlives_void_bytes(),
            "a voidable payload must survive losing its op's bytes: {:?}",
            std::mem::discriminant(&op.payload)
        );
    }
    assert!(!OpPayload::Noop.outlives_void_bytes());
    assert!(!OpPayload::Opaque { group: group() }.outlives_void_bytes());
}

#[test]
fn a_void_policy_with_its_bytes_dropped_is_still_void() {
    let ad = admins();
    let head = &ad[2];
    let mut log = ad.clone();

    // A policy acts in the scope's root group, which is the group Sam is removed from.
    let root = ContextGroupId::from([0u8; 32]);
    let removal = gov(
        ALICE,
        &[head],
        OpPayload::MemberRemoved {
            group: root,
            member: acct(SAM),
        },
    );
    let policy = gov(
        SAM,
        &[head],
        OpPayload::PolicyUpdated {
            policy_bytes: Vec::new(),
        },
    );
    log.extend([removal, policy.clone()]);

    assert!(ScopeState::void_ops(&log, base()).contains(&policy.id()));
}
