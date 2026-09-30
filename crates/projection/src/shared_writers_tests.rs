//! A shared cell's writer set at a cut, folded from its rotation ops.

use core::num::NonZeroU128;
use std::collections::BTreeMap;

use calimero_account::{AccountId, DeviceId};
use calimero_op::{Authorship, Op, OpPayload, ScopeId};
use calimero_primitives::context::ContextId;
use calimero_primitives::identity::PublicKey;
use calimero_storage::address::Id;
use calimero_storage::collections::cell_id;
use calimero_storage::entities::OpMask;
use calimero_storage::logical_clock::{HybridTimestamp, Timestamp, ID, NTP64};

use crate::ScopeState;

const CONTEXT: [u8; 32] = [0xC7; 32];

fn account(seed: u8) -> AccountId {
    AccountId::from([seed; 32])
}

fn writers(who: &[u8]) -> BTreeMap<AccountId, OpMask> {
    who.iter().map(|&w| (account(w), OpMask::FULL)).collect()
}

/// The cell whose genesis set is `genesis`.
fn cell(genesis: &[u8]) -> Id {
    cell_id(Id::new([0x11; 32]), &writers(genesis))
}

/// `by` stepping `cell` from `prior` to `new` at `nonce`, on `parents`.
fn step(by: u8, cell: Id, prior: &[u8], new: &[u8], nonce: u64, parents: Vec<[u8; 32]>) -> Op {
    rotation(by, cell, writers(prior), writers(new), nonce, parents)
}

fn rotation(
    by: u8,
    cell: Id,
    prior: BTreeMap<AccountId, OpMask>,
    new: BTreeMap<AccountId, OpMask>,
    nonce: u64,
    parents: Vec<[u8; 32]>,
) -> Op {
    Op::new(
        ScopeId::from([0; 32]),
        parents,
        Authorship {
            account: account(by),
            device: DeviceId::from([by ^ 0x5A; 32]),
            device_key: PublicKey::from([by ^ 0xA5; 32]),
        },
        HybridTimestamp::new(Timestamp::new(NTP64(0), ID::from(NonZeroU128::MIN))),
        OpPayload::SharedWritersRotated {
            context: ContextId::from(CONTEXT),
            cell,
            prior,
            nonce,
            new,
        },
        [0; 32],
        [0; 64],
    )
}

fn writers_at(log: &[Op], cut: &[&Op], cell: Id) -> Option<BTreeMap<AccountId, OpMask>> {
    let parents: Vec<[u8; 32]> = cut.iter().map(|op| op.id()).collect();
    ScopeState::acl_view_at(log, &parents)
        .shared_writers(ContextId::from(CONTEXT), cell)
        .cloned()
}

const ALICE: u8 = 0xA1;
const BOB: u8 = 0xB0;
const MALLORY: u8 = 0xEE;

#[test]
fn a_genuine_rotation_applies_and_removes_whom_it_drops() {
    let cell = cell(&[ALICE]);
    let add = step(ALICE, cell, &[ALICE], &[ALICE, BOB], 1, vec![]);
    let drop = step(ALICE, cell, &[ALICE, BOB], &[ALICE], 3, vec![add.id()]);
    let log = [add.clone(), drop.clone()];
    assert_eq!(
        writers_at(&log, &[&add], cell),
        Some(writers(&[ALICE, BOB]))
    );
    assert_eq!(writers_at(&log, &[&drop], cell), Some(writers(&[ALICE])));
    assert_eq!(
        writers_at(&log, &[], cell),
        None,
        "a cut before any step: the genesis set stands"
    );
}

#[test]
fn a_replayed_admin_signature_does_not_install_another_writer_set() {
    let cell = cell(&[ALICE]);
    let genuine = step(ALICE, cell, &[ALICE], &[ALICE, BOB], 1, vec![]);
    // Alice's step, re-published by Mallory with Mallory's writer set: the op is
    // Mallory's, and she holds nothing in the set it steps from.
    let replay = step(MALLORY, cell, &[ALICE], &[MALLORY], 1, vec![]);
    let log = [genuine.clone(), replay.clone()];
    let set = writers_at(&log, &[&genuine, &replay], cell).expect("the genuine step");
    assert!(!set.contains_key(&account(MALLORY)));
    assert_eq!(set, writers(&[ALICE, BOB]));
}

#[test]
fn a_strangers_first_log_entry_does_not_become_the_writer_set() {
    let cell = cell(&[ALICE]);
    // From a set Mallory made up, and from the cell's real genesis set.
    for from in [&[MALLORY][..], &[ALICE][..]] {
        let planted = step(MALLORY, cell, from, &[MALLORY], 1, vec![]);
        assert_eq!(
            writers_at(core::slice::from_ref(&planted), &[&planted], cell),
            None,
            "planted from {from:?}"
        );
    }
}

#[test]
fn a_signer_without_admin_in_the_prior_set_is_not_counted() {
    let genesis: BTreeMap<_, _> = [
        (account(ALICE), OpMask::FULL),
        (account(BOB), OpMask::WRITE),
    ]
    .into();
    let cell = cell_id(Id::new([0x11; 32]), &genesis);
    let by_bob = rotation(BOB, cell, genesis, writers(&[BOB]), 1, vec![]);
    assert_eq!(
        writers_at(core::slice::from_ref(&by_bob), &[&by_bob], cell),
        None
    );
}

#[test]
fn concurrent_steps_from_one_set_resolve_alike_in_every_arrival_order() {
    let genesis = [ALICE, BOB];
    let cell = cell(&genesis);
    let by_alice = step(ALICE, cell, &genesis, &[ALICE, BOB, 0xC1], 5, vec![]);
    let by_bob = step(BOB, cell, &genesis, &[ALICE, BOB, 0xC2], 5, vec![]);
    let later = step(BOB, cell, &genesis, &[ALICE, BOB, 0xC3], 6, vec![]);
    let cut = [&by_alice, &by_bob, &later];
    let orders = [
        [by_alice.clone(), by_bob.clone(), later.clone()],
        [later.clone(), by_bob.clone(), by_alice.clone()],
        [by_bob.clone(), later.clone(), by_alice.clone()],
    ];
    let expected = if by_alice.device_key() < by_bob.device_key() {
        writers(&[ALICE, BOB, 0xC1])
    } else {
        writers(&[ALICE, BOB, 0xC2])
    };
    for log in &orders {
        assert_eq!(
            writers_at(log, &cut, cell),
            Some(expected.clone()),
            "lowest nonce, then signer key"
        );
    }
}

#[test]
fn a_former_admin_cannot_override_its_removal_from_an_older_cut() {
    let genesis = [ALICE, BOB];
    let cell = cell(&genesis);
    let removal = step(BOB, cell, &genesis, &[BOB], 20, vec![]);
    // Alice forks from the set Bob removed her from, below his nonce, and keeps
    // him an admin so that she is not removing him in turn.
    let fork = step(ALICE, cell, &genesis, &[ALICE, BOB, MALLORY], 5, vec![]);
    for log in [
        [removal.clone(), fork.clone()],
        [fork.clone(), removal.clone()],
    ] {
        assert_eq!(
            writers_at(&log, &[&removal, &fork], cell),
            Some(writers(&[BOB])),
            "the removal wins whatever the nonces"
        );
    }
    assert_eq!(
        writers_at(&[removal.clone(), fork.clone()], &[&fork], cell),
        Some(writers(&[ALICE, BOB, MALLORY])),
        "at a cut without the removal her step stands"
    );
}

const CAROL: u8 = 0xCC;

/// Bob adds Carol, then removes Alice; Alice forks from the set before Carol,
/// below both of Bob's nonces.
fn fork_from_further_back() -> (Id, [Op; 3]) {
    let genesis = [ALICE, BOB];
    let cell = cell(&genesis);
    let add_carol = step(BOB, cell, &genesis, &[ALICE, BOB, CAROL], 10, vec![]);
    let remove_alice = step(
        BOB,
        cell,
        &[ALICE, BOB, CAROL],
        &[BOB, CAROL],
        20,
        vec![add_carol.id()],
    );
    let fork = step(ALICE, cell, &genesis, &[ALICE, BOB, MALLORY], 5, vec![]);
    (cell, [add_carol, remove_alice, fork])
}

#[test]
fn a_removed_admin_cannot_override_its_removal_by_forking_from_further_back() {
    let (cell, [add_carol, remove_alice, fork]) = fork_from_further_back();
    let log = [add_carol.clone(), remove_alice.clone(), fork.clone()];
    assert_eq!(
        writers_at(&log, &[&remove_alice, &fork], cell),
        Some(writers(&[BOB, CAROL])),
        "a step concurrent with her removal is void, however low its nonce"
    );
}

#[test]
fn the_writer_set_at_a_cut_is_the_same_in_every_arrival_order() {
    let (cell, [add_carol, remove_alice, fork]) = fork_from_further_back();
    let genesis = [ALICE, BOB];
    let sibling = step(BOB, cell, &genesis, &[ALICE, BOB, 0xC4], 12, vec![]);
    let ops = [add_carol, remove_alice, fork, sibling];
    let cut: Vec<&Op> = ops.iter().collect();
    let mut orders = vec![ops.to_vec()];
    for rotate in 1..ops.len() {
        let mut order = ops.to_vec();
        order.rotate_left(rotate);
        orders.push(order.clone());
        order.reverse();
        orders.push(order);
    }
    for log in &orders {
        assert_eq!(
            writers_at(log, &cut, cell),
            Some(writers(&[BOB, CAROL])),
            "the removal's branch wins over its concurrent sibling on the lower nonce"
        );
    }
}

#[test]
fn an_admins_sequential_rotations_apply_in_causal_order_whatever_its_clock() {
    let cell = cell(&[ALICE]);
    let first = step(ALICE, cell, &[ALICE], &[ALICE, BOB], 10, vec![]);
    // The clock went backwards between the two.
    let second = step(
        ALICE,
        cell,
        &[ALICE, BOB],
        &[ALICE, CAROL],
        5,
        vec![first.id()],
    );
    let log = [second.clone(), first.clone()];
    assert_eq!(
        writers_at(&log, &[&second], cell),
        Some(writers(&[ALICE, CAROL]))
    );
}
