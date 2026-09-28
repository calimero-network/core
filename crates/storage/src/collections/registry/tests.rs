use std::collections::BTreeSet;

use calimero_account::AccountId;
use ed25519_dalek::SigningKey;
use serial_test::serial;

use super::{
    claim_ref, name_of, order, Admin, Claim, NoAuthority, Registry, RegistryAuthority, Status, Tee,
    VerdictStore,
};
use crate::action::Action;
use crate::address::Id;
use crate::collections::{compute_collection_id, tee_only_id, Root};
use crate::entities::{ChildInfo, Data, Metadata};
use crate::env;
use crate::interface::{MainInterface, StorageError};
use crate::tests::common::{
    account_of_key, apply_ctx_for, assert_every_owned_entry_is_bound,
    assert_every_shared_entity_is_bound, build_signed_member_action, create_signed_user_add_action,
    map_entry_bytes,
};

const ANN: [u8; 32] = [0x11; 32];
const BOB: [u8; 32] = [0x22; 32];
const CAT: [u8; 32] = [0x33; 32];

type Names<A = Tee> = Registry<String, u64, A>;

fn account(bytes: [u8; 32]) -> AccountId {
    AccountId::from(bytes)
}

fn tee() -> [u8; 32] {
    *AccountId::TEE_AUTHORITY.as_bytes()
}

fn alice() -> String {
    "alice".to_owned()
}

/// A registry created by Ann.
fn setup<A: RegistryAuthority>() -> Root<Names<A>> {
    env::reset_for_testing();
    env::set_account_id(ANN);
    Root::new(Names::<A>::new)
}

/// Runs `f` as `who`, then restores the previous account.
fn as_<R>(who: [u8; 32], f: impl FnOnce() -> R) -> R {
    let previous = env::account_id();
    env::set_account_id(who);
    let out = f();
    env::set_account_id(previous);
    out
}

/// `action` filed under `parent`, as a claim arrives from a peer: apply reads an
/// owned map entry's key against the parent its action names. Ancestors are not
/// signed, so this leaves the signature valid.
fn under(parent: Id, mut action: Action) -> Action {
    if let Action::Add { ancestors, .. } = &mut action {
        *ancestors = vec![ChildInfo::new(parent, [0; 32], Metadata::default())];
    }
    action
}

fn verdict_count<A: RegistryAuthority>(names: &Names<A>) -> usize {
    names
        .verdicts
        .read()
        .unwrap()
        .map_or(0, |verdicts| verdicts.len().unwrap())
}

#[test]
#[serial]
fn a_claim_is_pending_until_the_tee_resolves_it() {
    let mut names = setup::<Tee>();
    names.claim(alice(), 7).unwrap();
    assert_eq!(
        names.status(&alice()).unwrap(),
        Status::Pending {
            claimants: 1,
            mine: true
        }
    );
    assert_eq!(names.owner_of(&alice()).unwrap(), None);
    assert_eq!(names.value_of(&alice()).unwrap(), None);
    assert_eq!(names.my_pending().unwrap(), [alice()]);

    let owner = as_(tee(), || names.resolve(&alice())).unwrap();
    assert_eq!(owner, Some(account(ANN)));
    assert_eq!(names.owner_of(&alice()).unwrap(), Some(account(ANN)));
    assert_eq!(names.value_of(&alice()).unwrap(), Some(7));
    assert_eq!(
        names.status(&alice()).unwrap(),
        Status::Owned {
            owner: account(ANN),
            epoch: 0,
            stable: true
        }
    );
    assert!(names.my_pending().unwrap().is_empty());

    // Firing is at least once: a second resolve decides nothing new.
    let again = as_(tee(), || names.resolve(&alice())).unwrap();
    assert_eq!(again, Some(account(ANN)));
    assert_eq!(verdict_count(&names), 1);
    assert_every_owned_entry_is_bound();
    assert_every_shared_entity_is_bound();
}

#[test]
#[serial]
fn two_claimants_get_one_owner_and_the_other_sees_lost() {
    let mut names = setup::<Tee>();
    names.claim(alice(), 1).unwrap();
    as_(BOB, || names.claim(alice(), 2)).unwrap();
    assert_eq!(
        as_(BOB, || names.status(&alice())).unwrap(),
        Status::Pending {
            claimants: 2,
            mine: true
        }
    );

    let owner = as_(tee(), || names.resolve(&alice())).unwrap().unwrap();
    let (winner, loser) = if owner == account(ANN) {
        (ANN, BOB)
    } else {
        (BOB, ANN)
    };
    assert_eq!(owner, account(winner));
    assert_eq!(
        as_(winner, || names.status(&alice())).unwrap(),
        Status::Owned {
            owner,
            epoch: 0,
            stable: true
        }
    );
    assert_eq!(
        as_(loser, || names.status(&alice())).unwrap(),
        Status::Lost { owner, epoch: 0 }
    );
    // A bystander sees the owner, not a loss.
    assert!(matches!(
        as_(CAT, || names.status(&alice())).unwrap(),
        Status::Owned { .. }
    ));
    assert!(as_(loser, || names.claim(alice(), 3)).is_err());
    assert!(as_(CAT, || names.claim(alice(), 3)).is_err());
    assert_eq!(names.claimants(&alice()).unwrap().len(), 2);
}

#[test]
#[serial]
fn the_winner_is_the_lowest_order_not_the_first_claim() {
    // Two registries, the claims made in opposite orders, resolved once both
    // are there: the same account wins both.
    let name = name_of(alice().as_bytes());
    let lowest = if order(&claim_ref(&name, 0, &account(ANN)), false)
        < order(&claim_ref(&name, 0, &account(BOB)), false)
    {
        account(ANN)
    } else {
        account(BOB)
    };
    for first in [ANN, BOB] {
        let mut names = setup::<Tee>();
        let second = if first == ANN { BOB } else { ANN };
        as_(first, || names.claim(alice(), 1)).unwrap();
        as_(second, || names.claim(alice(), 2)).unwrap();
        let owner = as_(tee(), || names.resolve(&alice())).unwrap();
        assert_eq!(owner, Some(lowest), "{first:?} claimed first");
    }
}

#[test]
#[serial]
fn a_pending_claim_is_withdrawn_and_a_released_name_is_claimed_at_the_next_epoch() {
    let mut names = setup::<Tee>();
    names.claim(alice(), 1).unwrap();
    assert!(names.withdraw(&alice()).unwrap());
    assert!(!names.withdraw(&alice()).unwrap());
    assert_eq!(names.status(&alice()).unwrap(), Status::Free);
    assert!(names.claimants(&alice()).unwrap().is_empty());

    names.claim(alice(), 1).unwrap();
    let _owner = as_(tee(), || names.resolve(&alice())).unwrap();
    assert!(
        names.withdraw(&alice()).is_err(),
        "an owner releases instead"
    );
    assert!(as_(BOB, || names.release(&alice())).is_err());

    // Releasing asks; the name is Ann's until the TEE answers.
    names.release(&alice()).unwrap();
    assert_eq!(names.owner_of(&alice()).unwrap(), Some(account(ANN)));
    assert_eq!(as_(tee(), || names.resolve(&alice())).unwrap(), None);
    assert_eq!(names.status(&alice()).unwrap(), Status::Free);
    assert_eq!(names.owner_of(&alice()).unwrap(), None);

    as_(BOB, || names.claim(alice(), 2)).unwrap();
    let bobs = names.claimants(&alice()).unwrap();
    assert!(bobs.contains(&(
        account(BOB),
        Claim {
            value: 2,
            epoch: 1,
            release: false
        }
    )));
    assert_eq!(
        as_(tee(), || names.resolve(&alice())).unwrap(),
        Some(account(BOB))
    );
    assert_eq!(
        names.status(&alice()).unwrap(),
        Status::Owned {
            owner: account(BOB),
            epoch: 1,
            stable: true
        },
        "Ann's released claim is from an earlier epoch: she lost nothing"
    );
    assert_eq!(names.value_of(&alice()).unwrap(), Some(2));
    // Grant, vacancy, grant.
    assert_eq!(verdict_count(&names), 3);
    assert_every_owned_entry_is_bound();
    assert_every_shared_entity_is_bound();
}

#[test]
#[serial]
fn a_member_cannot_resolve() {
    let mut names = setup::<Tee>();
    names.claim(alice(), 1).unwrap();
    assert!(names.resolve(&alice()).is_err());
    assert!(names.resolve_all_pending(10).is_err());
    assert_eq!(verdict_count(&names), 0);
    assert!(!names.verdicts.may_write());
}

#[test]
#[serial]
fn a_verdict_signed_by_a_member_is_refused_on_apply() {
    let mut names = setup::<Tee>();
    names.claim(alice(), 1).unwrap();
    as_(BOB, || names.claim(alice(), 2)).unwrap();
    let owner = as_(tee(), || names.resolve(&alice())).unwrap().unwrap();
    let rival = if owner == account(ANN) { BOB } else { ANN };

    // The verdict a patched member's node would send: the rival granted at a
    // later epoch, at the id such a verdict is stored under.
    let member = SigningKey::from_bytes(&[0x4D; 32]);
    let name = name_of(alice().as_bytes());
    let forged = super::Verdict::grant(&name, 5, account(rival));
    let anchor = names.verdicts.element().id();
    let id = names
        .verdicts
        .read()
        .unwrap()
        .unwrap()
        .entry_id(&forged.key(&name));
    let data = borsh::to_vec(&(forged, forged.key(&name))).unwrap();
    let action =
        build_signed_member_action(true, id, anchor, data, env::time_now(), &member, vec![]);
    let result = MainInterface::apply_action(action, &apply_ctx_for(account_of_key(&member)));
    assert!(
        matches!(result, Err(StorageError::InvalidSignature)),
        "got {result:?}"
    );
    assert_eq!(names.owner_of(&alice()).unwrap(), Some(owner));
    assert_eq!(verdict_count(&names), 1);
}

#[test]
#[serial]
fn a_claim_forged_for_another_account_is_refused_on_apply() {
    let mut names = setup::<Tee>();
    let attacker = SigningKey::from_bytes(&[0x4E; 32]);
    // Signed by the attacker, claiming to be Bob's, at Bob's id, in the bytes
    // an honest claim there is stored as, so only the signature is wrong.
    let bobs = names.claims.entry_id_of(&account(BOB), &alice());
    let claim = Claim {
        value: 9_u64,
        epoch: 0,
        release: false,
    };
    let data = map_entry_bytes(bobs, &alice(), &claim);
    let as_bob = under(
        (*names.claims).element().id(),
        create_signed_user_add_action(&attacker, account(BOB), bobs, data.clone(), env::time_now()),
    );
    let result = MainInterface::apply_action(as_bob, &apply_ctx_for(account_of_key(&attacker)));
    assert!(
        matches!(result, Err(StorageError::InvalidSignature)),
        "got {result:?}"
    );

    // The attacker's own claim, parked at Bob's id.
    let own = under(
        (*names.claims).element().id(),
        create_signed_user_add_action(
            &attacker,
            account_of_key(&attacker),
            bobs,
            data,
            env::time_now(),
        ),
    );
    let result = MainInterface::apply_action(own, &apply_ctx_for(account_of_key(&attacker)));
    assert!(
        matches!(result, Err(StorageError::ActionNotAllowed(_))),
        "got {result:?}"
    );

    assert!(names.claimants(&alice()).unwrap().is_empty());
    assert_eq!(as_(BOB, || names.status(&alice())).unwrap(), Status::Free);
    // Nothing was planted, so Bob's own claim still lands.
    as_(BOB, || names.claim(alice(), 2)).unwrap();
    assert_eq!(names.claimants(&alice()).unwrap().len(), 1);
}

#[test]
#[serial]
fn with_no_authority_a_contest_is_reported_and_never_owned() {
    let mut names = setup::<NoAuthority>();
    names.claim(alice(), 1).unwrap();
    assert_eq!(
        names.status(&alice()).unwrap(),
        Status::Pending {
            claimants: 1,
            mine: true
        }
    );
    as_(BOB, || names.claim(alice(), 2)).unwrap();
    let Status::Contested { claimants } = names.status(&alice()).unwrap() else {
        panic!("two claimants and no authority is a contest");
    };
    assert_eq!(
        claimants.into_iter().collect::<BTreeSet<_>>(),
        BTreeSet::from([account(ANN), account(BOB)])
    );
    assert_eq!(names.my_pending().unwrap(), [alice()]);

    assert_eq!(names.owner_of(&alice()).unwrap(), None);
    assert!(names.resolve(&alice()).is_err());
    assert!(as_(tee(), || names.resolve(&alice())).is_err());
    assert!(names.release(&alice()).is_err());
    assert!(names.withdraw(&alice()).unwrap());
    assert!(matches!(
        names.status(&alice()).unwrap(),
        Status::Pending {
            claimants: 1,
            mine: false
        }
    ));
}

#[test]
#[serial]
fn only_an_admin_resolves_and_admins_appoint_admins() {
    let mut names = setup::<Admin>();
    assert_eq!(names.admins(), BTreeSet::from([account(ANN)]));
    as_(BOB, || names.claim(alice(), 2)).unwrap();

    assert!(as_(BOB, || names.resolve(&alice())).is_err());
    assert!(as_(tee(), || names.resolve(&alice())).is_err());
    assert_eq!(names.resolve(&alice()).unwrap(), Some(account(BOB)));

    assert!(as_(BOB, || names.set_admins(BTreeSet::from([account(BOB)]))).is_err());
    names.set_admins(BTreeSet::from([account(CAT)])).unwrap();
    as_(BOB, || names.claim("bob".to_owned(), 3)).unwrap();
    assert!(names.resolve(&"bob".to_owned()).is_err());
    assert_eq!(
        as_(CAT, || names.resolve(&"bob".to_owned())).unwrap(),
        Some(account(BOB))
    );
    assert_every_owned_entry_is_bound();
    assert_every_shared_entity_is_bound();
}

#[test]
#[serial]
fn the_sweep_resolves_every_pending_name_within_its_budget() {
    let mut names = setup::<Tee>();
    for (who, name) in [(ANN, "a"), (BOB, "b"), (CAT, "c"), (BOB, "a")] {
        as_(who, || names.claim(name.to_owned(), 1)).unwrap();
    }
    assert_eq!(as_(tee(), || names.resolve_all_pending(2)).unwrap(), 2);
    assert_eq!(as_(tee(), || names.resolve_all_pending(2)).unwrap(), 1);
    assert_eq!(as_(tee(), || names.resolve_all_pending(2)).unwrap(), 0);
    for name in ["a", "b", "c"] {
        assert!(
            names.owner_of(&name.to_owned()).unwrap().is_some(),
            "{name}"
        );
    }
    assert_eq!(verdict_count(&names), 3);
}

#[test]
#[serial]
fn a_state_field_s_halves_take_ids_from_its_name() {
    let mut names = setup::<Tee>();
    names.reassign_deterministic_id("names");
    assert_eq!(
        names.claims.element().id(),
        compute_collection_id(None, "__registry_claims_names")
    );
    assert_eq!(
        names.verdicts.element().id(),
        tee_only_id("__registry_verdicts_names")
    );
}
