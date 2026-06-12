//! The per-context activation marker: which bytecode blob this context last
//! ACTIVATED (a migration commit, or a code-only swap). One fact shared by
//! the sync gate, the lazy trigger, and the migration rollup, with a single
//! up-to-date rule: `marker == group.app_key`.

use calimero_primitives::context::ContextId;
use calimero_store::Store;
use tracing::debug;

/// The blob this context last activated, if the marker is set.
pub fn activated_blob(store: &Store, context_id: &ContextId) -> Option<[u8; 32]> {
    store
        .handle()
        .get(&calimero_store::key::ContextActivatedBlob::new(*context_id))
        .ok()
        .flatten()
        .map(|v| v.blob)
}

/// Record that `context_id` now executes `blob` (migration committed or
/// code-only activation applied). Best-effort: a failed write means the
/// context re-runs its (idempotent) activation on next access.
pub fn record_activation(store: &Store, context_id: &ContextId, blob: [u8; 32]) {
    let mut handle = store.handle();
    if let Err(err) = handle.put(
        &calimero_store::key::ContextActivatedBlob::new(*context_id),
        &calimero_store::types::ContextActivatedBlob { blob },
    ) {
        debug!(%context_id, %err, "failed to record activation marker");
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use calimero_store::db::InMemoryDB;

    use super::*;

    fn store() -> Store {
        Store::new(Arc::new(InMemoryDB::owned()))
    }

    #[test]
    fn marker_roundtrip() {
        let store = store();
        let ctx = ContextId::from([1u8; 32]);
        assert_eq!(activated_blob(&store, &ctx), None);
        record_activation(&store, &ctx, [7u8; 32]);
        assert_eq!(activated_blob(&store, &ctx), Some([7u8; 32]));
        // Moves forward on re-activation.
        record_activation(&store, &ctx, [8u8; 32]);
        assert_eq!(activated_blob(&store, &ctx), Some([8u8; 32]));
    }
}
