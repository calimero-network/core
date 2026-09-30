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

/// Where a step or a cut stands in a cell's history.
#[derive(Clone, Debug, Eq, PartialEq)]
enum Node {
    Genesis,
    Step(usize),
    /// `after`, with both removals of each mutually removing pair applied.
    Mutual {
        after: Box<Node>,
        pairs: BTreeSet<(usize, usize)>,
    },
}

/// The writer set `steps` lead `cell` to, or `None` when no step takes effect.
///
/// The fold starts from the set the cell id commits to, found among the steps'
/// prior sets. A step counts only when its prior set is the set in effect in
/// its own causal past and its signer holds [`OpMask::ADMIN`] there. A step is
/// void when a concurrent step by another account removes its signer's `ADMIN`,
/// unless the step that granted the remover its own `ADMIN` is void. From
/// genesis the fold follows the live step built on the set in effect, the
/// lowest `(nonce, signer, id)` of concurrent ones; where none is live, the
/// removals of every pair of admins who removed each other take effect.
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

    // `parents[i]`: the node step `i` is built on, or `None` when it does not count.
    let mut parents: Vec<Option<Node>> = Vec::with_capacity(steps.len());
    for (i, step) in steps.iter().enumerate() {
        let past: Vec<usize> = (0..i)
            .filter(|&j| step.seen.contains(&steps[j].id))
            .collect();
        let (node, in_effect) = head_of(&steps, &parents, &past, &genesis);
        let counts =
            step.prior == in_effect && RotationStep::is_admin_in(&step.prior, &step.signer_account);
        parents.push(counts.then_some(node));
    }
    let everything: Vec<usize> = (0..steps.len()).collect();
    let (node, in_effect) = head_of(&steps, &parents, &everything, &genesis);
    (node != Node::Genesis).then_some(in_effect)
}

/// The node in effect over the steps `cut` (causally ordered and closed under
/// `seen`), and its writer set.
fn head_of(
    steps: &[&RotationStep],
    parents: &[Option<Node>],
    cut: &[usize],
    genesis: &Writers,
) -> (Node, Writers) {
    let counted: Vec<usize> = cut
        .iter()
        .copied()
        .filter(|&i| parents[i].is_some())
        .collect();
    let attacks = |r: usize, x: usize| {
        let (remover, step) = (steps[r], steps[x]);
        remover.signer_account != step.signer_account
            && remover.concurrent_with(step)
            && remover.removes(&step.signer_account)
    };
    let (void, power) = settle(steps, parents, &counted, attacks);

    let mut node = Node::Genesis;
    let mut in_effect = genesis.clone();
    let mut walked = vec![Node::Genesis];
    let mut applied = BTreeSet::new();
    loop {
        // The live steps built on one node are concurrent: one a later step had
        // seen would have been the set in effect there instead.
        let next = counted
            .iter()
            .copied()
            .filter(|&i| parents[i].as_ref() == Some(&node) && !void[i])
            .min_by_key(|&i| (steps[i].nonce, steps[i].signer, steps[i].id));
        if let Some(next) = next {
            node = Node::Step(next);
            in_effect = steps[next].new.clone();
            walked.push(node.clone());
            continue;
        }
        let pairs: BTreeSet<(usize, usize)> = counted
            .iter()
            .flat_map(|&r| counted.iter().map(move |&s| (r, s)))
            .filter(|&(r, s)| {
                r < s
                    && power[r]
                    && power[s]
                    && attacks(r, s)
                    && attacks(s, r)
                    && !applied.contains(&(r, s))
                    && [r, s]
                        .iter()
                        .all(|&i| parents[i].as_ref().is_some_and(|p| walked.contains(p)))
            })
            .collect();
        if pairs.is_empty() {
            return (node, in_effect);
        }
        remove_each_other(steps, &pairs, &mut in_effect);
        applied.extend(pairs.iter().copied());
        node = Node::Mutual {
            after: Box::new(node),
            pairs,
        };
        walked.push(node.clone());
    }
}

/// Which counted steps are void, and which may void others, over one cut.
///
/// A remover's power is lost only when the step that granted its signer
/// `ADMIN` is void, so the grounded answer is order-independent; a cycle of
/// such grants left undecided is one where every removal takes effect.
fn settle(
    steps: &[&RotationStep],
    parents: &[Option<Node>],
    counted: &[usize],
    attacks: impl Fn(usize, usize) -> bool,
) -> (Vec<bool>, Vec<bool>) {
    let grants: BTreeMap<usize, Option<usize>> = counted
        .iter()
        .map(|&i| (i, grant_of(steps, parents, i)))
        .collect();
    let mut void: Vec<Option<bool>> = vec![None; steps.len()];
    let mut power: Vec<Option<bool>> = vec![None; steps.len()];
    loop {
        let mut changed = false;
        for &r in counted {
            if power[r].is_none() {
                power[r] = match grants[&r] {
                    None => Some(true),
                    Some(grant) => void[grant].map(|void| !void),
                };
                changed |= power[r].is_some();
            }
        }
        for &x in counted {
            if void[x].is_none() {
                let attackers: Vec<usize> =
                    counted.iter().copied().filter(|&r| attacks(r, x)).collect();
                if attackers.iter().any(|&r| power[r] == Some(true)) {
                    void[x] = Some(true);
                } else if attackers.iter().all(|&r| power[r] == Some(false)) {
                    void[x] = Some(false);
                }
                changed |= void[x].is_some();
            }
        }
        if !changed {
            break;
        }
    }
    let power: Vec<bool> = power.iter().map(|p| p.unwrap_or(true)).collect();
    let void = counted
        .iter()
        .fold(vec![false; steps.len()], |mut out, &x| {
            out[x] = void[x].unwrap_or_else(|| counted.iter().any(|&r| power[r] && attacks(r, x)));
            out
        });
    (void, power)
}

/// The step on `i`'s chain that granted its signer `ADMIN`, or `None` when the
/// signer has held it since genesis.
fn grant_of(steps: &[&RotationStep], parents: &[Option<Node>], i: usize) -> Option<usize> {
    let account = &steps[i].signer_account;
    let mut node = parents[i].as_ref()?;
    loop {
        match node {
            Node::Genesis => return None,
            Node::Mutual { after, .. } => node = after,
            Node::Step(j) => {
                if !RotationStep::is_admin_in(&steps[*j].prior, account) {
                    return Some(*j);
                }
                node = parents[*j].as_ref()?;
            }
        }
    }
}

/// Apply both removals of each pair: each signer's entry is what the other's
/// step leaves it, and a signer removed by several keeps what they all leave it.
fn remove_each_other(
    steps: &[&RotationStep],
    pairs: &BTreeSet<(usize, usize)>,
    writers: &mut Writers,
) {
    let mut left: BTreeMap<AccountId, Option<OpMask>> = BTreeMap::new();
    for &(r, s) in pairs {
        for (removed, by) in [(steps[s], steps[r]), (steps[r], steps[s])] {
            let account = removed.signer_account;
            let kept = by.new.get(&account).copied();
            let _ = left
                .entry(account)
                .and_modify(|mask| {
                    *mask = mask.zip(kept).map(|(a, b)| a.intersection(b));
                })
                .or_insert(kept);
        }
    }
    for (account, mask) in left {
        match mask {
            Some(mask) => {
                let _ = writers.insert(account, mask);
            }
            None => {
                let _ = writers.remove(&account);
            }
        }
    }
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
            Some(Writers::new()),
            "two admins removing each other: both removals take effect"
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
        // The admin Alice's void step added removes Bob: an admin whose authority
        // comes from a void step voids nothing, so her branch has no effect.
        let removes_bob = step(4, 0xEE, &[0xAA, 0xBB, 0xEE], &[0xAA, 0xEE], 6, &[3]);
        assert_eq!(
            fold(cell, [&add_carol, &removal, &fork, &removes_bob]),
            Some(set(&[0xBB, 0xCC]))
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

    #[test]
    fn a_removed_admin_who_removes_the_remover_goes_down_with_them() {
        // Genesis: Alice and Bob administer, Carol writes.
        let mut genesis = set(&[0xAA, 0xBB]);
        let _ = genesis.insert(acct(0xCC), OpMask::WRITE);
        let cell = cell_id(Id::new([1; 32]), &genesis);
        let with = |mut writers: Writers| {
            let _ = writers.insert(acct(0xCC), OpMask::WRITE);
            writers
        };
        let rotate = |id, signer, new: Writers, nonce, seen: &[u8]| RotationStep {
            prior: genesis.clone(),
            new,
            ..step(id, signer, &[], &[], nonce, seen)
        };
        let removal = rotate(1, 0xBB, with(set(&[0xBB])), 20, &[]);
        // Alice has seen it, and answers on parents from before it.
        let answer = rotate(2, 0xAA, with(set(&[0xAA])), 10, &[]);
        let only_carol = with(Writers::new());
        assert_eq!(fold(cell, [&removal, &answer]), Some(only_carol.clone()));

        // Bob, again, having seen both, and Alice forking once more from genesis.
        let again = RotationStep {
            prior: only_carol.clone(),
            ..step(3, 0xBB, &[], &[0xBB], 30, &[1, 2])
        };
        let fork = rotate(4, 0xAA, with(set(&[0xAA, 0xEE])), 5, &[]);
        let after = RotationStep {
            prior: only_carol.clone(),
            ..step(5, 0xAA, &[], &[0xAA], 40, &[1, 2])
        };
        let steps = [removal, answer, again, fork, after];
        let mut reversed = steps.clone();
        reversed.reverse();
        assert_eq!(fold(cell, &steps), Some(only_carol.clone()));
        assert_eq!(fold(cell, &reversed), Some(only_carol));
    }
}
