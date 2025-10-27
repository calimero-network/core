//! Storage delta for synchronization.
//!
//! Represents the output of storage operations that needs to be synchronized
//! across nodes using a DAG (Directed Acyclic Graph) structure.

use core::cell::RefCell;
use std::io;

use borsh::{to_vec, BorshDeserialize, BorshSerialize};
use sha2::{Digest, Sha256};

use crate::action::Action;
use crate::env;
use crate::integration::Comparison;

/// A causal delta in the DAG representing a set of CRDT actions.
///
/// Each delta has a unique ID (content hash) and references its parent delta(s),
/// forming a DAG structure that preserves causal ordering.
#[derive(Clone, Debug, BorshSerialize, BorshDeserialize, PartialEq, Eq)]
pub struct CausalDelta {
    /// Unique ID: SHA256(parents || actions || timestamp)
    pub id: [u8; 32],

    /// Parent delta IDs (empty for root, 1 for sequential, 2+ for merges)
    pub parents: Vec<[u8; 32]>,

    /// CRDT actions in this delta
    pub actions: Vec<Action>,

    /// Timestamp for ordering (nanoseconds since epoch)
    pub timestamp: u64,
}

impl CausalDelta {
    /// Compute the ID for a delta
    pub fn compute_id(parents: &[[u8; 32]], actions: &[Action], timestamp: u64) -> [u8; 32] {
        let mut hasher = Sha256::new();

        // Hash parents
        for parent in parents {
            hasher.update(parent);
        }

        // Hash actions
        if let Ok(actions_bytes) = to_vec(actions) {
            hasher.update(&actions_bytes);
        }

        // Hash timestamp
        hasher.update(&timestamp.to_le_bytes());

        hasher.finalize().into()
    }
}

/// Delta produced by storage operations for synchronization.
///
/// Contains either a list of actions (operation-based CRDT) or comparisons
/// (state-based CRDT for Merkle tree reconciliation).
#[derive(Debug, BorshSerialize)]
pub enum StorageDelta {
    /// A list of actions from direct operations.
    Actions(Vec<Action>),
    /// A list of comparisons for Merkle tree sync.
    Comparisons(Vec<Comparison>),
}

impl BorshDeserialize for StorageDelta {
    fn deserialize_reader<R: io::Read>(reader: &mut R) -> io::Result<Self> {
        let Ok(tag) = u8::deserialize_reader(reader) else {
            return Ok(StorageDelta::Comparisons(vec![]));
        };

        match tag {
            0 => Ok(StorageDelta::Actions(Vec::deserialize_reader(reader)?)),
            1 => Ok(StorageDelta::Comparisons(Vec::deserialize_reader(reader)?)),
            _ => Err(io::Error::new(io::ErrorKind::InvalidData, "Invalid tag")),
        }
    }
}

/// Thread-local context for DAG delta creation
struct DeltaContext {
    actions: Vec<Action>,
    comparisons: Vec<Comparison>,
    current_heads: Vec<[u8; 32]>,
}

impl DeltaContext {
    const fn new() -> Self {
        Self {
            actions: Vec::new(),
            comparisons: Vec::new(),
            current_heads: Vec::new(),
        }
    }
}

thread_local! {
    static DELTA_CONTEXT: RefCell<DeltaContext> = const { RefCell::new(DeltaContext::new()) };
}

/// Records an action for eventual synchronisation.
///
/// # Parameters
///
/// * `action` - The action to record.
///
pub fn push_action(action: Action) {
    DELTA_CONTEXT.with(|ctx| ctx.borrow_mut().actions.push(action));
}

/// Records a comparison for eventual synchronisation.
///
/// # Parameters
///
/// * `comparison` - The comparison to record.
///
pub fn push_comparison(comparison: Comparison) {
    DELTA_CONTEXT.with(|ctx| ctx.borrow_mut().comparisons.push(comparison));
}

/// Sets the current DAG heads for the next delta.
///
/// This should be called when initializing a context or after receiving deltas from peers.
pub fn set_current_heads(heads: Vec<[u8; 32]>) {
    DELTA_CONTEXT.with(|ctx| {
        ctx.borrow_mut().current_heads = heads;
    });
}

/// Gets the current DAG heads.
pub fn get_current_heads() -> Vec<[u8; 32]> {
    DELTA_CONTEXT.with(|ctx| ctx.borrow().current_heads.clone())
}

/// Creates a causal delta from the current context and commits it.
///
/// Returns the created CausalDelta which should be broadcast to peers.
///
/// # Errors
///
/// This function will return an error if there are issues serializing the delta.
pub fn commit_causal_delta(root_hash: &[u8; 32]) -> eyre::Result<Option<CausalDelta>> {
    DELTA_CONTEXT.with(|ctx| {
        let mut context = ctx.borrow_mut();

        // If no actions or comparisons, nothing to commit
        if context.actions.is_empty() && context.comparisons.is_empty() {
            return Ok(None);
        }

        // Get current timestamp
        let timestamp = env::time_now();

        // Create delta with current heads as parents
        let parents = std::mem::take(&mut context.current_heads);
        let actions = std::mem::take(&mut context.actions);
        let _comparisons = std::mem::take(&mut context.comparisons);

        // Compute ID
        let id = CausalDelta::compute_id(&parents, &actions, timestamp);

        let delta = CausalDelta {
            id,
            parents,
            actions,
            timestamp,
        };

        // Update heads - this delta is now the new head
        context.current_heads = vec![delta.id];

        // Serialize for environment
        let artifact = to_vec(&StorageDelta::Actions(delta.actions.clone()))?;
        env::commit(root_hash, &artifact);

        Ok(Some(delta))
    })
}

/// Commits the root hash to the runtime (legacy compatibility).
/// This will also commit any recorded actions or comparisons.
/// If both actions and comparisons are present, this function will panic.
/// This function must only be called once.
///
/// # Errors
///
/// This function will return an error if there are issues accessing local
/// data or if there are problems during the comparison process.
///
pub fn commit_root(root_hash: &[u8; 32]) -> eyre::Result<()> {
    DELTA_CONTEXT.with(|ctx| {
        let mut context = ctx.borrow_mut();

        let actions = std::mem::take(&mut context.actions);
        let comparisons = std::mem::take(&mut context.comparisons);

        let artifact = match (&*actions, &*comparisons) {
            (&[], &[]) => vec![],
            (&[], _) => to_vec(&StorageDelta::Comparisons(comparisons))?,
            (_, &[]) => to_vec(&StorageDelta::Actions(actions))?,
            _ => eyre::bail!("both actions and comparison are present"),
        };

        env::commit(root_hash, &artifact);

        Ok(())
    })
}

/// Resets the delta context for testing.
///
/// Clears all pending actions, comparisons, and heads. Use this between
/// test commits to simulate separate execution contexts.
#[cfg(test)]
pub fn reset_delta_context() {
    DELTA_CONTEXT.with(|ctx| {
        *ctx.borrow_mut() = DeltaContext::new();
    });
}
