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
use crate::logical_clock::HybridTimestamp;

/// A causal delta in the DAG representing a set of CRDT actions.
///
/// Each delta has a unique ID (content hash) and references its parent delta(s),
/// forming a DAG structure that preserves causal ordering.
///
/// # Timestamp Strategy
///
/// Uses Hybrid Logical Clock (HLC) which contains:
/// - **Logical clock**: Guarantees causal ordering
/// - **Physical time**: Embedded in NTP64 format (first 32 bits = seconds since epoch)
///
/// The DAG provides coarse-grained ordering (delta-level), while HLC provides
/// fine-grained ordering (action-level).
///
/// **Note**: The delta ID does NOT include the HLC to ensure determinism.
/// Nodes executing the same operations produce identical IDs regardless of physical time.
#[derive(Clone, Debug, BorshSerialize, BorshDeserialize, PartialEq, Eq)]
pub struct CausalDelta {
    /// Unique ID: SHA256(parents || actions) - deterministic, excludes timestamp
    pub id: [u8; 32],

    /// Parent delta IDs (empty for root, 1 for sequential, 2+ for merges)
    pub parents: Vec<[u8; 32]>,

    /// CRDT actions in this delta
    pub actions: Vec<Action>,

    /// Hybrid timestamp for this delta (last/max HLC of actions).
    ///
    /// Provides both:
    /// - Causal ordering across deltas (logical clock)
    /// - Wall-clock semantics (physical time embedded in NTP64)
    pub hlc: HybridTimestamp,

    /// Expected root hash after applying this delta.
    ///
    /// This ensures deterministic DAG structure across nodes even when
    /// WASM execution produces different root hashes due to non-determinism.
    /// During sync, receiving nodes MUST use this hash rather than their
    /// computed hash to maintain DAG consistency.
    pub expected_root_hash: [u8; 32],
}

impl CausalDelta {
    /// Compute the ID for a delta
    ///
    /// The ID is deterministic based on parents and actions only.
    /// Timestamps are excluded to ensure nodes computing the same
    /// operations produce identical delta IDs regardless of physical time.
    pub fn compute_id(
        parents: &[[u8; 32]],
        actions: &[Action],
        _hlc: &HybridTimestamp,
    ) -> [u8; 32] {
        let mut hasher = Sha256::new();

        // Hash parents
        for parent in parents {
            hasher.update(parent);
        }

        // Hash actions WITHOUT metadata timestamps to ensure determinism
        // Serialize only the content-addressable parts: id, data, ancestors (without timestamps)
        for action in actions {
            match action {
                Action::Add { id, data, .. } | Action::Update { id, data, .. } => {
                    let id_bytes: [u8; 32] = (*id).into();
                    hasher.update(&id_bytes);
                    hasher.update(data);
                    // Metadata and ancestors excluded - they contain timestamps
                }
                Action::DeleteRef { id, .. } => {
                    let id_bytes: [u8; 32] = (*id).into();
                    hasher.update(&id_bytes);
                    // deleted_at excluded - it's a timestamp
                }
                Action::Compare { id } => {
                    let id_bytes: [u8; 32] = (*id).into();
                    hasher.update(&id_bytes);
                }
            }
        }

        // HLC is NOT hashed - it's metadata for ordering/LWW conflict resolution.

        hasher.finalize().into()
    }

    /// Get the physical timestamp (nanoseconds since epoch).
    #[must_use]
    pub fn physical_time(&self) -> u64 {
        // Extract physical time from HLC (first 32 bits of NTP64)
        let ntp64 = self.hlc.get_time().as_u64();
        let seconds = ntp64 >> 32;
        // Convert to nanoseconds
        seconds * 1_000_000_000
    }

    /// Get the logical clock value from HLC.
    #[must_use]
    pub fn logical_clock(&self) -> u64 {
        crate::logical_clock::logical_counter(&self.hlc) as u64
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
    /// Maximum HLC timestamp for actions in this delta
    max_hlc: Option<HybridTimestamp>,
}

impl DeltaContext {
    const fn new() -> Self {
        Self {
            actions: Vec::new(),
            comparisons: Vec::new(),
            current_heads: Vec::new(),
            max_hlc: None,
        }
    }

    /// Record an HLC timestamp for an action (tracks maximum)
    fn record_hlc(&mut self, ts: HybridTimestamp) {
        match &mut self.max_hlc {
            None => {
                self.max_hlc = Some(ts);
            }
            Some(max) => {
                if ts > *max {
                    *max = ts;
                }
            }
        }
    }

    /// Get HLC timestamp, creating default if empty
    fn get_hlc(&mut self) -> HybridTimestamp {
        self.max_hlc.unwrap_or_else(|| env::hlc_timestamp())
    }
}

thread_local! {
    static DELTA_CONTEXT: RefCell<DeltaContext> = const { RefCell::new(DeltaContext::new()) };
}

/// Records an action for eventual synchronisation.
///
/// This also captures an HLC timestamp to track fine-grained causal ordering.
///
/// # Parameters
///
/// * `action` - The action to record.
///
pub fn push_action(action: Action) {
    DELTA_CONTEXT.with(|ctx| {
        let mut context = ctx.borrow_mut();

        // Capture HLC timestamp for this action
        let hlc_ts = env::hlc_timestamp();
        context.record_hlc(hlc_ts);

        context.actions.push(action);
    });
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

        // Create delta with current heads as parents
        let parents = std::mem::take(&mut context.current_heads);
        let actions = std::mem::take(&mut context.actions);
        let hlc = context.get_hlc();
        let _comparisons = std::mem::take(&mut context.comparisons);

        // Compute ID
        let id = CausalDelta::compute_id(&parents, &actions, &hlc);

        let delta = CausalDelta {
            id,
            parents,
            actions,
            hlc,
            expected_root_hash: *root_hash,
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
