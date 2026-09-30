//! The writer set of a `SharedStorage` cell, folded from its rotations.
//!
//! A rotation is a signed governance op in the context's group; this module owns
//! only the fold, so every reader of those ops reaches the same writer set.

use std::collections::{BTreeMap, BTreeSet};

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
    /// The signer's nonce: only a tie-break between concurrent steps.
    pub nonce: u64,
    /// The set after the rotation.
    pub new: Writers,
    /// The key that signed the op.
    pub signer: PublicKey,
    /// The account that key spoke for when the op applied.
    pub signer_account: AccountId,
    /// The op's id.
    pub id: [u8; 32],
    /// The ids of the cell's other steps in this op's causal past.
    pub seen: BTreeSet<[u8; 32]>,
}

impl RotationStep {
    fn is_admin_in(writers: &Writers, account: &AccountId) -> bool {
        writers
            .get(account)
            .is_some_and(|mask| mask.contains(OpMask::ADMIN))
    }

    fn removes(&self, account: &AccountId) -> bool {
        Self::is_admin_in(&self.prior, account) && !Self::is_admin_in(&self.new, account)
    }

    fn concurrent_with(&self, other: &Self) -> bool {
        !self.seen.contains(&other.id) && !other.seen.contains(&self.id)
    }
}

/// The writer set `steps` lead `cell` to, or `None` when no step takes effect.
///
/// The fold starts from the set the cell id commits to, found among the steps'
/// prior sets. A step counts only when its prior set is the set in effect in
/// its own causal past and its signer holds [`OpMask::ADMIN`] there. A step is
/// void when a concurrent step by another account removes its signer's `ADMIN`,
/// and so is every step built on a void one. From genesis, the fold follows the
/// live step built on the set in effect; of concurrent ones, the lowest
/// `(nonce, signer, id)` wins.
pub fn fold<'a>(cell: Id, steps: impl IntoIterator<Item = &'a RotationStep>) -> Option<Writers> {
    let mut steps: Vec<&RotationStep> = steps.into_iter().collect();
    // An ancestor's past is a strict subset of its descendant's, so this is a
    // causal order: every step comes after the steps it has seen.
    steps.sort_by_key(|step| (step.seen.len(), step.id));
    steps.dedup_by_key(|step| step.id);
    let genesis = steps
        .iter()
        .map(|step| &step.prior)
        .find(|prior| cell_id_binds(cell, prior))?
        .clone();

    // `parents[i]`: `None` when step `i` does not count, else the step it is built
    // on (`Some(None)` for genesis).
    let mut parents: Vec<Option<Option<usize>>> = Vec::with_capacity(steps.len());
    for (i, step) in steps.iter().enumerate() {
        let past: Vec<usize> = (0..i)
            .filter(|&j| step.seen.contains(&steps[j].id))
            .collect();
        let head = head_of(&steps, &parents, &past);
        let in_effect = head.map_or(&genesis, |j| &steps[j].new);
        let counts = step.prior == *in_effect
            && RotationStep::is_admin_in(&step.prior, &step.signer_account);
        parents.push(counts.then_some(head));
    }
    let everything: Vec<usize> = (0..steps.len()).collect();
    head_of(&steps, &parents, &everything).map(|head| steps[head].new.clone())
}

/// The step whose set is in effect over the steps `cut` (causally ordered and
/// closed under `seen`), or `None` for the genesis set.
fn head_of(
    steps: &[&RotationStep],
    parents: &[Option<Option<usize>>],
    cut: &[usize],
) -> Option<usize> {
    let counted: Vec<usize> = cut
        .iter()
        .copied()
        .filter(|&i| parents[i].is_some())
        .collect();
    let void = |step: &RotationStep| {
        counted.iter().any(|&r| {
            let remover = steps[r];
            remover.signer_account != step.signer_account
                && remover.concurrent_with(step)
                && remover.removes(&step.signer_account)
        })
    };
    // Following only live steps also leaves out every step built on a void one.
    // The live steps built on one set are concurrent: one a later step had seen
    // would have been the set in effect there instead.
    let mut head = None;
    while let Some(next) = counted
        .iter()
        .copied()
        .filter(|&i| parents[i] == Some(head) && !void(steps[i]))
        .min_by_key(|&i| (steps[i].nonce, steps[i].signer, steps[i].id))
    {
        head = Some(next);
    }
    head
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

    /// Step `id` by `signer` from `prior` to `new`, having seen the steps `seen`.
    fn step(id: u8, signer: u8, prior: &[u8], new: &[u8], nonce: u64, seen: &[u8]) -> RotationStep {
        RotationStep {
            prior: set(prior),
            nonce,
            new: set(new),
            signer: key(signer),
            signer_account: acct(signer),
            id: [id; 32],
            seen: seen.iter().map(|&s| [s; 32]).collect(),
        }
    }

    #[test]
    fn rotations_apply_as_a_chain_from_genesis() {
        let steps = [
            step(2, 0xBB, &[0xAA, 0xBB], &[0xBB], 20, &[1]),
            step(1, 0xAA, &[0xAA], &[0xAA, 0xBB], 10, &[]),
        ];
        assert_eq!(fold(cell(), &steps), Some(set(&[0xBB])));
        assert_eq!(
            fold(cell(), &steps[..1]),
            None,
            "a step from a set never in effect"
        );
    }

    #[test]
    fn sequential_steps_apply_in_causal_order_whatever_their_nonces() {
        let steps = [
            step(1, 0xAA, &[0xAA], &[0xAA, 0xBB], 20, &[]),
            step(2, 0xAA, &[0xAA, 0xBB], &[0xAA, 0xCC], 5, &[1]),
        ];
        assert_eq!(fold(cell(), &steps), Some(set(&[0xAA, 0xCC])));
    }

    #[test]
    fn a_step_counts_only_from_the_set_in_effect_in_its_own_past() {
        let steps = [
            step(1, 0xAA, &[0xAA], &[0xAA, 0xBB], 10, &[]),
            // Alice had seen her first step, and steps from genesis again.
            step(2, 0xAA, &[0xAA], &[0xAA, 0xCC], 30, &[1]),
        ];
        assert_eq!(fold(cell(), &steps), Some(set(&[0xAA, 0xBB])));
    }

    #[test]
    fn a_copy_of_a_step_does_not_roll_the_set_back() {
        let first = step(1, 0xAA, &[0xAA], &[0xBB], 10, &[]);
        let back = step(2, 0xBB, &[0xBB], &[0xAA], 20, &[1]);
        // The first step again, concurrent with it: the two tie, and the lower id
        // is the one the chain continues from.
        let copy = step(3, 0xAA, &[0xAA], &[0xBB], 10, &[]);
        assert_eq!(fold(cell(), [&first, &back, &copy]), Some(set(&[0xAA])));
        // Made after both, it is a new rotation from the set in effect.
        let again = step(3, 0xAA, &[0xAA], &[0xBB], 10, &[1, 2]);
        assert_eq!(fold(cell(), [&first, &back, &again]), Some(set(&[0xBB])));
    }

    #[test]
    fn of_concurrent_rotations_from_one_set_the_lowest_nonce_signer_and_id_applies() {
        let steps = [
            step(4, 0xAA, &[0xAA], &[0xAA, 0xDD], 11, &[]),
            step(3, 0xAA, &[0xAA], &[0xAA, 0xCC], 10, &[]),
            step(2, 0xAA, &[0xAA], &[0xAA, 0xEE], 10, &[]),
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
        let mut writer_only = step(1, 0xAA, &[0xAA], &[0xEE], 10, &[]);
        writer_only.prior = [(acct(0xAA), OpMask::WRITE)].into();
        let steps = [writer_only, step(2, 0xEE, &[0xAA], &[0xEE], 11, &[])];
        assert_eq!(
            fold(cell_id(Id::new([1; 32]), &steps[0].prior), &steps),
            None
        );
    }

    #[test]
    fn a_removed_admin_cannot_override_its_removal() {
        // Bob removes Alice; Alice forks from the set before it, below his nonce.
        let genesis = &[0xAA, 0xBB];
        let cell = cell_id(Id::new([1; 32]), &set(genesis));
        let removal = step(1, 0xBB, genesis, &[0xBB], 20, &[]);
        let fork = step(2, 0xAA, genesis, &[0xAA, 0xBB, 0xEE], 5, &[]);
        assert_eq!(
            fold(cell, [&removal, &fork]),
            Some(set(&[0xBB])),
            "the removal wins whatever the nonces"
        );
        let keeps_alice = step(3, 0xBB, genesis, &[0xAA, 0xBB, 0xCC], 20, &[]);
        assert_eq!(
            fold(cell, [&keeps_alice, &fork]),
            Some(set(&[0xAA, 0xBB, 0xEE])),
            "a concurrent step that keeps her an admin does not void hers"
        );
        let counter_removal = step(4, 0xAA, genesis, &[0xAA], 10, &[]);
        assert_eq!(
            fold(cell, [&removal, &counter_removal]),
            None,
            "two admins removing each other cancel out"
        );
    }

    #[test]
    fn a_removal_voids_the_removed_admins_concurrent_steps_from_further_back() {
        let genesis = &[0xAA, 0xBB];
        let cell = cell_id(Id::new([1; 32]), &set(genesis));
        let add_carol = step(1, 0xBB, genesis, &[0xAA, 0xBB, 0xCC], 10, &[]);
        let removal = step(2, 0xBB, &[0xAA, 0xBB, 0xCC], &[0xBB, 0xCC], 20, &[1]);
        let fork = step(3, 0xAA, genesis, &[0xAA, 0xBB, 0xEE], 5, &[]);
        // Eve builds on Alice's void step; she was never removed herself.
        let built_on_it = step(
            4,
            0xEE,
            &[0xAA, 0xBB, 0xEE],
            &[0xAA, 0xBB, 0xEE, 0x11],
            6,
            &[3],
        );
        assert_eq!(
            fold(cell, [&add_carol, &removal, &fork, &built_on_it]),
            Some(set(&[0xBB, 0xCC])),
            "a step built on a void one is void"
        );
        // The admin Alice's branch added removes Bob in turn: removals on both
        // sides of the fork cancel out.
        let removes_bob = step(4, 0xEE, &[0xAA, 0xBB, 0xEE], &[0xAA, 0xEE], 6, &[3]);
        assert_eq!(
            fold(cell, [&add_carol, &removal, &fork, &removes_bob]),
            None
        );
    }

    #[test]
    fn the_genesis_set_is_the_one_the_cell_id_commits_to() {
        let genesis = step(1, 0xAA, &[0xAA], &[0xBB], 10, &[]);
        // A stranger's step from a set of their own making.
        let forged = step(2, 0xEE, &[0xEE], &[0xEE], 5, &[]);
        assert_eq!(
            fold(cell(), [&forged, &genesis]),
            Some(set(&[0xBB])),
            "found in the genuine step's prior set, never in a forged one"
        );
        assert_eq!(fold(cell(), [&forged]), None);
        assert_eq!(fold(Id::new([2; 32]), [&genesis]), None, "not a cell id");
    }
}
