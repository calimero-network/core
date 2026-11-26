//! Which nodes hold a new app's replicas (`Node::place`).
//!
//! Replicas span as many zones as the eligible nodes are in, then fill up by
//! load: an app on nodes of three zones survives losing any one zone, which
//! the least-loaded nodes alone do not promise. A node's zone is the one
//! `POST /ops/nodes` registered for it; nodes registered without one (every
//! node from `--peers`) share a single unknown zone, since nothing says they
//! are apart: a node in a named zone is preferred over a second zoneless one.
//! A cluster with no zones at all is therefore placed by load alone, as before.

use std::collections::BTreeSet;

use calimero_log::NodeId;

use crate::system::SystemState;

/// A node placement may choose: its id, its zone (empty: unknown) and how
/// many apps it holds a replica of.
#[derive(Clone, Debug)]
pub struct Candidate<'a> {
    pub id: NodeId,
    pub zone: &'a str,
    pub load: usize,
}

/// Picks `count` replicas among `allowed` (the registered nodes on a release
/// the app's owner approved), from the zones and loads `state` records.
#[must_use]
pub fn replicas(state: &SystemState, allowed: &[NodeId], count: usize) -> Vec<NodeId> {
    let candidates: Vec<Candidate<'_>> = allowed
        .iter()
        .filter_map(|id| {
            Some(Candidate {
                id: *id,
                zone: &state.replica_node(id)?.zone,
                load: state
                    .apps
                    .values()
                    .filter(|a| a.replicas.contains(id))
                    .count(),
            })
        })
        .collect();
    choose(&candidates, count)
}

/// Picks `count` of `candidates` (all of them when fewer), sorted by id.
///
/// Taken least loaded first (ties by lower id), skipping a node whose zone an
/// earlier pick already covers while a candidate of an uncovered zone is left;
/// once every zone is covered the rest fill by load the same way. So the picks
/// cover `min(count, zones)` zones, and within that the least-loaded nodes.
#[must_use]
pub fn choose(candidates: &[Candidate<'_>], count: usize) -> Vec<NodeId> {
    let mut by_load: Vec<&Candidate<'_>> = candidates.iter().collect();
    by_load.sort_unstable_by_key(|c| (c.load, c.id));
    let mut covered: BTreeSet<&str> = BTreeSet::new();
    let mut picked: Vec<NodeId> = Vec::with_capacity(count);
    // First a node of each zone, least loaded first; then the rest by load.
    for c in &by_load {
        if picked.len() == count {
            break;
        }
        if covered.insert(c.zone) {
            picked.push(c.id);
        }
    }
    for c in &by_load {
        if picked.len() == count {
            break;
        }
        if !picked.contains(&c.id) {
            picked.push(c.id);
        }
    }
    picked.sort_unstable();
    picked
}

/// Orders an app's replica set so the first entry — the replica that
/// initialises the group and so becomes its first leader
/// ([the write path](https://calimero-network.github.io/core/protocol/write-path/))
/// — is round-robin across the set by `index`, instead of always the lowest id.
/// `index` is the count of apps already created when the leader places this one,
/// so index 0 keeps the lowest id first — the first app a cluster creates still
/// leads on the same node as before — and each later app moves one node along.
/// The set is unchanged (its zones and load are [`choose`]'s); only the order
/// rotates. The leader runs this before it records the set in the `CreateApp`
/// entry, so every node applies the same order and apply does not recompute it —
/// no node disagrees across a mixed-version rollout. Spreading the initial
/// leadership is all core does: it hands a caught-up node no leadership later
/// (openraft 0.9 has no leader transfer), so evening out leadership after
/// failovers, or as the fleet grows, is the cluster manager's.
pub fn order_for_leader(replicas: &mut [NodeId], index: u64) {
    if replicas.is_empty() {
        return;
    }
    replicas.sort_unstable();
    let first = (index % replicas.len() as u64) as usize;
    replicas.rotate_left(first);
}

#[cfg(test)]
mod tests {
    use super::order_for_leader;

    #[test]
    fn the_leader_slot_round_robins_by_index() {
        // Whatever order the set arrives in, it is sorted, then the first slot
        // rotates by index: index 0 keeps the lowest id leading (so the first
        // app a cluster makes is unchanged), and later apps move one along.
        let first = |i| {
            let mut v = vec![3_u64, 1, 2];
            order_for_leader(&mut v, i);
            v[0]
        };
        assert_eq!((first(0), first(1), first(2), first(3)), (1, 2, 3, 1));
    }

    #[test]
    fn ordering_keeps_the_set_and_handles_small_sets() {
        // The rotation never drops, repeats or adds a replica.
        let mut v = vec![5_u64, 9, 7];
        order_for_leader(&mut v, 4); // sorted [5,7,9], 4 % 3 == 1 -> [7,9,5]
        assert_eq!(v, vec![7, 9, 5]);
        let mut sorted = v.clone();
        sorted.sort_unstable();
        assert_eq!(sorted, vec![5, 7, 9]);
        // One replica, or none, is left as it is (no leadership to spread).
        let mut one = vec![42_u64];
        order_for_leader(&mut one, 7);
        assert_eq!(one, vec![42]);
        let mut none: Vec<u64> = vec![];
        order_for_leader(&mut none, 7);
        assert!(none.is_empty());
    }
}

#[cfg(test)]
mod set_tests {
    use super::{choose, replicas, Candidate};
    use crate::system::SystemState;
    use crate::types::{NodeRecord, NodeRole};

    fn nodes<'a>(spec: &[(u64, &'a str, usize)]) -> Vec<Candidate<'a>> {
        spec.iter()
            .map(|&(id, zone, load)| Candidate { id, zone, load })
            .collect()
    }

    #[test]
    fn one_replica_per_zone_before_a_second_in_any() {
        let n = nodes(&[(1, "a", 0), (2, "a", 0), (3, "b", 5), (4, "c", 9)]);
        assert_eq!(choose(&n, 3), vec![1, 3, 4]);
    }

    #[test]
    fn the_least_loaded_node_of_a_zone_is_taken() {
        let n = nodes(&[(1, "a", 2), (2, "a", 1), (3, "b", 0), (4, "c", 0)]);
        assert_eq!(choose(&n, 3), vec![2, 3, 4]);
    }

    #[test]
    fn with_fewer_zones_than_replicas_the_rest_fill_by_load() {
        let n = nodes(&[(1, "a", 3), (2, "a", 0), (3, "a", 1), (4, "b", 7)]);
        assert_eq!(choose(&n, 3), vec![2, 3, 4]);
    }

    #[test]
    fn with_more_zones_than_replicas_the_least_loaded_zones_win() {
        let n = nodes(&[(1, "a", 4), (2, "b", 1), (3, "c", 2), (4, "d", 1)]);
        assert_eq!(choose(&n, 3), vec![2, 3, 4]);
    }

    #[test]
    fn zoneless_nodes_share_one_unknown_zone() {
        // Two zoneless nodes and two of zone a: one of each, then by load.
        let n = nodes(&[(1, "", 0), (2, "", 0), (3, "a", 1), (4, "a", 2)]);
        assert_eq!(choose(&n, 3), vec![1, 2, 3]);
        let n = nodes(&[(1, "", 0), (2, "", 0), (3, "a", 1), (4, "b", 2)]);
        assert_eq!(choose(&n, 3), vec![1, 3, 4]);
    }

    #[test]
    fn without_zones_placement_is_by_load_then_id() {
        let n = nodes(&[(1, "", 2), (2, "", 0), (3, "", 1), (4, "", 0)]);
        assert_eq!(choose(&n, 3), vec![2, 3, 4]);
        let n = nodes(&[(4, "", 0), (3, "", 0), (2, "", 0), (1, "", 0)]);
        assert_eq!(choose(&n, 3), vec![1, 2, 3]);
    }

    #[test]
    fn fewer_candidates_than_replicas_takes_them_all() {
        let n = nodes(&[(5, "a", 0), (2, "b", 0)]);
        assert_eq!(choose(&n, 3), vec![2, 5]);
        assert!(choose(&[], 3).is_empty());
    }

    /// Placement never picks a job node as an app replica: a job node holds no K_c and no Raft log.
    #[test]
    fn a_job_node_is_never_placed() {
        let mut state = SystemState::default();
        for (id, role) in [
            (1, NodeRole::Replica),
            (2, NodeRole::Job),
            (3, NodeRole::Replica),
        ] {
            let _prev = state.nodes.insert(
                id,
                NodeRecord {
                    id,
                    addr: String::new(),
                    zone: String::new(),
                    role,
                    pool: String::new(),
                },
            );
        }
        assert_eq!(replicas(&state, &[1, 2, 3], 3), vec![1, 3]);
    }
}
