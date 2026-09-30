//! The writer set of a `SharedStorage` cell, folded from its rotations.
//!
//! A rotation is a signed governance op in the context's group; this module owns
//! only the fold, so every reader of those ops reaches the same writer set.

use std::collections::BTreeMap;

use calimero_account::AccountId;
use calimero_primitives::identity::PublicKey;

use crate::address::Id;
use crate::collections::cell_id_binds;
use crate::entities::OpMask;

/// A cell's writers and what each may do.
pub type Writers = BTreeMap<AccountId, OpMask>;

/// One rotation of a cell's writer set, as its op states it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RotationStep {
    /// The set the rotation steps from.
    pub prior: Writers,
    /// The signer's nonce: the order the fold applies steps in.
    pub nonce: u64,
    /// The set after the rotation.
    pub new: Writers,
    /// The key that signed the op.
    pub signer: PublicKey,
    /// The account that key spoke for when the op applied.
    pub signer_account: AccountId,
    /// The op's id.
    pub id: [u8; 32],
}

impl RotationStep {
    fn signer_is_admin_in(&self, writers: &Writers) -> bool {
        writers
            .get(&self.signer_account)
            .is_some_and(|mask| mask.contains(OpMask::ADMIN))
    }
}

/// The writer set `steps` lead `cell` to, or `None` when no step takes effect.
///
/// The fold starts from the set the cell id commits to, found among the steps'
/// prior sets, so no step can supply a genesis of its own. From the set in
/// effect, a candidate is a step from exactly that set, at a nonce above the
/// last applied one, whose signer holds [`OpMask::ADMIN`] in it. A candidate
/// whose signer loses `ADMIN` in another account's candidate is dropped, so an
/// admin removed by a rotation cannot override it from an older cut; two admins
/// removing each other cancel out. Of the rest, the lowest `(nonce, signer, id)`
/// applies.
pub fn fold<'a>(cell: Id, steps: impl IntoIterator<Item = &'a RotationStep>) -> Option<Writers> {
    let steps: Vec<&RotationStep> = steps.into_iter().collect();
    let mut current = steps
        .iter()
        .map(|step| &step.prior)
        .find(|prior| cell_id_binds(cell, prior))?
        .clone();
    let mut last_nonce = None;
    let mut applied = false;
    loop {
        let candidates: Vec<&RotationStep> = steps
            .iter()
            .copied()
            .filter(|step| {
                step.prior == current
                    && last_nonce.is_none_or(|last| step.nonce > last)
                    && step.signer_is_admin_in(&current)
            })
            .collect();
        let Some(next) = candidates
            .iter()
            .copied()
            .filter(|step| {
                !candidates.iter().any(|other| {
                    other.signer_account != step.signer_account
                        && !step.signer_is_admin_in(&other.new)
                })
            })
            .min_by_key(|step| (step.nonce, step.signer, step.id))
        else {
            break;
        };
        current = next.new.clone();
        last_nonce = Some(next.nonce);
        applied = true;
    }
    applied.then_some(current)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collections::cell_id;

    /// The account the key `key(b)` speaks for; distinct bytes, so a mix-up
    /// between a key and an account fails a test.
    fn acct(b: u8) -> AccountId {
        let mut bytes = [b; 32];
        bytes[0] = b ^ 0xA5;
        AccountId::from(bytes)
    }

    fn key(b: u8) -> PublicKey {
        PublicKey::from([b; 32])
    }

    fn set(accounts: &[u8]) -> Writers {
        accounts.iter().map(|&b| (acct(b), OpMask::FULL)).collect()
    }

    fn cell() -> Id {
        cell_id(Id::new([1; 32]), &set(&[0xAA]))
    }

    fn step(id: u8, signer: u8, prior: &[u8], new: &[u8], nonce: u64) -> RotationStep {
        RotationStep {
            prior: set(prior),
            nonce,
            new: set(new),
            signer: key(signer),
            signer_account: acct(signer),
            id: [id; 32],
        }
    }

    #[test]
    fn rotations_apply_as_a_chain_from_genesis() {
        let steps = [
            step(2, 0xBB, &[0xAA, 0xBB], &[0xBB], 20),
            step(1, 0xAA, &[0xAA], &[0xAA, 0xBB], 10),
        ];
        assert_eq!(fold(cell(), &steps), Some(set(&[0xBB])));
        assert_eq!(
            fold(cell(), &steps[..1]),
            None,
            "a step from a set never in effect"
        );
    }

    #[test]
    fn a_copy_of_an_earlier_step_does_not_roll_the_set_back() {
        let steps = [
            step(1, 0xAA, &[0xAA], &[0xBB], 10),
            step(2, 0xBB, &[0xBB], &[0xAA], 20),
            // The first step again under another op: it steps from the set now
            // in effect, but its nonce places it before the second.
            step(3, 0xAA, &[0xAA], &[0xBB], 10),
        ];
        assert_eq!(fold(cell(), &steps), Some(set(&[0xAA])));
    }

    #[test]
    fn a_step_at_the_nonce_of_the_one_before_it_is_skipped() {
        let steps = [
            step(1, 0xAA, &[0xAA], &[0xBB], 10),
            step(2, 0xBB, &[0xBB], &[0xCC], 10),
        ];
        assert_eq!(fold(cell(), &steps), Some(set(&[0xBB])));
    }

    #[test]
    fn of_two_rotations_from_one_set_the_lowest_nonce_signer_and_id_applies() {
        let steps = [
            step(4, 0xAA, &[0xAA], &[0xAA, 0xDD], 11),
            step(3, 0xAA, &[0xAA], &[0xAA, 0xCC], 10),
            step(2, 0xAA, &[0xAA], &[0xAA, 0xEE], 10),
        ];
        let mut reversed = steps.clone();
        reversed.reverse();
        assert_eq!(fold(cell(), &steps), fold(cell(), &reversed));
        assert_eq!(
            fold(cell(), &steps),
            Some(set(&[0xAA, 0xEE])),
            "equal nonce and signer: the lower op id"
        );
    }

    #[test]
    fn a_step_whose_signer_is_not_an_admin_of_its_prior_set_is_skipped() {
        let mut writer_only = step(1, 0xAA, &[0xAA], &[0xEE], 10);
        writer_only.prior = [(acct(0xAA), OpMask::WRITE)].into();
        let steps = [writer_only, step(2, 0xEE, &[0xAA], &[0xEE], 11)];
        assert_eq!(
            fold(cell_id(Id::new([1; 32]), &steps[0].prior), &steps),
            None
        );
    }

    #[test]
    fn a_removed_admin_cannot_override_its_removal_from_an_older_cut() {
        // Bob removes Alice; Alice, forking from the set before it, backdates a
        // rotation of her own below Bob's nonce.
        let genesis = &[0xAA, 0xBB];
        let removal = step(1, 0xBB, genesis, &[0xBB], 20);
        let fork = step(2, 0xAA, genesis, &[0xAA, 0xBB, 0xEE], 10);
        let cell = cell_id(Id::new([1; 32]), &set(genesis));
        assert_eq!(
            fold(cell, [&removal, &fork]),
            Some(set(&[0xBB])),
            "the removal wins whatever the nonces"
        );
        let keeps_alice = step(3, 0xBB, genesis, &[0xAA, 0xBB, 0xCC], 20);
        assert_eq!(
            fold(cell, [&keeps_alice, &fork]),
            Some(set(&[0xAA, 0xBB, 0xEE])),
            "a sibling that keeps her an admin does not void her step"
        );
        let counter_removal = step(4, 0xAA, genesis, &[0xAA], 10);
        assert_eq!(
            fold(cell, [&removal, &counter_removal]),
            None,
            "two admins removing each other cancel out"
        );
    }

    #[test]
    fn the_genesis_set_is_the_one_the_cell_id_commits_to() {
        let genesis = step(1, 0xAA, &[0xAA], &[0xBB], 10);
        // A stranger's step from a set of their own making.
        let forged = step(2, 0xEE, &[0xEE], &[0xEE], 5);
        assert_eq!(
            fold(cell(), [&forged, &genesis]),
            Some(set(&[0xBB])),
            "found in the genuine step's prior set, never in a forged one"
        );
        assert_eq!(fold(cell(), [&forged]), None);
        assert_eq!(fold(Id::new([2; 32]), [&genesis]), None, "not a cell id");
    }
}
