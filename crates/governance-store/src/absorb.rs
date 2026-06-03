//! Typed Repository over the [`Column::AbsorbBuffer`] CF (PR-6b straggler
//! safety).
//!
//! Persists [`AbsorbRecord`]s — the durable mirror of a stale-schema straggler
//! delta — keyed by `prefix ‖ context_id ‖ producing_app_key ‖ delta_id`. The
//! `delta_id` in the key makes [`save`](AbsorbRepository::save) idempotent: a
//! re-delivered straggler delta overwrites its previous buffered copy rather
//! than duplicating it.
//!
//! Mirrors [`UpgradesRepository`](crate::UpgradesRepository) in shape: a thin
//! `{ store }` handle exposing save/load/delete plus a contiguous-range scan
//! ([`enumerate_pending`](AbsorbRepository::enumerate_pending)) used by the
//! drain-on-advance and crash-recovery paths.

use borsh::BorshDeserialize;
use calimero_primitives::context::ContextId;
use calimero_store::key::{AbsorbBufferKey, ABSORB_BUFFER_PREFIX};
use calimero_store::Store;
use eyre::Result as EyreResult;

use super::collect_keys_with_prefix;
use crate::AbsorbRecord;

/// Typed Repository for the per-context absorb buffer.
///
/// Holds one [`AbsorbRecord`] per `(context, producing_app_key, delta_id)`
/// (save/load/delete) plus a per-context contiguous scan for the drain and
/// crash-recovery paths. See [`UpgradesRepository`](crate::UpgradesRepository)
/// for the Repository pattern's rationale — same shape.
///
/// PR-6b / issue #2539.
pub struct AbsorbRepository<'a> {
    store: &'a Store,
}

impl<'a> AbsorbRepository<'a> {
    pub fn new(store: &'a Store) -> Self {
        Self { store }
    }

    /// Persist a straggler delta's durable mirror. Idempotent on the
    /// `delta_id`: a re-delivered straggler overwrites rather than duplicates,
    /// because the `delta_id` is part of the key.
    pub fn save(
        &self,
        context_id: &ContextId,
        producing_app_key: [u8; 32],
        record: &AbsorbRecord,
    ) -> EyreResult<()> {
        let mut handle = self.store.handle();
        let key = AbsorbBufferKey::new(*context_id.as_ref(), producing_app_key, record.id);
        // `AbsorbRecord` lives in this crate; the store CF stores it as an
        // opaque borsh byte blob (see the `PredefinedEntry` impl), so encode
        // here.
        let bytes = borsh::to_vec(record)?;
        handle.put(&key, &bytes)?;
        Ok(())
    }

    pub fn load(
        &self,
        context_id: &ContextId,
        producing_app_key: [u8; 32],
        delta_id: [u8; 32],
    ) -> EyreResult<Option<AbsorbRecord>> {
        let handle = self.store.handle();
        let key = AbsorbBufferKey::new(*context_id.as_ref(), producing_app_key, delta_id);
        match handle.get(&key)? {
            Some(bytes) => Ok(Some(AbsorbRecord::try_from_slice(&bytes)?)),
            None => Ok(None),
        }
    }

    pub fn delete(
        &self,
        context_id: &ContextId,
        producing_app_key: [u8; 32],
        delta_id: [u8; 32],
    ) -> EyreResult<()> {
        let mut handle = self.store.handle();
        let key = AbsorbBufferKey::new(*context_id.as_ref(), producing_app_key, delta_id);
        handle.delete(&key)?;
        Ok(())
    }

    /// Scan the absorb buffer for every pending record belonging to
    /// `context_id`, returning `((producing_app_key, delta_id), record)`
    /// pairs. Used by the drain-on-advance and crash-recovery paths.
    ///
    /// `collect_keys_with_prefix` seeks directly to the target context's block
    /// and `break`s the moment the `belongs` closure first returns false. Since
    /// `AbsorbBuffer` keys sort lexicographically by `context_id`, the scan must
    /// start at the *target* context (not context 0) so it lands on the
    /// contiguous per-context block; otherwise any smaller context present in
    /// the CF fails `belongs` on the very first key and terminates the scan
    /// early. Mirrors `CapabilitiesRepository::enumerate_members`.
    pub fn enumerate_pending(
        &self,
        context_id: &ContextId,
    ) -> EyreResult<Vec<(([u8; 32], [u8; 32]), AbsorbRecord)>> {
        let target = *context_id.as_ref();
        let keys = collect_keys_with_prefix(
            self.store,
            AbsorbBufferKey::new(target, [0u8; 32], [0u8; 32]),
            ABSORB_BUFFER_PREFIX,
            |key: &AbsorbBufferKey| key.context_id() == target,
        )?;
        let handle = self.store.handle();
        let mut results = Vec::new();
        for key in keys {
            if let Some(bytes) = handle.get(&key)? {
                let record = AbsorbRecord::try_from_slice(&bytes)?;
                results.push(((key.producing_app_key(), key.delta_id()), record));
            }
        }
        Ok(results)
    }

    /// Distinct contexts that have at least one pending absorbed delta. Used by
    /// the startup recovery scan to know which contexts to attempt to drain.
    pub fn enumerate_all_contexts(&self) -> EyreResult<Vec<ContextId>> {
        let keys = collect_keys_with_prefix(
            self.store,
            AbsorbBufferKey::new([0u8; 32], [0u8; 32], [0u8; 32]),
            ABSORB_BUFFER_PREFIX,
            |_| true,
        )?;
        let mut contexts = Vec::new();
        for key in keys {
            let context_id = ContextId::from(key.context_id());
            if !contexts.contains(&context_id) {
                contexts.push(context_id);
            }
        }
        Ok(contexts)
    }
}

#[cfg(test)]
mod tests {
    use calimero_primitives::hash::Hash;
    use calimero_primitives::identity::PublicKey;
    use calimero_storage::logical_clock::HybridTimestamp;

    use super::*;
    use crate::test_fixtures::test_store;

    fn sample_record(delta_id: [u8; 32]) -> AbsorbRecord {
        AbsorbRecord {
            id: delta_id,
            parents: vec![[1; 32]],
            hlc: HybridTimestamp::zero(),
            payload: vec![1, 2, 3],
            nonce: [0; 12],
            author_id: PublicKey::from([0; 32]),
            root_hash: Hash::from([0; 32]),
            events: None,
            source_peer: libp2p::PeerId::random().to_bytes(),
            key_id: [0; 32],
            governance_position: None,
            delta_signature: Some([9; 64]),
            governance_drain_attempts: 0,
            producing_app_key: Some([2; 32]),
            leaf: None,
            entity: None,
        }
    }

    #[test]
    fn save_then_load_round_trip() {
        let store = test_store();
        let repo = AbsorbRepository::new(&store);
        let ctx = ContextId::from([0xAA; 32]);
        repo.save(&ctx, [9; 32], &sample_record([1; 32])).unwrap();
        let loaded = repo
            .load(&ctx, [9; 32], [1; 32])
            .unwrap()
            .expect("record must round-trip");
        assert_eq!(loaded.id, [1; 32]);
    }

    #[test]
    fn delete_clears_existing_record() {
        let store = test_store();
        let repo = AbsorbRepository::new(&store);
        let ctx = ContextId::from([0xAA; 32]);
        repo.save(&ctx, [9; 32], &sample_record([1; 32])).unwrap();
        repo.delete(&ctx, [9; 32], [1; 32]).unwrap();
        assert!(repo.load(&ctx, [9; 32], [1; 32]).unwrap().is_none());
    }

    #[test]
    fn enumerate_pending_returns_only_this_context() {
        let store = test_store();
        let repo = AbsorbRepository::new(&store);
        let ctx_a = ContextId::from([0xAA; 32]);
        let ctx_b = ContextId::from([0xBB; 32]);
        repo.save(&ctx_a, [9; 32], &sample_record([1; 32])).unwrap();
        repo.save(&ctx_a, [9; 32], &sample_record([2; 32])).unwrap();
        repo.save(&ctx_b, [9; 32], &sample_record([3; 32])).unwrap();
        let pending = repo.enumerate_pending(&ctx_a).unwrap();
        assert_eq!(pending.len(), 2);
    }

    #[test]
    fn enumerate_pending_returns_records_for_larger_context() {
        // Regression: the scan must seek to the *target* context, not context 0.
        // When a lexicographically smaller context (0xAA) also has records, a
        // scan seeded at context 0 would `break` on the first (smaller-context)
        // key and return nothing for the larger context (0xBB).
        let store = test_store();
        let repo = AbsorbRepository::new(&store);
        let ctx_a = ContextId::from([0xAA; 32]);
        let ctx_b = ContextId::from([0xBB; 32]);
        repo.save(&ctx_a, [9; 32], &sample_record([1; 32])).unwrap();
        repo.save(&ctx_b, [9; 32], &sample_record([2; 32])).unwrap();
        let pending = repo.enumerate_pending(&ctx_b).unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!((pending[0].1).id, [2; 32]);
    }

    #[test]
    fn save_is_idempotent_on_delta_id() {
        let store = test_store();
        let repo = AbsorbRepository::new(&store);
        let ctx = ContextId::from([0xAA; 32]);
        repo.save(&ctx, [9; 32], &sample_record([1; 32])).unwrap();
        repo.save(&ctx, [9; 32], &sample_record([1; 32])).unwrap(); // same delta_id key overwrites
        assert_eq!(repo.enumerate_pending(&ctx).unwrap().len(), 1);
    }

    #[test]
    fn enumerate_all_contexts_returns_distinct_contexts() {
        let store = test_store();
        let repo = AbsorbRepository::new(&store);
        let ctx_a = ContextId::from([0xAA; 32]);
        let ctx_b = ContextId::from([0xBB; 32]);
        repo.save(&ctx_a, [9; 32], &sample_record([1; 32])).unwrap();
        repo.save(&ctx_a, [9; 32], &sample_record([2; 32])).unwrap();
        repo.save(&ctx_b, [9; 32], &sample_record([3; 32])).unwrap();
        let contexts = repo.enumerate_all_contexts().unwrap();
        assert_eq!(contexts.len(), 2);
        assert!(contexts.contains(&ctx_a));
        assert!(contexts.contains(&ctx_b));
    }
}
