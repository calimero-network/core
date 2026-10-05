//! The writer set of a `SharedStorage` cell, folded from its rotations.
//!
//! A rotation is a signed governance op in the context's group; this owns only the fold.

use std::collections::{BTreeMap, BTreeSet};

use calimero_account::AccountId;
use calimero_primitives::identity::PublicKey;

use crate::address::Id;
use crate::collections::cell_id_binds;
use crate::entities::OpMask;

/// A cell's writers and what each may do.
pub type Writers = BTreeMap<AccountId, OpMask>;

/// A cell has more rotation steps than the fold takes, so it has no answer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OverBudget;

/// Why a cell's writer set cannot be read at a cut.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WritersUnavailable {
    /// The cut is empty, incomplete or unreadable here; more data can clear it.
    Cut,
    /// The cell has more steps than the fold takes; nothing clears it.
    OverBudget,
}

/// A cell's writer set at a cut.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CellWriters {
    /// No rotation took effect: the set the cell id commits to stands.
    Genesis,
    /// The set the rotations in effect leave.
    Rotated(Writers),
}

/// The most rotations of one cell the fold takes, which bounds its cost. More gives no answer.
pub const MAX_STEPS_PER_CELL: usize = 256;

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
/// The rules are in the governance chapter; ties go to the lowest `(nonce, signer, id)`.
pub fn fold<'a>(
    cell: Id,
    steps: impl IntoIterator<Item = &'a RotationStep>,
) -> Result<Option<Writers>, OverBudget> {
    let mut steps: Vec<&RotationStep> = steps.into_iter().collect();
    // An ancestor's past is a strict subset of its descendant's, so this is a
    // causal order: every step comes after the steps it has seen.
    steps.sort_by_key(|step| (step.seen.len(), step.id));
    steps.dedup_by_key(|step| step.id);
    // Over budget there is no answer: dropping steps could roll a rotation back.
    if steps.len() > MAX_STEPS_PER_CELL {
        return Err(OverBudget);
    }
    let Some(genesis) = steps
        .iter()
        .map(|step| &step.prior)
        .find(|prior| cell_id_binds(cell, prior))
        .cloned()
    else {
        return Ok(None);
    };

    // `parents[i]` is the node step `i` is built on, `None` if it does not count; `reps[i]`
    // is its first twin, so a step built on either of two identical rotations still counts.
    let mut parents: Vec<Option<Node>> = Vec::with_capacity(steps.len());
    let mut reps: Vec<usize> = Vec::with_capacity(steps.len());
    for (i, step) in steps.iter().enumerate() {
        let past: Vec<usize> = (0..i)
            .filter(|&j| step.seen.contains(&steps[j].id))
            .collect();
        let (node, in_effect) = head_of(&steps, &parents, &reps, &past, &genesis);
        let counts =
            step.prior == in_effect && RotationStep::is_admin_in(&step.prior, &step.signer_account);
        let node = counts.then_some(node);
        let rep = (0..i)
            .find(|&j| parents[j].is_some() && parents[j] == node && steps[j].new == step.new)
            .unwrap_or(i);
        parents.push(node);
        reps.push(rep);
    }
    let everything: Vec<usize> = (0..steps.len()).collect();
    let (node, in_effect) = head_of(&steps, &parents, &reps, &everything, &genesis);
    Ok((node != Node::Genesis).then_some(in_effect))
}

/// The node in effect over the steps `cut` (causally ordered and closed under
/// `seen`), and its writer set.
fn head_of(
    steps: &[&RotationStep],
    parents: &[Option<Node>],
    reps: &[usize],
    cut: &[usize],
    genesis: &Writers,
) -> (Node, Writers) {
    let counted: Vec<usize> = cut
        .iter()
        .copied()
        .filter(|&i| parents[i].is_some())
        .collect();
    // The twins of each grant that the step had seen: its `ADMIN` rests on all of them,
    // so it is lost only when every one is void.
    let grant_from: BTreeMap<usize, Vec<usize>> = counted
        .iter()
        .filter_map(|&x| {
            let grant = grant_of(steps, parents, x)?;
            let seen: Vec<usize> = counted
                .iter()
                .copied()
                .filter(|&t| reps[t] == grant && steps[x].seen.contains(&steps[t].id))
                .collect();
            Some((x, if seen.is_empty() { vec![grant] } else { seen }))
        })
        .collect();
    let grants: BTreeMap<usize, Option<usize>> = counted
        .iter()
        .map(|&i| (i, grant_from.get(&i).and_then(|from| from.first().copied())))
        .collect();
    // A step and every step its signer's `ADMIN` was granted by, transitively.
    let chain = |i: usize| core::iter::successors(Some(i), |at| grants.get(at).copied().flatten());
    let removes = |r: usize, x: usize| {
        let (remover, step) = (steps[r], steps[x]);
        remover.signer_account != step.signer_account
            && remover.concurrent_with(step)
            && remover.removes(&step.signer_account)
    };
    // The steps of `s`'s chain that `r` removes the signer of.
    let hits = |r: usize, s: usize| chain(s).filter(move |&x| removes(r, x));
    // Two concurrent steps each removing an admin the other's chain rests on.
    let races: Vec<(usize, usize)> = counted
        .iter()
        .flat_map(|&r| counted.iter().map(move |&s| (r, s)))
        .filter(|&(r, s)| {
            r < s
                && steps[r].concurrent_with(steps[s])
                && hits(r, s).next().is_some()
                && hits(s, r).next().is_some()
        })
        .collect();
    // In a race, a step removing a genesis admin does not apply.
    let protected: BTreeSet<usize> = races
        .iter()
        .flat_map(|&(r, s)| [(r, s), (s, r)])
        .filter(|&(a, b)| {
            hits(a, b).any(|x| RotationStep::is_admin_in(genesis, &steps[x].signer_account))
        })
        .map(|(a, _)| a)
        .collect();
    let attacks = |r: usize, x: usize| !protected.contains(&r) && removes(r, x);
    let (void, power) = settle(&grant_from, &counted, &protected, attacks);

    let mut node = Node::Genesis;
    let mut in_effect = genesis.clone();
    let mut applied = BTreeSet::new();
    loop {
        // The live steps built on one node are concurrent: one a later step had
        // seen would have been the set in effect there instead.
        let next = counted
            .iter()
            .copied()
            .filter(|&i| parents[i].as_ref() == Some(&node) && !void[&i])
            .min_by_key(|&i| (steps[i].nonce, steps[i].signer, steps[i].id));
        if let Some(next) = next {
            node = Node::Step(reps[next]);
            in_effect = steps[next].new.clone();
            continue;
        }
        let pairs: BTreeSet<(usize, usize)> = races
            .iter()
            .copied()
            .filter(|&(r, s)| {
                [r, s]
                    .iter()
                    .all(|i| power[i] && void[i] && !protected.contains(i))
                    && !applied.contains(&(r, s))
            })
            .collect();
        if pairs.is_empty() {
            return (node, in_effect);
        }
        let removals = pairs.iter().flat_map(|&(r, s)| {
            [(r, s), (s, r)].into_iter().flat_map(move |(by, of)| {
                hits(by, of).map(move |x| {
                    let account = steps[x].signer_account;
                    (account, steps[by].new.get(&account).copied())
                })
            })
        });
        remove_each_other(removals, &mut in_effect);
        applied.extend(pairs.iter().copied());
        node = Node::Mutual {
            after: Box::new(node),
            pairs,
        };
    }
}

/// Which counted steps are void, and which may still void others. Of a cycle of
/// three or more, every step is void.
fn settle(
    grants: &BTreeMap<usize, Vec<usize>>,
    counted: &[usize],
    protected: &BTreeSet<usize>,
    attacks: impl Fn(usize, usize) -> bool,
) -> (BTreeMap<usize, bool>, BTreeMap<usize, bool>) {
    let mut void: BTreeMap<usize, Option<bool>> = counted
        .iter()
        .map(|&i| (i, protected.contains(&i).then_some(true)))
        .collect();
    let mut power: BTreeMap<usize, Option<bool>> = counted.iter().map(|&i| (i, None)).collect();
    // Whether the steps `x`'s `ADMIN` rests on are void: no if any is live, yes if all are void.
    let grant_void = |void: &BTreeMap<usize, Option<bool>>, x: usize| {
        let Some(from) = grants.get(&x) else {
            return Some(false);
        };
        let states: Vec<Option<bool>> = from
            .iter()
            .map(|g| void.get(g).copied().flatten())
            .collect();
        if states.contains(&Some(false)) {
            Some(false)
        } else if states.iter().all(|state| *state == Some(true)) {
            Some(true)
        } else {
            None
        }
    };
    loop {
        let mut changed = false;
        for &r in counted {
            if power[&r].is_none() {
                let known = grant_void(&void, r).map(|void| !void);
                changed |= known.is_some();
                let _ = power.insert(r, known);
            }
        }
        for &x in counted {
            if void[&x].is_none() {
                let attackers: Vec<usize> =
                    counted.iter().copied().filter(|&r| attacks(r, x)).collect();
                let known = if grant_void(&void, x) == Some(true)
                    || attackers.iter().any(|r| power[r] == Some(true))
                {
                    Some(true)
                } else if grant_void(&void, x) == Some(false)
                    && attackers.iter().all(|r| power[r] == Some(false))
                {
                    Some(false)
                } else {
                    None
                };
                changed |= known.is_some();
                let _ = void.insert(x, known);
            }
        }
        if !changed {
            break;
        }
    }
    let power: BTreeMap<usize, bool> = power.iter().map(|(&i, p)| (i, p.unwrap_or(true))).collect();
    // Causal order: a grant is decided before the steps it granted.
    let mut settled: BTreeMap<usize, bool> = BTreeMap::new();
    for &x in counted {
        let decided = void[&x].unwrap_or_else(|| {
            grants.get(&x).is_some_and(|from| {
                from.iter()
                    .all(|g| settled.get(g).copied().unwrap_or(false))
            }) || counted.iter().any(|&r| power[&r] && attacks(r, x))
        });
        let _ = settled.insert(x, decided);
    }
    (settled, power)
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

/// Apply every racing pair's removals: a removed signer keeps what its remover's step left.
fn remove_each_other(
    removals: impl IntoIterator<Item = (AccountId, Option<OpMask>)>,
    writers: &mut Writers,
) {
    let mut left: BTreeMap<AccountId, Option<OpMask>> = BTreeMap::new();
    for (account, kept) in removals {
        let _ = left
            .entry(account)
            .and_modify(|mask| *mask = mask.zip(kept).map(|(a, b)| a.intersection(b)))
            .or_insert(kept);
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

    fn fold_in<'a>(cell: Id, steps: impl IntoIterator<Item = &'a RotationStep>) -> Option<Writers> {
        fold(cell, steps).expect("within the budget")
    }
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
        assert_eq!(fold_in(cell(), &steps), Some(set(&[0xBB])));
        assert_eq!(
            fold_in(cell(), &steps[..1]),
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
        assert_eq!(fold_in(cell(), &steps), Some(set(&[0xAA, 0xCC])));
    }

    #[test]
    fn a_step_counts_only_from_the_set_in_effect_in_its_own_past() {
        let steps = [
            step(1, 0xAA, &[0xAA], &[0xAA, 0xBB], 10, &[]),
            // Alice had seen her first step, and steps from genesis again.
            step(2, 0xAA, &[0xAA], &[0xAA, 0xCC], 30, &[1]),
        ];
        assert_eq!(fold_in(cell(), &steps), Some(set(&[0xAA, 0xBB])));
    }

    #[test]
    fn a_copy_of_a_step_does_not_roll_the_set_back() {
        let first = step(1, 0xAA, &[0xAA], &[0xBB], 10, &[]);
        let back = step(2, 0xBB, &[0xBB], &[0xAA], 20, &[1]);
        // The first step again, concurrent with it: the two tie, and the lower id
        // is the one the chain continues from.
        let copy = step(3, 0xAA, &[0xAA], &[0xBB], 10, &[]);
        assert_eq!(fold_in(cell(), [&first, &back, &copy]), Some(set(&[0xAA])));
        // Whichever of the two wins the tie, the step built on the other still counts.
        let lower = step(0, 0xAA, &[0xAA], &[0xBB], 10, &[]);
        assert_eq!(fold_in(cell(), [&first, &back, &lower]), Some(set(&[0xAA])));
        // Made after both, it is a new rotation from the set in effect.
        let again = step(3, 0xAA, &[0xAA], &[0xBB], 10, &[1, 2]);
        assert_eq!(fold_in(cell(), [&first, &back, &again]), Some(set(&[0xBB])));
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
        assert_eq!(fold_in(cell(), &steps), fold_in(cell(), &reversed));
        assert_eq!(
            fold_in(cell(), &steps),
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
            fold_in(cell_id(Id::new([1; 32]), &steps[0].prior), &steps),
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
            fold_in(cell, [&removal, &fork]),
            Some(set(&[0xBB])),
            "the removal wins whatever the nonces, of a genesis admin too"
        );
        let keeps_alice = step(3, 0xBB, genesis, &[0xAA, 0xBB, 0xCC], 20, &[]);
        assert_eq!(
            fold_in(cell, [&keeps_alice, &fork]),
            Some(set(&[0xAA, 0xBB, 0xEE])),
            "a concurrent step that keeps her an admin does not void hers"
        );
    }

    #[test]
    fn a_genesis_admin_keeps_admin_through_a_mutual_removal() {
        // The cell's only two admins, both in its genesis set.
        let genesis = &[0xAA, 0xBB];
        let cell = cell_id(Id::new([1; 32]), &set(genesis));
        let removal = step(1, 0xBB, genesis, &[0xBB], 20, &[]);
        let answer = step(2, 0xAA, genesis, &[0xAA], 10, &[]);
        assert_eq!(fold_in(cell, [&removal, &answer]), None, "both keep ADMIN");
        let later = step(3, 0xBB, genesis, &[0xAA, 0xBB, 0xDD], 30, &[1, 2]);
        assert_eq!(
            fold_in(cell, [&removal, &answer, &later]),
            Some(set(&[0xAA, 0xBB, 0xDD])),
            "and the cell stays rotatable"
        );

        // Bob is a genesis admin, Alice one he added.
        let cell = cell_id(Id::new([1; 32]), &set(&[0xBB]));
        let add_alice = step(1, 0xBB, &[0xBB], genesis, 5, &[]);
        let removal = step(2, 0xBB, genesis, &[0xBB], 20, &[1]);
        let answer = step(3, 0xAA, genesis, &[0xAA], 10, &[1]);
        assert_eq!(
            fold_in(cell, [&add_alice, &removal, &answer]),
            Some(set(&[0xBB])),
            "the added admin is removed, the genesis admin keeps ADMIN"
        );
    }

    #[test]
    fn a_removal_voids_the_removed_admins_concurrent_steps_from_further_back() {
        // Bob is a genesis admin; he added Alice, then Carol, then removed Alice.
        let cell = cell_id(Id::new([1; 32]), &set(&[0xBB]));
        let add_alice = step(1, 0xBB, &[0xBB], &[0xAA, 0xBB], 1, &[]);
        let add_carol = step(2, 0xBB, &[0xAA, 0xBB], &[0xAA, 0xBB, 0xCC], 10, &[1]);
        let removal = step(3, 0xBB, &[0xAA, 0xBB, 0xCC], &[0xBB, 0xCC], 20, &[1, 2]);
        let fork = step(4, 0xAA, &[0xAA, 0xBB], &[0xAA, 0xBB, 0xEE], 5, &[1]);
        // Eve builds on Alice's void step; she was never removed herself.
        let built_on_it = step(
            5,
            0xEE,
            &[0xAA, 0xBB, 0xEE],
            &[0xAA, 0xBB, 0xEE, 0x11],
            6,
            &[1, 4],
        );
        assert_eq!(
            fold_in(
                cell,
                [&add_alice, &add_carol, &removal, &fork, &built_on_it]
            ),
            Some(set(&[0xBB, 0xCC])),
            "a step built on a void one is void"
        );
        // The admin Alice's void step added removes Bob: an admin whose authority
        // comes from a void step voids nothing, so her branch has no effect.
        let removes_bob = step(5, 0xEE, &[0xAA, 0xBB, 0xEE], &[0xAA, 0xEE], 6, &[1, 4]);
        assert_eq!(
            fold_in(
                cell,
                [&add_alice, &add_carol, &removal, &fork, &removes_bob]
            ),
            Some(set(&[0xBB, 0xCC]))
        );
    }

    /// All orders of `steps`' rotations and reversals.
    fn in_every_order(cell: Id, steps: &[RotationStep]) -> Option<Writers> {
        let first = fold_in(cell, steps);
        for shift in 0..steps.len() {
            let mut order = steps.to_vec();
            order.rotate_left(shift);
            assert_eq!(fold_in(cell, &order), first);
            order.reverse();
            assert_eq!(fold_in(cell, &order), first);
        }
        first
    }

    #[test]
    fn a_removal_by_a_delegate_stands_against_the_removed_admins_delegate() {
        // Bob, a genesis admin, added Alice. Bob adds Carol, Carol removes Alice;
        // Alice forks from before Carol, adds Eve, and Eve removes Bob.
        let ab = &[0xAA, 0xBB];
        let delegates = |genesis: &[u8], first: u8| {
            [
                step(2, 0xBB, ab, &[0xAA, 0xBB, 0xCC], 10, &[first]),
                step(3, 0xCC, &[0xAA, 0xBB, 0xCC], &[0xBB, 0xCC], 20, &[first, 2]),
                step(4, 0xAA, ab, &[0xAA, 0xBB, 0xEE], 5, &[first]),
                step(5, 0xEE, &[0xAA, 0xBB, 0xEE], &[0xAA, 0xEE], 6, &[first, 4]),
                step(first, genesis[0], genesis, ab, 1, &[]),
            ]
        };
        let cell = cell_id(Id::new([1; 32]), &set(&[0xBB]));
        assert_eq!(
            in_every_order(cell, &delegates(&[0xBB], 1)),
            Some(set(&[0xBB, 0xCC])),
            "Alice removed, Bob and Carol kept, Eve's steps void"
        );

        // Both in the genesis set: neither is removed by the race, and the two
        // additions compete as concurrent steps do.
        let cell = cell_id(Id::new([1; 32]), &set(ab));
        let steps = &delegates(ab, 1)[..4];
        assert_eq!(in_every_order(cell, steps), Some(set(&[0xAA, 0xBB, 0xEE])));

        // Neither Alice nor Bob in the genesis set: both removals take effect.
        let genesis = &[0x99];
        let cell = cell_id(Id::new([1; 32]), &set(genesis));
        let mut steps = delegates(genesis, 1).to_vec();
        steps[4] = step(1, 0x99, genesis, &[0x99, 0xAA, 0xBB], 1, &[]);
        for step in &mut steps[..4] {
            let _ = step.prior.insert(acct(0x99), OpMask::FULL);
            let _ = step.new.insert(acct(0x99), OpMask::FULL);
        }
        assert_eq!(in_every_order(cell, &steps), Some(set(&[0x99])));
    }

    #[test]
    fn identical_concurrent_rotations_do_not_orphan_a_step_built_on_either() {
        // Alice and Bob, both genesis admins, each add Carol at once. Alice's copy
        // wins the tie; Bob then builds on his own.
        let ab = &[0xAA, 0xBB];
        let cell = cell_id(Id::new([1; 32]), &set(ab));
        let abc = &[0xAA, 0xBB, 0xCC];
        let alice = step(1, 0xAA, ab, abc, 10, &[]);
        let bob = step(2, 0xBB, ab, abc, 10, &[]);
        let on_bobs = step(3, 0xBB, abc, &[0xAA, 0xBB, 0xCC, 0xDD], 20, &[2]);
        assert_eq!(
            in_every_order(cell, &[alice, bob, on_bobs]),
            Some(set(&[0xAA, 0xBB, 0xCC, 0xDD]))
        );
    }

    #[test]
    fn a_step_built_on_a_twin_does_not_lose_its_grant_when_the_other_twin_is_void() {
        // Alice and Bob each add Carol at once, Rem removes Alice, and Carol
        // rotates having seen only Bob's copy.
        let genesis = &[0xAA, 0xBB, 0xCC];
        let cell = cell_id(Id::new([1; 32]), &set(genesis));
        let with_dd = &[0xAA, 0xBB, 0xCC, 0xDD];
        let alice = step(1, 0xAA, genesis, with_dd, 10, &[]);
        let bob = step(2, 0xBB, genesis, with_dd, 10, &[]);
        let removes_alice = step(3, 0xCC, genesis, &[0xBB, 0xCC], 30, &[]);
        // Dave, added by both copies, rotates having seen only Bob's.
        let dave = step(4, 0xDD, with_dd, &[0xBB, 0xDD, 0xEE], 40, &[2]);
        let steps = [alice, bob, removes_alice, dave];
        assert_eq!(
            in_every_order(cell, &steps),
            Some(set(&[0xBB, 0xDD, 0xEE])),
            "Bob's grant stands although Alice is removed"
        );
    }

    #[test]
    fn steps_not_closed_under_seen_are_folded_without_panicking() {
        let cell = cell_id(Id::new([1; 32]), &set(&[0xAA]));
        let first = step(1, 0xAA, &[0xAA], &[0xAA, 0xBB], 1, &[]);
        let second = step(2, 0xBB, &[0xAA, 0xBB], &[0xBB], 2, &[1]);
        // Names the second step but not the first it rests on.
        let third = step(3, 0xBB, &[0xBB], &[0xBB, 0xCC], 3, &[2]);
        let _ = fold_in(cell, [&first, &second, &third]);
    }

    #[test]
    fn a_cycle_of_three_concurrent_removals_removes_nobody() {
        let cell = cell_id(Id::new([1; 32]), &set(&[0x99]));
        let all = &[0x99, 0xAA, 0xBB, 0xCC];
        let add = step(1, 0x99, &[0x99], all, 1, &[]);
        let a_removes_b = step(2, 0xAA, all, &[0x99, 0xAA, 0xCC], 10, &[1]);
        let b_removes_c = step(3, 0xBB, all, &[0x99, 0xAA, 0xBB], 10, &[1]);
        let c_removes_a = step(4, 0xCC, all, &[0x99, 0xBB, 0xCC], 10, &[1]);
        assert_eq!(
            in_every_order(cell, &[add, a_removes_b, b_removes_c, c_removes_a]),
            Some(set(all))
        );
    }

    #[test]
    fn a_cell_over_the_step_budget_has_no_answer_and_at_the_budget_has_one() {
        let cell = cell_id(Id::new([1; 32]), &set(&[0xAA]));
        let (one, two) = (&[0xAA][..], &[0xAA, 0xBB][..]);
        let chain = |count: usize| -> Vec<RotationStep> {
            // Each step sees every step before it, flipping between the two sets.
            let steps: Vec<RotationStep> = (0..count as u32)
                .map(|i| {
                    let (prior, new) = if i % 2 == 0 { (one, two) } else { (two, one) };
                    let mut step = step(0, 0xAA, prior, new, u64::from(i), &[]);
                    step.id = [i as u8; 32];
                    step.id[1] = (i >> 8) as u8;
                    step
                })
                .collect();
            steps
                .iter()
                .enumerate()
                .map(|(i, step)| RotationStep {
                    seen: steps[..i].iter().map(|earlier| earlier.id).collect(),
                    ..step.clone()
                })
                .collect()
        };
        let expected = if MAX_STEPS_PER_CELL.is_multiple_of(2) {
            one
        } else {
            two
        };
        assert_eq!(
            fold_in(cell, &chain(MAX_STEPS_PER_CELL)),
            Some(set(expected))
        );
        assert_eq!(fold(cell, &chain(MAX_STEPS_PER_CELL + 1)), Err(OverBudget));
    }

    #[test]
    fn the_genesis_set_is_the_one_the_cell_id_commits_to() {
        let genesis = step(1, 0xAA, &[0xAA], &[0xBB], 10, &[]);
        // A stranger's step from a set of their own making.
        let forged = step(2, 0xEE, &[0xEE], &[0xEE], 5, &[]);
        assert_eq!(
            fold_in(cell(), [&forged, &genesis]),
            Some(set(&[0xBB])),
            "found in the genuine step's prior set, never in a forged one"
        );
        assert_eq!(fold_in(cell(), [&forged]), None);
        assert_eq!(fold_in(Id::new([2; 32]), [&genesis]), None, "not a cell id");
    }

    #[test]
    fn a_removed_admin_who_removes_the_remover_goes_down_with_them() {
        // Gina, the genesis admin, added Alice and Bob; Carol writes.
        let mut genesis = set(&[0x99]);
        let _ = genesis.insert(acct(0xCC), OpMask::WRITE);
        let cell = cell_id(Id::new([1; 32]), &genesis);
        let with = |mut writers: Writers| {
            let _ = writers.insert(acct(0x99), OpMask::FULL);
            let _ = writers.insert(acct(0xCC), OpMask::WRITE);
            writers
        };
        let admins = with(set(&[0xAA, 0xBB]));
        let add = RotationStep {
            prior: genesis.clone(),
            new: admins.clone(),
            ..step(1, 0x99, &[], &[], 1, &[])
        };
        let rotate = |id, signer, new: Writers, nonce, seen: &[u8]| RotationStep {
            prior: admins.clone(),
            new,
            ..step(id, signer, &[], &[], nonce, seen)
        };
        let removal = rotate(2, 0xBB, with(set(&[0xBB])), 20, &[1]);
        // Alice has seen it, and answers on parents from before it.
        let answer = rotate(3, 0xAA, with(set(&[0xAA])), 10, &[1]);
        let neither = with(Writers::new());
        assert_eq!(
            fold_in(cell, [&add, &removal, &answer]),
            Some(neither.clone())
        );

        // Bob, again, having seen both, and Alice forking once more from before.
        let again = RotationStep {
            prior: neither.clone(),
            ..step(4, 0xBB, &[], &[0xBB], 30, &[1, 2, 3])
        };
        let fork = rotate(5, 0xAA, with(set(&[0xAA, 0xEE])), 5, &[1]);
        let after = RotationStep {
            prior: neither.clone(),
            ..step(6, 0xAA, &[], &[0xAA], 40, &[1, 2, 3])
        };
        assert_eq!(
            in_every_order(cell, &[add, removal, answer, again, fork, after]),
            Some(neither)
        );
    }
}
