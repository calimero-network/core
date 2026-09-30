//! The maintained projection reads a log minus the ops a removal voids, in any
//! arrival order.

use calimero_account::AccountId;
use calimero_context::scope_projection::ScopeProjections;
use calimero_context_config::types::ContextGroupId;
use calimero_op::{Authorship, Op, OpPayload, ScopeId};
use calimero_primitives::context::GroupMemberRole;
use calimero_projection::AuthorityBase;
use calimero_storage::logical_clock::{HybridTimestamp, Timestamp, ID, NTP64};
use core::num::NonZeroU128;

const OWNER: u8 = 0x10;
const ALICE: u8 = 0x11;
const SAM: u8 = 0x12;
const XAVIER: u8 = 0x14;
const YARA: u8 = 0x15;
const ZED: u8 = 0x16;

fn scope() -> ScopeId {
    ScopeId::from([0u8; 32])
}

fn group() -> ContextGroupId {
    ContextGroupId::from([3u8; 32])
}

fn acct(n: u8) -> AccountId {
    AccountId::from([n; 32])
}

fn base() -> AuthorityBase {
    AuthorityBase {
        root: Some((group(), acct(OWNER))),
        default_cap_base: 0,
    }
}

fn gov(author: u8, parents: &[&Op], payload: OpPayload) -> Op {
    Op::new(
        scope(),
        parents.iter().map(|p| p.id()).collect(),
        Authorship {
            account: acct(author),
            device: calimero_account::DeviceId::from([author; 32]),
            device_key: calimero_primitives::identity::PublicKey::from([author; 32]),
        },
        HybridTimestamp::new(Timestamp::new(
            NTP64(0),
            ID::from(NonZeroU128::new(1).unwrap()),
        )),
        payload,
        [0u8; 32],
        [0u8; 64],
    )
}

fn add(author: u8, parents: &[&Op], member: u8, role: GroupMemberRole) -> Op {
    gov(
        author,
        parents,
        OpPayload::MemberAdded {
            group: group(),
            member: acct(member),
            role,
        },
    )
}

struct Log {
    genesis: Vec<Op>,
    removal: Op,
    by_sam: Op,
    by_sam_again: Op,
    by_alice: Op,
}

fn log() -> Log {
    let a = add(OWNER, &[], ALICE, GroupMemberRole::Admin);
    let s = add(OWNER, &[&a], SAM, GroupMemberRole::Admin);
    let removal = gov(
        ALICE,
        &[&s],
        OpPayload::MemberRemoved {
            group: group(),
            member: acct(SAM),
        },
    );
    // Sam, who has not seen the removal, promotes Xavier and Xavier adds Yara.
    let by_sam = add(SAM, &[&s], XAVIER, GroupMemberRole::Admin);
    let by_sam_again = add(XAVIER, &[&by_sam], YARA, GroupMemberRole::Member);
    let by_alice = add(ALICE, &[&s], ZED, GroupMemberRole::Member);
    Log {
        genesis: vec![a, s],
        removal,
        by_sam,
        by_sam_again,
        by_alice,
    }
}

fn fed(ops: &[&Op]) -> ScopeProjections {
    let mut proj = ScopeProjections::new();
    for op in ops {
        proj.ingest_op(op);
    }
    proj
}

#[test]
fn the_views_and_the_root_do_not_depend_on_which_arrived_first() {
    let l = log();
    let [a, s] = [&l.genesis[0], &l.genesis[1]];
    let heads = [l.removal.id(), l.by_sam_again.id(), l.by_alice.id()];

    let ops_first = fed(&[a, s, &l.by_sam, &l.by_sam_again, &l.by_alice, &l.removal]);
    let removal_first = fed(&[a, s, &l.removal, &l.by_alice, &l.by_sam_again, &l.by_sam]);
    // What a node that never saw the voided ops holds.
    let without = fed(&[a, s, &l.removal, &l.by_alice]);

    let view = ops_first.acl_view_at(&scope(), &heads).expect("fed");
    assert_eq!(
        removal_first.acl_view_at(&scope(), &heads).expect("fed"),
        view
    );
    let groups = view.groups.get(&group()).expect("the group");
    assert!(groups.contains_key(&acct(ALICE)) && groups.contains_key(&acct(ZED)));
    for voided in [SAM, XAVIER, YARA] {
        assert!(
            !groups.contains_key(&acct(voided)),
            "{voided:#x} is not a member"
        );
    }

    let root = ops_first.scope_root_for(&scope(), [0u8; 32]).expect("fed");
    assert_eq!(
        removal_first.scope_root_for(&scope(), [0u8; 32]),
        Some(root)
    );
    assert_eq!(
        without.scope_root_for(&scope(), [0u8; 32]),
        Some(root),
        "the streaming root is the root of a log without the voided ops"
    );
    assert_eq!(ops_first.role_of(&scope(), &group(), &acct(XAVIER)), None);
}

#[test]
fn an_op_is_judged_void_before_it_is_stored() {
    let l = log();
    let proj = fed(&[&l.genesis[0], &l.genesis[1], &l.removal]);

    assert_eq!(
        proj.op_is_void(&scope(), base(), &l.by_sam, Some(group())),
        Some(true),
        "Sam's op cites a cut from before his removal"
    );
    assert_eq!(
        proj.op_is_void(&scope(), base(), &l.by_alice, Some(group())),
        Some(false)
    );
}

#[test]
fn an_op_whose_cut_is_not_held_is_not_judged() {
    let l = log();
    let proj = fed(&[&l.genesis[0], &l.removal]);
    assert_eq!(
        proj.op_is_void(&scope(), base(), &l.by_sam, Some(group())),
        None,
        "the cut cited here has a gap; the answer would depend on what this node holds"
    );
}
