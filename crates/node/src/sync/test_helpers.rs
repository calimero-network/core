//! Shared test scaffolding for the node-side DAG-causal Shared verifier
//! tests (the P3 verifier-swap and P5 partition-scenario suites).
//!
//! The signing/action builders these tests need live in
//! `calimero_storage::tests::common`, shared cross-crate via the storage
//! `testing` feature. What stays here is the node-side-specific piece: the
//! [`Dag`] mirror whose `happens_before` reverse-BFS over delta parent links
//! feeds `rotation_log_reader::writers_at`.

use std::collections::{HashMap, HashSet};

/// Tracks a DAG of deltas by parent links. Tests build this incrementally as
/// they author deltas; the [`Dag::happens_before`] predicate is then derived
/// via reverse BFS from the descendant.
#[derive(Default)]
pub(crate) struct Dag {
    parents: HashMap<[u8; 32], Vec<[u8; 32]>>,
}

impl Dag {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn record(&mut self, delta_id: [u8; 32], parents: Vec<[u8; 32]>) {
        self.parents.insert(delta_id, parents);
    }

    pub(crate) fn happens_before(&self, ancestor: &[u8; 32], descendant: &[u8; 32]) -> bool {
        if ancestor == descendant {
            return false;
        }
        let mut frontier: Vec<[u8; 32]> = self.parents.get(descendant).cloned().unwrap_or_default();
        let mut seen: HashSet<[u8; 32]> = HashSet::new();
        while let Some(node) = frontier.pop() {
            if !seen.insert(node) {
                continue;
            }
            if node == *ancestor {
                return true;
            }
            if let Some(ps) = self.parents.get(&node) {
                frontier.extend(ps.iter().copied());
            }
        }
        false
    }
}
