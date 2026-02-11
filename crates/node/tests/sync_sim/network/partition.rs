//! Network partition modeling.
//!
//! See spec §12 - Partition Modeling.

use std::collections::HashSet;

use crate::sync_sim::runtime::SimTime;
use crate::sync_sim::types::NodeId;

/// Partition specification.
#[derive(Debug, Clone)]
pub enum PartitionSpec {
    /// Symmetric: neither side can reach the other.
    /// Groups are mutually isolated.
    Bidirectional { groups: Vec<Vec<NodeId>> },

    /// Asymmetric: specific directed links are blocked.
    Directional { blocked: Vec<(NodeId, NodeId)> },
}

impl PartitionSpec {
    /// Create a bidirectional partition splitting nodes into two groups.
    pub fn split(group_a: Vec<NodeId>, group_b: Vec<NodeId>) -> Self {
        Self::Bidirectional {
            groups: vec![group_a, group_b],
        }
    }

    /// Create a partition isolating a single node from all others.
    pub fn isolate(node: NodeId, others: Vec<NodeId>) -> Self {
        Self::Bidirectional {
            groups: vec![vec![node], others],
        }
    }

    /// Create a directional block (from cannot reach to).
    pub fn block(from: NodeId, to: NodeId) -> Self {
        Self::Directional {
            blocked: vec![(from, to)],
        }
    }

    /// Check if this partition blocks communication from `from` to `to`.
    pub fn blocks(&self, from: &NodeId, to: &NodeId) -> bool {
        match self {
            Self::Bidirectional { groups } => {
                // Find which groups contain from and to
                let from_group = groups.iter().position(|g| g.contains(from));
                let to_group = groups.iter().position(|g| g.contains(to));

                match (from_group, to_group) {
                    (Some(fg), Some(tg)) => fg != tg, // Different groups = blocked
                    _ => false,                       // Unknown nodes = not blocked
                }
            }
            Self::Directional { blocked } => blocked.iter().any(|(f, t)| f == from && t == to),
        }
    }
}

/// Active partition with timing.
#[derive(Debug, Clone)]
struct ActivePartition {
    /// The partition specification.
    spec: PartitionSpec,
    /// When the partition started.
    #[allow(dead_code)]
    start_time: SimTime,
    /// When the partition ends (None = permanent until explicitly removed).
    end_time: Option<SimTime>,
}

/// Manages active network partitions.
#[derive(Debug, Default)]
pub struct PartitionManager {
    /// Active partitions.
    partitions: Vec<ActivePartition>,
    /// Quick lookup: node pairs that are currently partitioned.
    /// Key: (from, to) where from < to for bidirectional.
    blocked_cache: HashSet<(String, String)>,
    /// Cache validity time.
    cache_time: Option<SimTime>,
}

impl PartitionManager {
    /// Create a new partition manager.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a partition.
    pub fn add_partition(&mut self, spec: PartitionSpec, start: SimTime, end: Option<SimTime>) {
        self.partitions.push(ActivePartition {
            spec,
            start_time: start,
            end_time: end,
        });
        self.invalidate_cache();
    }

    /// Remove partitions matching a predicate.
    pub fn remove_partitions<F>(&mut self, predicate: F)
    where
        F: Fn(&PartitionSpec) -> bool,
    {
        self.partitions.retain(|p| !predicate(&p.spec));
        self.invalidate_cache();
    }

    /// Clear all partitions.
    pub fn clear(&mut self) {
        self.partitions.clear();
        self.invalidate_cache();
    }

    /// Check if communication from `from` to `to` is blocked at time `now`.
    pub fn is_partitioned(&mut self, from: &NodeId, to: &NodeId, now: SimTime) -> bool {
        // Remove expired partitions
        self.partitions
            .retain(|p| p.end_time.map_or(true, |end| end > now));

        // Check each active partition
        for partition in &self.partitions {
            if partition.spec.blocks(from, to) {
                return true;
            }
        }

        false
    }

    /// Get number of active partitions.
    pub fn partition_count(&self) -> usize {
        self.partitions.len()
    }

    /// Check if there are any active partitions.
    pub fn has_partitions(&self) -> bool {
        !self.partitions.is_empty()
    }

    /// Invalidate the cache.
    fn invalidate_cache(&mut self) {
        self.blocked_cache.clear();
        self.cache_time = None;
    }

    /// Get all currently blocked node pairs (for debugging).
    pub fn get_blocked_pairs(&self, nodes: &[NodeId], now: SimTime) -> Vec<(NodeId, NodeId)> {
        let mut blocked = Vec::new();

        for partition in &self.partitions {
            if partition.end_time.map_or(true, |end| end > now) {
                for from in nodes {
                    for to in nodes {
                        if from != to && partition.spec.blocks(from, to) {
                            blocked.push((from.clone(), to.clone()));
                        }
                    }
                }
            }
        }

        blocked
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bidirectional_partition() {
        let spec = PartitionSpec::split(
            vec![NodeId::new("a"), NodeId::new("b")],
            vec![NodeId::new("c"), NodeId::new("d")],
        );

        // Within group: not blocked
        assert!(!spec.blocks(&NodeId::new("a"), &NodeId::new("b")));
        assert!(!spec.blocks(&NodeId::new("c"), &NodeId::new("d")));

        // Across groups: blocked
        assert!(spec.blocks(&NodeId::new("a"), &NodeId::new("c")));
        assert!(spec.blocks(&NodeId::new("c"), &NodeId::new("a"))); // Symmetric
        assert!(spec.blocks(&NodeId::new("b"), &NodeId::new("d")));
    }

    #[test]
    fn test_directional_partition() {
        let spec = PartitionSpec::block(NodeId::new("a"), NodeId::new("b"));

        // a -> b is blocked
        assert!(spec.blocks(&NodeId::new("a"), &NodeId::new("b")));

        // b -> a is NOT blocked (directional)
        assert!(!spec.blocks(&NodeId::new("b"), &NodeId::new("a")));

        // Others not affected
        assert!(!spec.blocks(&NodeId::new("a"), &NodeId::new("c")));
    }

    #[test]
    fn test_isolate_partition() {
        let spec = PartitionSpec::isolate(
            NodeId::new("isolated"),
            vec![NodeId::new("a"), NodeId::new("b"), NodeId::new("c")],
        );

        // Isolated node cannot reach anyone
        assert!(spec.blocks(&NodeId::new("isolated"), &NodeId::new("a")));
        assert!(spec.blocks(&NodeId::new("isolated"), &NodeId::new("b")));

        // Others cannot reach isolated node
        assert!(spec.blocks(&NodeId::new("a"), &NodeId::new("isolated")));

        // Others can reach each other
        assert!(!spec.blocks(&NodeId::new("a"), &NodeId::new("b")));
    }

    #[test]
    fn test_partition_manager_timing() {
        let mut manager = PartitionManager::new();

        let now = SimTime::from_millis(100);
        let end = SimTime::from_millis(200);

        manager.add_partition(
            PartitionSpec::split(vec![NodeId::new("a")], vec![NodeId::new("b")]),
            now,
            Some(end),
        );

        // During partition
        assert!(manager.is_partitioned(&NodeId::new("a"), &NodeId::new("b"), now));
        assert!(manager.is_partitioned(
            &NodeId::new("a"),
            &NodeId::new("b"),
            SimTime::from_millis(150)
        ));

        // After partition expires
        assert!(!manager.is_partitioned(
            &NodeId::new("a"),
            &NodeId::new("b"),
            SimTime::from_millis(200)
        ));
    }

    #[test]
    fn test_partition_manager_permanent() {
        let mut manager = PartitionManager::new();

        manager.add_partition(
            PartitionSpec::split(vec![NodeId::new("a")], vec![NodeId::new("b")]),
            SimTime::ZERO,
            None, // Permanent
        );

        // Always blocked
        assert!(manager.is_partitioned(
            &NodeId::new("a"),
            &NodeId::new("b"),
            SimTime::from_millis(1_000_000)
        ));
    }

    #[test]
    fn test_partition_manager_clear() {
        let mut manager = PartitionManager::new();

        manager.add_partition(
            PartitionSpec::split(vec![NodeId::new("a")], vec![NodeId::new("b")]),
            SimTime::ZERO,
            None,
        );

        assert!(manager.has_partitions());

        manager.clear();

        assert!(!manager.has_partitions());
        assert!(!manager.is_partitioned(&NodeId::new("a"), &NodeId::new("b"), SimTime::ZERO));
    }
}
