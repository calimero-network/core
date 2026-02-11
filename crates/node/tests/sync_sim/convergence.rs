//! Convergence checking for simulation.
//!
//! See spec §8 - Convergence Definition.

use std::collections::HashMap;

use super::types::{NodeId, StateDigest};

/// Result of convergence check.
#[derive(Debug, Clone)]
pub enum ConvergenceResult {
    /// System has converged - all properties satisfied.
    Converged,
    /// System is still converging - one or more properties not yet satisfied.
    Pending(ConvergencePending),
    /// System has diverged - state digests don't match.
    Diverged(ConvergenceDiff),
}

impl ConvergenceResult {
    /// Check if converged.
    pub fn is_converged(&self) -> bool {
        matches!(self, Self::Converged)
    }

    /// Check if pending.
    pub fn is_pending(&self) -> bool {
        matches!(self, Self::Pending(_))
    }

    /// Check if diverged.
    pub fn is_diverged(&self) -> bool {
        matches!(self, Self::Diverged(_))
    }
}

/// Reason why convergence is pending.
#[derive(Debug, Clone)]
pub struct ConvergencePending {
    /// Which property is blocking.
    pub blocking_property: ConvergenceProperty,
    /// Human-readable reason.
    pub reason: String,
}

/// Convergence properties from spec §8.1.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConvergenceProperty {
    /// C1: Network quiescent (no messages in flight).
    NetworkQuiescent,
    /// C2: All nodes idle (sync_state == Idle).
    AllNodesIdle,
    /// C3: No pending buffers (delta buffers empty).
    NoPendingBuffers,
    /// C4: No pending sync timers.
    NoPendingSyncTimers,
    /// C5: State digests equal.
    StateDigestsEqual,
}

impl ConvergenceProperty {
    /// Get property ID for display.
    pub fn id(&self) -> &'static str {
        match self {
            Self::NetworkQuiescent => "C1",
            Self::AllNodesIdle => "C2",
            Self::NoPendingBuffers => "C3",
            Self::NoPendingSyncTimers => "C4",
            Self::StateDigestsEqual => "C5",
        }
    }
}

/// Difference details when diverged.
#[derive(Debug, Clone)]
pub struct ConvergenceDiff {
    /// State digests by node.
    pub digests: HashMap<NodeId, StateDigest>,
    /// Nodes that differ from the majority.
    pub differing_nodes: Vec<NodeId>,
    /// The majority digest (most common).
    pub majority_digest: Option<StateDigest>,
}

/// Input for convergence checking.
#[derive(Debug, Clone)]
pub struct ConvergenceInput {
    /// Number of messages in flight.
    pub in_flight_messages: usize,
    /// Per-node state.
    pub nodes: Vec<NodeConvergenceState>,
}

/// Per-node state for convergence checking.
#[derive(Debug, Clone)]
pub struct NodeConvergenceState {
    /// Node ID.
    pub id: NodeId,
    /// Whether sync is active.
    pub sync_active: bool,
    /// Number of deltas in buffer.
    pub buffer_size: usize,
    /// Number of active sync timers.
    pub sync_timer_count: usize,
    /// State digest.
    pub digest: StateDigest,
}

/// Check convergence according to spec §8.1.
///
/// Properties checked in order (fast-fail):
/// 1. C1: Network quiescent
/// 2. C2: All nodes idle
/// 3. C3: No pending buffers
/// 4. C4: No pending sync timers
/// 5. C5: State digests equal
pub fn check_convergence(input: &ConvergenceInput) -> ConvergenceResult {
    // C1: Network quiescent
    if input.in_flight_messages > 0 {
        return ConvergenceResult::Pending(ConvergencePending {
            blocking_property: ConvergenceProperty::NetworkQuiescent,
            reason: format!("{} messages in flight", input.in_flight_messages),
        });
    }

    // C2: All nodes idle
    for node in &input.nodes {
        if node.sync_active {
            return ConvergenceResult::Pending(ConvergencePending {
                blocking_property: ConvergenceProperty::AllNodesIdle,
                reason: format!("node {} has sync active", node.id),
            });
        }
    }

    // C3: No pending buffers
    for node in &input.nodes {
        if node.buffer_size > 0 {
            return ConvergenceResult::Pending(ConvergencePending {
                blocking_property: ConvergenceProperty::NoPendingBuffers,
                reason: format!("node {} has {} buffered deltas", node.id, node.buffer_size),
            });
        }
    }

    // C4: No pending sync timers
    for node in &input.nodes {
        if node.sync_timer_count > 0 {
            return ConvergenceResult::Pending(ConvergencePending {
                blocking_property: ConvergenceProperty::NoPendingSyncTimers,
                reason: format!("node {} has {} sync timers", node.id, node.sync_timer_count),
            });
        }
    }

    // C5: State digests equal
    if input.nodes.is_empty() {
        return ConvergenceResult::Converged;
    }

    let first_digest = input.nodes[0].digest;
    let all_equal = input.nodes.iter().all(|n| n.digest == first_digest);

    if all_equal {
        return ConvergenceResult::Converged;
    }

    // Compute diff
    let mut digests = HashMap::new();
    let mut digest_counts: HashMap<StateDigest, usize> = HashMap::new();

    for node in &input.nodes {
        digests.insert(node.id.clone(), node.digest);
        *digest_counts.entry(node.digest).or_default() += 1;
    }

    // Find majority with deterministic tiebreaker (sort by digest value on ties)
    let majority_digest = digest_counts
        .iter()
        .max_by(|(d1, c1), (d2, c2)| {
            c1.cmp(c2).then_with(|| {
                // On count tie, use lexicographic ordering of digest bytes for determinism
                d1.0.cmp(&d2.0)
            })
        })
        .map(|(digest, _)| *digest);

    // Find differing nodes
    let differing_nodes: Vec<_> = input
        .nodes
        .iter()
        .filter(|n| Some(n.digest) != majority_digest)
        .map(|n| n.id.clone())
        .collect();

    ConvergenceResult::Diverged(ConvergenceDiff {
        digests,
        differing_nodes,
        majority_digest,
    })
}

/// Deadlock detection according to spec §5.3.
///
/// System is in deadlock when ALL of:
/// - Event queue is empty
/// - System has NOT converged
/// - At least one of:
///   - Some node has sync_state != Idle
///   - Some node has non-empty delta buffer
///   - Some node has pending sync timers
pub fn is_deadlocked(input: &ConvergenceInput, queue_empty: bool) -> bool {
    if !queue_empty {
        return false;
    }

    let result = check_convergence(input);
    if result.is_converged() {
        return false;
    }

    // Check if stuck
    let has_active_sync = input.nodes.iter().any(|n| n.sync_active);
    let has_buffered = input.nodes.iter().any(|n| n.buffer_size > 0);
    let has_timers = input.nodes.iter().any(|n| n.sync_timer_count > 0);

    has_active_sync || has_buffered || has_timers
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_node(id: &str, digest: [u8; 32]) -> NodeConvergenceState {
        NodeConvergenceState {
            id: NodeId::new(id),
            sync_active: false,
            buffer_size: 0,
            sync_timer_count: 0,
            digest: StateDigest::from_bytes(digest),
        }
    }

    #[test]
    fn test_converged_empty() {
        let input = ConvergenceInput {
            in_flight_messages: 0,
            nodes: vec![],
        };

        assert!(check_convergence(&input).is_converged());
    }

    #[test]
    fn test_converged_single_node() {
        let input = ConvergenceInput {
            in_flight_messages: 0,
            nodes: vec![make_node("a", [1; 32])],
        };

        assert!(check_convergence(&input).is_converged());
    }

    #[test]
    fn test_converged_matching_digests() {
        let input = ConvergenceInput {
            in_flight_messages: 0,
            nodes: vec![
                make_node("a", [1; 32]),
                make_node("b", [1; 32]),
                make_node("c", [1; 32]),
            ],
        };

        assert!(check_convergence(&input).is_converged());
    }

    #[test]
    fn test_pending_messages_in_flight() {
        let input = ConvergenceInput {
            in_flight_messages: 5,
            nodes: vec![make_node("a", [1; 32]), make_node("b", [1; 32])],
        };

        let result = check_convergence(&input);
        assert!(result.is_pending());

        if let ConvergenceResult::Pending(p) = result {
            assert_eq!(p.blocking_property, ConvergenceProperty::NetworkQuiescent);
        }
    }

    #[test]
    fn test_pending_sync_active() {
        let mut node = make_node("a", [1; 32]);
        node.sync_active = true;

        let input = ConvergenceInput {
            in_flight_messages: 0,
            nodes: vec![node, make_node("b", [1; 32])],
        };

        let result = check_convergence(&input);
        assert!(result.is_pending());

        if let ConvergenceResult::Pending(p) = result {
            assert_eq!(p.blocking_property, ConvergenceProperty::AllNodesIdle);
        }
    }

    #[test]
    fn test_pending_buffer_not_empty() {
        let mut node = make_node("a", [1; 32]);
        node.buffer_size = 3;

        let input = ConvergenceInput {
            in_flight_messages: 0,
            nodes: vec![node, make_node("b", [1; 32])],
        };

        let result = check_convergence(&input);
        assert!(result.is_pending());

        if let ConvergenceResult::Pending(p) = result {
            assert_eq!(p.blocking_property, ConvergenceProperty::NoPendingBuffers);
        }
    }

    #[test]
    fn test_pending_sync_timers() {
        let mut node = make_node("a", [1; 32]);
        node.sync_timer_count = 1;

        let input = ConvergenceInput {
            in_flight_messages: 0,
            nodes: vec![node, make_node("b", [1; 32])],
        };

        let result = check_convergence(&input);
        assert!(result.is_pending());

        if let ConvergenceResult::Pending(p) = result {
            assert_eq!(
                p.blocking_property,
                ConvergenceProperty::NoPendingSyncTimers
            );
        }
    }

    #[test]
    fn test_diverged() {
        let input = ConvergenceInput {
            in_flight_messages: 0,
            nodes: vec![
                make_node("a", [1; 32]),
                make_node("b", [1; 32]),
                make_node("c", [2; 32]), // Different!
            ],
        };

        let result = check_convergence(&input);
        assert!(result.is_diverged());

        if let ConvergenceResult::Diverged(diff) = result {
            assert_eq!(diff.differing_nodes.len(), 1);
            assert!(diff.differing_nodes.contains(&NodeId::new("c")));
            assert_eq!(diff.majority_digest, Some(StateDigest::from_bytes([1; 32])));
        }
    }

    #[test]
    fn test_deadlock_detection() {
        // Not deadlocked: queue not empty
        let input = ConvergenceInput {
            in_flight_messages: 0,
            nodes: vec![make_node("a", [1; 32]), make_node("b", [2; 32])],
        };
        assert!(!is_deadlocked(&input, false));

        // Not deadlocked: converged
        let input = ConvergenceInput {
            in_flight_messages: 0,
            nodes: vec![make_node("a", [1; 32]), make_node("b", [1; 32])],
        };
        assert!(!is_deadlocked(&input, true));

        // Deadlocked: diverged, queue empty, sync active
        let mut node = make_node("a", [1; 32]);
        node.sync_active = true;
        let input = ConvergenceInput {
            in_flight_messages: 0,
            nodes: vec![node, make_node("b", [2; 32])],
        };
        assert!(is_deadlocked(&input, true));
    }

    #[test]
    fn test_majority_digest_tiebreaker() {
        // With two digests having equal count, should deterministically pick
        // the lexicographically greater digest (for consistent results)
        let input = ConvergenceInput {
            in_flight_messages: 0,
            nodes: vec![
                make_node("a", [1; 32]), // digest [1; 32]
                make_node("b", [2; 32]), // digest [2; 32]
            ],
        };

        let result = check_convergence(&input);
        if let ConvergenceResult::Diverged(diff) = result {
            // With tie, should pick the lexicographically greater digest [2; 32]
            assert_eq!(
                diff.majority_digest,
                Some(StateDigest::from_bytes([2; 32])),
                "Majority should be deterministic on tie"
            );
        } else {
            panic!("Expected diverged result");
        }

        // Run multiple times to verify determinism
        for _ in 0..10 {
            let result = check_convergence(&input);
            if let ConvergenceResult::Diverged(diff) = result {
                assert_eq!(
                    diff.majority_digest,
                    Some(StateDigest::from_bytes([2; 32])),
                    "Majority should be consistent across calls"
                );
            }
        }
    }
}
