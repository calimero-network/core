//! Random scenario generation for property-based testing.
//!
//! See spec §10 - Property-based tests over seeds.

use std::collections::HashSet;

use calimero_primitives::crdt::CrdtType;

use crate::sync_sim::actions::EntityMetadata;
use crate::sync_sim::node::SimNode;
use crate::sync_sim::runtime::SimRng;
use crate::sync_sim::types::EntityId;

/// Configuration for random scenario generation.
#[derive(Debug, Clone)]
pub struct RandomScenarioConfig {
    /// Number of nodes.
    pub node_count: usize,
    /// Entity count range per node.
    pub entity_count_range: (usize, usize),
    /// Probability that two nodes share an entity.
    pub shared_entity_probability: f64,
    /// Probability that a node is fresh (no state).
    pub fresh_node_probability: f64,
    /// Available CRDT types.
    pub crdt_types: Vec<CrdtType>,
}

impl Default for RandomScenarioConfig {
    fn default() -> Self {
        Self {
            node_count: 2,
            entity_count_range: (10, 100),
            shared_entity_probability: 0.5,
            fresh_node_probability: 0.0,
            crdt_types: vec![CrdtType::LwwRegister],
        }
    }
}

impl RandomScenarioConfig {
    /// Builder: set node count.
    pub fn with_nodes(mut self, count: usize) -> Self {
        self.node_count = count;
        self
    }

    /// Builder: set entity count range.
    ///
    /// If min > max, they will be swapped to ensure valid range.
    pub fn with_entity_count(mut self, min: usize, max: usize) -> Self {
        // Ensure min <= max to prevent underflow
        self.entity_count_range = if min <= max { (min, max) } else { (max, min) };
        self
    }

    /// Builder: set shared entity probability.
    pub fn with_shared_probability(mut self, prob: f64) -> Self {
        self.shared_entity_probability = prob.clamp(0.0, 1.0);
        self
    }

    /// Builder: allow fresh nodes.
    pub fn with_fresh_probability(mut self, prob: f64) -> Self {
        self.fresh_node_probability = prob.clamp(0.0, 1.0);
        self
    }

    /// Builder: set CRDT types.
    pub fn with_crdt_types(mut self, types: Vec<CrdtType>) -> Self {
        self.crdt_types = types;
        self
    }
}

/// Random scenario generator.
pub struct RandomScenario {
    rng: SimRng,
    config: RandomScenarioConfig,
}

impl RandomScenario {
    /// Create a new generator with seed and config.
    pub fn new(seed: u64, config: RandomScenarioConfig) -> Self {
        Self {
            rng: SimRng::new(seed),
            config,
        }
    }

    /// Create with default config.
    pub fn with_seed(seed: u64) -> Self {
        Self::new(seed, RandomScenarioConfig::default())
    }

    /// Generate nodes according to config.
    pub fn generate(&mut self) -> Vec<SimNode> {
        let mut nodes = Vec::new();
        let mut shared_pool = Vec::new();

        // Generate shared entity pool
        let max_entities = self.config.entity_count_range.1;
        for _i in 0..max_entities {
            let id = EntityId::from_u64(self.rng.gen_u64());
            let data = self.random_data();
            let crdt_type = self.random_crdt_type();
            let timestamp = self.rng.gen_u64();
            let metadata = EntityMetadata::new(crdt_type, timestamp);
            shared_pool.push((id, data, metadata));
        }

        // Generate nodes
        for i in 0..self.config.node_count {
            let mut node = SimNode::new(format!("node-{}", i));

            // Check if this node should be fresh
            if self
                .rng
                .bool_with_probability(self.config.fresh_node_probability)
            {
                nodes.push(node);
                continue;
            }

            // Determine entity count
            let (min, max) = self.config.entity_count_range;
            let range_size = max.saturating_sub(min);
            let entity_count = if range_size == 0 {
                min
            } else {
                self.rng.gen_range_usize(range_size + 1) + min
            };

            // Add entities, tracking inserted IDs to avoid duplicate overwrites
            // that would reduce final entity count below the target
            let mut inserted_ids: HashSet<EntityId> = HashSet::new();
            // Use larger retry budget to handle coupon-collector behavior when sampling from shared pool.
            // With high shared_entity_probability, we need O(n*ln(n)) attempts to collect n unique items.
            // Using (entity_count + pool_size) * 5 provides sufficient margin.
            let max_attempts = (entity_count + shared_pool.len()).saturating_mul(5);
            let mut attempts = 0;

            while inserted_ids.len() < entity_count && attempts < max_attempts {
                attempts += 1;

                if self
                    .rng
                    .bool_with_probability(self.config.shared_entity_probability)
                    && !shared_pool.is_empty()
                {
                    // Use shared entity
                    let idx = self.rng.gen_range_usize(shared_pool.len());
                    let (id, data, metadata) = &shared_pool[idx];

                    // Only insert if not already in this node
                    if inserted_ids.insert(*id) {
                        node.insert_entity_with_metadata(*id, data.clone(), metadata.clone());
                    }
                    // If already inserted, loop will retry with another attempt
                } else {
                    // Create unique entity
                    let id = EntityId::from_u64(self.rng.gen_u64());

                    // Only insert if not already present (extremely unlikely collision)
                    if inserted_ids.insert(id) {
                        let data = self.random_data();
                        let crdt_type = self.random_crdt_type();
                        let timestamp = self.rng.gen_u64();
                        let metadata = EntityMetadata::new(crdt_type, timestamp);
                        node.insert_entity_with_metadata(id, data, metadata);
                    }
                }
            }

            nodes.push(node);
        }

        nodes
    }

    /// Generate random data.
    fn random_data(&mut self) -> Vec<u8> {
        let len = self.rng.gen_range_usize(100) + 10;
        let mut data = vec![0u8; len];
        self.rng.fill_bytes(&mut data);
        data
    }

    /// Pick random CRDT type.
    fn random_crdt_type(&mut self) -> CrdtType {
        if self.config.crdt_types.is_empty() {
            return CrdtType::LwwRegister;
        }
        let idx = self.rng.gen_range_usize(self.config.crdt_types.len());
        self.config.crdt_types[idx].clone()
    }
}

/// Common random scenarios.
impl RandomScenario {
    /// Two nodes with varying overlap.
    pub fn two_nodes_random(seed: u64) -> Vec<SimNode> {
        let config = RandomScenarioConfig::default()
            .with_nodes(2)
            .with_entity_count(50, 100)
            .with_shared_probability(0.3);

        Self::new(seed, config).generate()
    }

    /// Multiple nodes forming a mesh.
    pub fn mesh_random(seed: u64, node_count: usize) -> Vec<SimNode> {
        let config = RandomScenarioConfig::default()
            .with_nodes(node_count)
            .with_entity_count(20, 50)
            .with_shared_probability(0.5);

        Self::new(seed, config).generate()
    }

    /// Scenario with exactly one fresh node joining.
    ///
    /// Creates `existing_count` initialized nodes with shared state, plus one fresh
    /// (empty) node. The fresh node is always the last node in the returned vector.
    pub fn fresh_join_random(seed: u64, existing_count: usize) -> Vec<SimNode> {
        // Generate initialized nodes with no fresh probability
        let config = RandomScenarioConfig::default()
            .with_nodes(existing_count)
            .with_entity_count(50, 100)
            .with_fresh_probability(0.0) // No fresh nodes in this batch
            .with_shared_probability(0.8);

        let mut nodes = Self::new(seed, config).generate();

        // Add exactly one fresh node
        let fresh = SimNode::new(format!("node-{}", existing_count));
        nodes.push(fresh);

        nodes
    }

    /// Heavy divergence scenario.
    pub fn heavy_divergence(seed: u64, node_count: usize) -> Vec<SimNode> {
        let config = RandomScenarioConfig::default()
            .with_nodes(node_count)
            .with_entity_count(50, 200)
            .with_shared_probability(0.1); // Low overlap

        Self::new(seed, config).generate()
    }

    /// Nearly synced scenario.
    pub fn nearly_synced(seed: u64, node_count: usize) -> Vec<SimNode> {
        let config = RandomScenarioConfig::default()
            .with_nodes(node_count)
            .with_entity_count(100, 150)
            .with_shared_probability(0.95); // High overlap

        Self::new(seed, config).generate()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_random_scenario_deterministic() {
        let mut nodes1 = RandomScenario::two_nodes_random(42);
        let mut nodes2 = RandomScenario::two_nodes_random(42);

        assert_eq!(nodes1.len(), nodes2.len());

        for (n1, n2) in nodes1.iter_mut().zip(nodes2.iter_mut()) {
            assert_eq!(n1.entity_count(), n2.entity_count());
            assert_eq!(n1.storage.digest(), n2.storage.digest());
        }
    }

    #[test]
    fn test_random_scenario_different_seeds() {
        let mut nodes1 = RandomScenario::two_nodes_random(42);
        let mut nodes2 = RandomScenario::two_nodes_random(43);

        // Very unlikely to have same digests
        let digests1: Vec<_> = nodes1.iter_mut().map(|n| n.storage.digest()).collect();
        let digests2: Vec<_> = nodes2.iter_mut().map(|n| n.storage.digest()).collect();

        assert_ne!(digests1, digests2);
    }

    #[test]
    fn test_mesh_random() {
        let nodes = RandomScenario::mesh_random(42, 5);
        assert_eq!(nodes.len(), 5);

        // All nodes should have some state (at least some entities)
        for node in &nodes {
            // Each node should have at least the minimum from config (20)
            // but shared probability means some might have fewer unique
            assert!(
                node.entity_count() >= 10,
                "node {} has {} entities",
                node.id(),
                node.entity_count()
            );
        }
    }

    #[test]
    fn test_fresh_join() {
        let nodes = RandomScenario::fresh_join_random(42, 3);

        // Should have exactly 4 nodes: 3 existing + 1 fresh
        assert_eq!(nodes.len(), 4);

        // First 3 nodes should have state
        for node in &nodes[..3] {
            assert!(
                node.has_any_state(),
                "existing node {} should have state",
                node.id()
            );
        }

        // Last node should be fresh (no state)
        assert!(!nodes[3].has_any_state(), "fresh node should have no state");
    }

    #[test]
    fn test_heavy_divergence() {
        let nodes = RandomScenario::heavy_divergence(42, 3);

        // With low shared probability, nodes should have mostly different entities
        let all_ids: Vec<_> = nodes
            .iter()
            .flat_map(|n| n.storage.iter().map(|e| e.id))
            .collect();

        // Count unique IDs
        let mut unique_ids = all_ids.clone();
        unique_ids.sort();
        unique_ids.dedup();

        // Should have many unique IDs (high divergence)
        assert!(unique_ids.len() as f64 > all_ids.len() as f64 * 0.5);
    }

    #[test]
    fn test_nearly_synced() {
        let nodes = RandomScenario::nearly_synced(42, 3);

        // With high shared probability, nodes should have mostly same entities
        // Digests might not be equal (due to unique entities), but entity counts should be similar
        for node in &nodes {
            // Entity counts depend on random selection from pool
            assert!(
                node.entity_count() >= 50,
                "node {} has {} entities",
                node.id(),
                node.entity_count()
            );
        }
    }

    #[test]
    fn test_entity_count_range_swap() {
        // Test that min > max is handled gracefully
        let config = RandomScenarioConfig::default().with_entity_count(100, 50);

        // Should have swapped to (50, 100)
        assert_eq!(config.entity_count_range, (50, 100));
    }

    #[test]
    fn test_entity_count_range_equal() {
        // Test that min == max works
        let config = RandomScenarioConfig::default()
            .with_nodes(2)
            .with_entity_count(50, 50)
            .with_shared_probability(0.0); // No shared to ensure exact count

        let mut scenario = RandomScenario::new(42, config);
        let nodes = scenario.generate();

        // All nodes should have exactly 50 entities (or 0 if fresh)
        for node in &nodes {
            if node.has_any_state() {
                assert_eq!(
                    node.entity_count(),
                    50,
                    "node {} has {} entities",
                    node.id(),
                    node.entity_count()
                );
            }
        }
    }
}
