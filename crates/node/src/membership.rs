//! The system group's membership: which learners the system leader makes
//! voters ([`crate::node::Node`]'s `promote_learners`, from `reconcile`), and
//! how many voters it keeps.

use std::collections::BTreeSet;

use calimero_log::NodeId;

/// The system voters a cluster keeps when nobody chose ([`SystemVoters`]):
/// 3 tolerate one failure.
pub const DEFAULT_SYSTEM_VOTERS: usize = 3;

/// How far behind the system leader's log a learner may be and still be
/// promoted to a system voter, in entries. While the membership change is
/// joint, a write needs a majority of the new voters too, so a promoted
/// learner far behind would hold every system write (sign-ins, access
/// checks' read index) until it caught up. The system log is low-rate
/// (directory changes, sessions, keys): a learner that is replicating keeps
/// within one heartbeat's batch, a handful of entries; 16 allows a burst
/// in flight without promoting one that is still installing a snapshot.
pub const PROMOTE_MAX_LAG: u64 = 16;

/// How many voters the system group keeps: 3 (one failure tolerated) or 5
/// (two), never another count. Set at genesis (`--system-voters`) and by the
/// operator (`POST /ops/system-voters`), both through the system log, so every
/// leader reads the same number.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SystemVoters(usize);

impl SystemVoters {
    /// The counts a cluster may choose. Odd: an even count tolerates no more
    /// failures than the odd one below it, and needs one more vote per write.
    pub const ALLOWED: [usize; 2] = [3, 5];

    /// `n` if it is one of [`Self::ALLOWED`].
    pub fn new(n: usize) -> Result<Self, String> {
        if Self::ALLOWED.contains(&n) {
            Ok(Self(n))
        } else {
            Err(format!("the system group keeps 3 or 5 voters, not {n}"))
        }
    }

    #[must_use]
    pub const fn get(self) -> usize {
        self.0
    }
}

impl Default for SystemVoters {
    fn default() -> Self {
        Self(DEFAULT_SYSTEM_VOTERS)
    }
}

/// The odd sizes from `target` down to the smallest above `have`, largest
/// first: what a change from `have` voters may reach.
fn odd_sizes(target: usize, have: usize) -> impl Iterator<Item = usize> {
    (have + 1..=target).rev().filter(|n| n % 2 == 1)
}

/// The voter set to grow `voters` to from caught-up `candidates` (id, zone):
/// the largest odd size up to `target` that enough candidates reach, or
/// `None` when there are already `target`, or too few candidates to reach the
/// next odd size (a cluster on one node waits for two learners, then has 3;
/// on 3 it waits for two more, then has 5). Prefers a candidate in a zone no
/// voter (nor one already picked) is in, then one with no zone, then the
/// rest; lower ids first within each.
#[must_use]
pub fn grow_voters(
    target: SystemVoters,
    voters: &[NodeId],
    voter_zones: &[String],
    candidates: &[(NodeId, String)],
) -> Option<BTreeSet<NodeId>> {
    let mut pool: Vec<(NodeId, String)> = candidates
        .iter()
        .filter(|(id, _)| !voters.contains(id))
        .cloned()
        .collect();
    let size = odd_sizes(target.get(), voters.len()).find(|n| n - voters.len() <= pool.len())?;
    pool.sort_unstable();
    let mut zones: BTreeSet<String> = voter_zones
        .iter()
        .filter(|z| !z.is_empty())
        .cloned()
        .collect();
    let mut grown: BTreeSet<NodeId> = voters.iter().copied().collect();
    for _ in voters.len()..size {
        let rank = |z: &String| {
            if z.is_empty() {
                1
            } else if zones.contains(z) {
                2
            } else {
                0
            }
        };
        let (at, _) = pool
            .iter()
            .enumerate()
            .min_by_key(|(_, (id, z))| (rank(z), *id))?;
        let (id, zone) = pool.remove(at);
        let _new = grown.insert(id);
        if !zone.is_empty() {
            let _new = zones.insert(zone);
        }
    }
    Some(grown)
}

/// The voters to keep when there are more than `target`, or an even count
/// no candidate can make odd: the largest odd size up to `target`, keeping
/// `leader`, then one voter per zone, then the rest; lower ids first. `None`
/// when `voters` is already an odd count no larger than `target`.
#[must_use]
pub fn shrink_voters(
    target: SystemVoters,
    leader: NodeId,
    voters: &[(NodeId, String)],
) -> Option<BTreeSet<NodeId>> {
    let have = voters.len();
    if have <= target.get() && have % 2 == 1 {
        return None;
    }
    let keep = (1..=target.get().min(have))
        .rev()
        .find(|n| n % 2 == 1)
        .unwrap_or(1);
    let mut rest: Vec<(NodeId, String)> = voters
        .iter()
        .filter(|(id, _)| *id != leader)
        .cloned()
        .collect();
    rest.sort_unstable();
    let mut kept: BTreeSet<NodeId> = BTreeSet::from([leader]);
    let mut zones: BTreeSet<String> = voters
        .iter()
        .filter(|(id, z)| *id == leader && !z.is_empty())
        .map(|(_, z)| z.clone())
        .collect();
    // New zones first, then the rest in id order.
    let (spread, others): (Vec<_>, Vec<_>) = rest.into_iter().partition(|(_, z)| {
        let new = !z.is_empty() && !zones.contains(z);
        if new {
            let _new = zones.insert(z.clone());
        }
        new
    });
    for (id, _) in spread.into_iter().chain(others) {
        if kept.len() >= keep {
            break;
        }
        let _new = kept.insert(id);
    }
    Some(kept)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(list: &[(NodeId, &str)]) -> Vec<(NodeId, String)> {
        list.iter().map(|(i, z)| (*i, (*z).to_owned())).collect()
    }

    fn three() -> SystemVoters {
        SystemVoters::default()
    }

    fn five() -> SystemVoters {
        SystemVoters::new(5).unwrap()
    }

    #[test]
    fn only_three_or_five() {
        for n in [0, 1, 2, 4, 6, 7] {
            assert!(SystemVoters::new(n).is_err(), "{n}");
        }
        assert_eq!(SystemVoters::new(5).unwrap().get(), 5);
        assert_eq!(SystemVoters::default().get(), 3);
    }

    #[test]
    fn one_voter_waits_for_two_learners_never_stopping_at_two() {
        let one = [1];
        let zones = ["a".to_owned()];
        assert_eq!(grow_voters(three(), &one, &zones, &c(&[(2, "b")])), None);
        assert_eq!(
            grow_voters(three(), &one, &zones, &c(&[(2, "b"), (3, "c")])),
            Some(BTreeSet::from([1, 2, 3]))
        );
    }

    #[test]
    fn two_voters_take_one_and_three_take_none() {
        let zones = ["a".to_owned(), "b".to_owned()];
        assert_eq!(
            grow_voters(three(), &[1, 2], &zones, &c(&[(3, "c"), (4, "d")])),
            Some(BTreeSet::from([1, 2, 3]))
        );
        let zones = ["a".to_owned(), "b".to_owned(), "c".to_owned()];
        assert_eq!(
            grow_voters(three(), &[1, 2, 3], &zones, &c(&[(4, "d")])),
            None
        );
    }

    #[test]
    fn five_grows_by_two_never_to_four() {
        let zones = ["a".to_owned(), "b".to_owned(), "c".to_owned()];
        assert_eq!(
            grow_voters(five(), &[1, 2, 3], &zones, &c(&[(4, "d")])),
            None,
            "one learner would make four"
        );
        assert_eq!(
            grow_voters(
                five(),
                &[1, 2, 3],
                &zones,
                &c(&[(4, "a"), (5, "d"), (6, "e")])
            ),
            Some(BTreeSet::from([1, 2, 3, 5, 6])),
            "new zones first"
        );
        // From one node: 3 first when only two learners are there, 5 at once with four.
        let one = ["a".to_owned()];
        assert_eq!(
            grow_voters(five(), &[1], &one, &c(&[(2, "b"), (3, "c")])),
            Some(BTreeSet::from([1, 2, 3]))
        );
        assert_eq!(
            grow_voters(
                five(),
                &[1],
                &one,
                &c(&[(2, "b"), (3, "c"), (4, "d"), (5, "e")])
            ),
            Some(BTreeSet::from([1, 2, 3, 4, 5]))
        );
        let all = ["a", "b", "c", "d", "e"].map(str::to_owned);
        assert_eq!(
            grow_voters(five(), &[1, 2, 3, 4, 5], &all, &c(&[(6, "f")])),
            None
        );
    }

    #[test]
    fn learners_in_new_zones_come_first() {
        let zones = ["a".to_owned()];
        // 2 shares a's zone, 3 has none, 4 and 5 share one new zone, 6 another.
        let pool = c(&[(2, "a"), (3, ""), (4, "b"), (5, "b"), (6, "c")]);
        assert_eq!(
            grow_voters(three(), &[1], &zones, &pool),
            Some(BTreeSet::from([1, 4, 6]))
        );
        // Without a second new zone: one with no zone before a taken one.
        let pool = c(&[(2, "a"), (3, ""), (4, "b"), (5, "b")]);
        assert_eq!(
            grow_voters(three(), &[1], &zones, &pool),
            Some(BTreeSet::from([1, 3, 4]))
        );
    }

    #[test]
    fn shrinking_keeps_the_leader_and_the_spread() {
        let v = c(&[(1, "a"), (2, "a"), (3, "b"), (4, "c"), (5, "b")]);
        assert_eq!(shrink_voters(five(), 2, &v), None);
        assert_eq!(
            shrink_voters(three(), 2, &v),
            Some(BTreeSet::from([2, 3, 4])),
            "the leader, then one voter per zone"
        );
        // An even count is made odd: four voters left after a removal.
        let v = c(&[(1, "a"), (2, "b"), (3, "c"), (4, "d")]);
        assert_eq!(
            shrink_voters(five(), 4, &v),
            Some(BTreeSet::from([1, 2, 4]))
        );
        let v = c(&[(1, "a"), (2, "b"), (3, "c")]);
        assert_eq!(shrink_voters(three(), 1, &v), None);
        assert_eq!(
            shrink_voters(three(), 1, &c(&[(1, "a"), (2, "b")])),
            Some(BTreeSet::from([1]))
        );
    }
}
