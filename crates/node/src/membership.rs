//! The system group's membership: which learners the system leader makes
//! voters ([`crate::node::Node`]'s `promote_learners`, from `reconcile`).

use std::collections::BTreeSet;

use calimero_log::NodeId;

/// The system group's voters the leader grows to on its own, from learners.
/// 3 tolerate one failure; 5 (two) would be an operator's choice, not automatic.
pub const SYSTEM_VOTERS: usize = 3;

/// How far behind the system leader's log a learner may be and still be
/// promoted to a system voter, in entries. While the membership change is
/// joint, a write needs a majority of the new voters too, so a promoted
/// learner far behind would hold every system write (sign-ins, access
/// checks' read index) until it caught up. The system log is low-rate
/// (directory changes, sessions, keys): a learner that is replicating keeps
/// within one heartbeat's batch, a handful of entries; 16 allows a burst
/// in flight without promoting one that is still installing a snapshot.
pub const PROMOTE_MAX_LAG: u64 = 16;

/// The voter set to grow `voters` to from caught-up `candidates` (id, zone):
/// exactly [`SYSTEM_VOTERS`], or `None` when there are already as many, or
/// too few candidates to get there (never 2). Prefers a candidate in a zone
/// no voter (nor one already picked) is in, then one with no zone, then the
/// rest; lower ids first within each.
#[must_use]
pub fn grow_voters(
    voters: &[NodeId],
    voter_zones: &[String],
    candidates: &[(NodeId, String)],
) -> Option<BTreeSet<NodeId>> {
    let need = SYSTEM_VOTERS.checked_sub(voters.len()).filter(|n| *n > 0)?;
    let mut pool: Vec<(NodeId, String)> = candidates
        .iter()
        .filter(|(id, _)| !voters.contains(id))
        .cloned()
        .collect();
    if pool.len() < need {
        return None;
    }
    pool.sort_unstable();
    let mut zones: BTreeSet<String> = voter_zones
        .iter()
        .filter(|z| !z.is_empty())
        .cloned()
        .collect();
    let mut grown: BTreeSet<NodeId> = voters.iter().copied().collect();
    for _ in 0..need {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn c(list: &[(NodeId, &str)]) -> Vec<(NodeId, String)> {
        list.iter().map(|(i, z)| (*i, (*z).to_owned())).collect()
    }

    #[test]
    fn one_voter_waits_for_two_learners_never_stopping_at_two() {
        let one = [1];
        let zones = ["a".to_owned()];
        assert_eq!(grow_voters(&one, &zones, &c(&[(2, "b")])), None);
        assert_eq!(
            grow_voters(&one, &zones, &c(&[(2, "b"), (3, "c")])),
            Some(BTreeSet::from([1, 2, 3]))
        );
    }

    #[test]
    fn two_voters_take_one_and_three_take_none() {
        let zones = ["a".to_owned(), "b".to_owned()];
        assert_eq!(
            grow_voters(&[1, 2], &zones, &c(&[(3, "c"), (4, "d")])),
            Some(BTreeSet::from([1, 2, 3]))
        );
        let zones = ["a".to_owned(), "b".to_owned(), "c".to_owned()];
        assert_eq!(grow_voters(&[1, 2, 3], &zones, &c(&[(4, "d")])), None);
    }

    #[test]
    fn learners_in_new_zones_come_first() {
        let zones = ["a".to_owned()];
        // 2 shares a's zone, 3 has none, 4 and 5 share one new zone, 6 another.
        let pool = c(&[(2, "a"), (3, ""), (4, "b"), (5, "b"), (6, "c")]);
        assert_eq!(
            grow_voters(&[1], &zones, &pool),
            Some(BTreeSet::from([1, 4, 6]))
        );
        // Without a second new zone: one with no zone before a taken one.
        let pool = c(&[(2, "a"), (3, ""), (4, "b"), (5, "b")]);
        assert_eq!(
            grow_voters(&[1], &zones, &pool),
            Some(BTreeSet::from([1, 3, 4]))
        );
    }
}
