//! A registry converges on one owner per name however its claims and verdicts
//! arrive.
//!
//! Each test scripts who sees what before deciding, then replays every order a
//! DAG could deliver all the deltas in (each after what its author had seen,
//! everything concurrent both ways) on a fresh replica, and asserts one root
//! hash and the owner every replica must agree on. The cases are the ones a single authority never produces and a
//! real deployment does: two TEEs that each saw a different claimant, a TEE
//! whose verdict is an epoch behind, two devices of one admin.
//!
//! Run with: `cargo test -p calimero-storage --features testing --test converge_registry`

#![cfg(feature = "testing")]
#![allow(clippy::unwrap_used)]

use calimero_account::AccountId;
use calimero_storage::collections::{Admin, NoAuthority, Registry, Status, Tee};
use calimero_storage::testing::Script;
use serial_test::serial;

type Names = Registry<String, u64, Tee>;

fn alice() -> String {
    "alice".to_owned()
}

fn claim<A: calimero_storage::collections::RegistryAuthority>(
    value: u64,
) -> impl FnOnce(&mut Registry<String, u64, A>) {
    move |names| names.claim(alice(), value).unwrap()
}

fn resolve<A: calimero_storage::collections::RegistryAuthority>(
) -> impl FnOnce(&mut Registry<String, u64, A>) {
    |names| {
        let _owner = names.resolve(&alice()).unwrap();
    }
}

#[test]
#[serial]
fn two_tees_that_granted_different_claimants_converge_on_one_owner() {
    let mut script = Script::new(Names::new);
    let (ann, bob) = (script.member(), script.member());
    let (tee1, tee2, judge) = (script.tee(), script.tee(), script.tee());

    let ann_claims = script.run(ann, claim(1)).unwrap();
    let bob_claims = script.run(bob, claim(2)).unwrap();

    // Partitioned: each TEE sees one claimant, and grants it, creating the
    // TEE-only cell as it does.
    assert_eq!(script.deliver(tee1, ann_claims), 0);
    assert_eq!(script.deliver(tee2, bob_claims), 0);
    let grant_ann = script.run(tee1, resolve()).unwrap();
    let _ = script.run(tee2, resolve()).unwrap();

    // A repair can deliver a verdict before the claim it grants. Until the
    // claim lands the name is granted but not owned.
    assert_eq!(script.deliver(bob, grant_ann), 0);
    assert_eq!(
        script.view(bob, |n| n.status(&alice()).unwrap()),
        Status::Pending {
            claimants: 1,
            mine: true
        }
    );
    assert_eq!(script.view(bob, |n| n.owner_of(&alice()).unwrap()), None);
    assert_eq!(script.deliver(bob, ann_claims), 0);
    assert!(matches!(
        script.view(bob, |n| n.status(&alice()).unwrap()),
        Status::Lost { .. } | Status::Owned { .. }
    ));
    assert_eq!(
        script.view(tee1, |n| n.owner_of(&alice()).unwrap()),
        Some(script.account(ann))
    );
    assert_eq!(
        script.view(tee2, |n| n.owner_of(&alice()).unwrap()),
        Some(script.account(bob))
    );

    // A third TEE that saw both claims grants the one the merge must keep:
    // the lower order, whichever partition it came from.
    assert_eq!(script.deliver(judge, ann_claims), 0);
    assert_eq!(script.deliver(judge, bob_claims), 0);
    let _ = script.run(judge, resolve()).unwrap();
    let expected: AccountId = script.view(judge, |n| n.owner_of(&alice()).unwrap().unwrap());
    let value = if expected == script.account(ann) {
        1
    } else {
        2
    };

    let orders = script.assert_every_order_converges(|names| {
        names.owner_of(&alice()).unwrap() == Some(expected)
            && names.value_of(&alice()).unwrap() == Some(value)
            && names.claimants(&alice()).unwrap().len() == 2
            // The rival verdict is visible, so the owner is not called final.
            && matches!(
                names.status(&alice()).unwrap(),
                Status::Owned { owner, epoch: 0, stable: false } if owner == expected
            )
    });
    assert_eq!(orders, 16);
}

#[test]
#[serial]
fn a_stale_verdict_at_a_lower_epoch_loses_however_late_it_arrives() {
    let mut script = Script::new(Names::new);
    let (ann, bob, cat) = (script.member(), script.member(), script.member());
    let (tee, stale) = (script.tee(), script.tee());

    // Ann is granted `alice` at epoch 0, releases it, and Bob takes it at 1.
    let ann_claims = script.run(ann, claim(1)).unwrap();
    assert_eq!(script.deliver(tee, ann_claims), 0);
    let grant_ann = script.run(tee, resolve()).unwrap();
    assert_eq!(script.deliver(ann, grant_ann), 0);
    let ann_releases = script
        .run(ann, |n: &mut Names| n.release(&alice()).unwrap())
        .unwrap();
    assert_eq!(script.deliver(tee, ann_releases), 0);
    let vacancy = script.run(tee, resolve()).unwrap();
    for delta in [ann_claims, grant_ann, ann_releases, vacancy] {
        assert_eq!(script.deliver(bob, delta), 0);
    }
    assert_eq!(
        script.view(bob, |n| n.status(&alice()).unwrap()),
        Status::Free
    );
    let bob_claims = script.run(bob, claim(2)).unwrap();
    assert_eq!(script.deliver(tee, bob_claims), 0);
    let _grant_bob = script.run(tee, resolve()).unwrap();

    // A TEE that never heard of any of it grants Cat at epoch 0.
    let cat_claims = script.run(cat, claim(3)).unwrap();
    assert_eq!(script.deliver(stale, cat_claims), 0);
    let _stale_grant = script.run(stale, resolve()).unwrap();

    let bob_id = script.account(bob);
    let orders = script.assert_every_order_converges(|names| {
        names.owner_of(&alice()).unwrap() == Some(bob_id)
            && matches!(
                names.status(&alice()).unwrap(),
                Status::Owned { owner, epoch: 1, stable: true } if owner == bob_id
            )
    });
    // Bob's history is one causal chain; Cat's claim and the stale grant
    // interleave with it anywhere.
    assert_eq!(orders, 28);
}

#[test]
#[serial]
fn two_devices_of_one_admin_granting_different_claimants_converge() {
    let mut script = Script::new(Registry::<String, u64, Admin>::new);
    let (ann, bob) = (script.member(), script.member());
    let (desk, phone) = (script.founder(), script.founder());

    let ann_claims = script.run(ann, claim(1)).unwrap();
    let bob_claims = script.run(bob, claim(2)).unwrap();
    assert_eq!(script.deliver(desk, ann_claims), 0);
    assert_eq!(script.deliver(phone, bob_claims), 0);
    let _ = script.run(desk, resolve()).unwrap();
    let _ = script.run(phone, resolve()).unwrap();

    let (ann_id, bob_id) = (script.account(ann), script.account(bob));
    let owners = std::cell::RefCell::new(Vec::new());
    let orders = script.assert_every_order_converges(|names| {
        let owner = names.owner_of(&alice()).unwrap();
        owners.borrow_mut().push(owner);
        owner == Some(ann_id) || owner == Some(bob_id)
    });
    assert_eq!(orders, 6);
    let owners = owners.into_inner();
    assert!(owners.windows(2).all(|pair| pair[0] == pair[1]));
}

#[test]
#[serial]
fn with_no_authority_every_replica_reports_the_same_contest() {
    let mut script = Script::new(Registry::<String, u64, NoAuthority>::new);
    let claimants: Vec<_> = (0..3).map(|_| script.member()).collect();
    for (value, &member) in claimants.iter().enumerate() {
        let _ = script.run(member, claim(value as u64)).unwrap();
    }
    let accounts: Vec<_> = claimants.iter().map(|&m| script.account(m)).collect();
    let orders = script.assert_every_order_converges(|names| {
        names.owner_of(&alice()).unwrap().is_none()
            && match names.status(&alice()).unwrap() {
                Status::Contested { claimants } => {
                    claimants.len() == 3 && accounts.iter().all(|a| claimants.contains(a))
                }
                _ => false,
            }
    });
    assert_eq!(orders, 6);
}
