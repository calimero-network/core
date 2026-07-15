//! Node-local durable buffer for namespace ops parked because a semantic
//! prerequisite (the signer's membership op) has not applied yet. Backed by the
//! never-synced [`Column::NamespacePendingGovOp`](calimero_store::db::Column),
//! keyed `namespace_id ‖ delta_id` so a re-park overwrites. Parked ops are
//! unvalidated, so the buffer is bounded per namespace and oldest-evicted.

use borsh::BorshDeserialize;
use calimero_context_client::local_governance::SignedNamespaceOp;
use calimero_governance_types::NamespaceId;
use calimero_store::key::{NamespacePendingGovOp, NAMESPACE_PENDING_GOV_OP_PREFIX};
use calimero_store::Store;
use eyre::Result as EyreResult;

use crate::collect_keys_with_prefix;

/// Per-namespace cap on parked ops, bounding memory under a flood of
/// never-valid ops; an evicted real waiter is re-parked on the next catch-up.
pub const MAX_PENDING_OPS_PER_NAMESPACE: usize = 128;

/// Typed repository over the parked-op buffer for one namespace.
pub struct NamespacePendingOpRepository<'a> {
    store: &'a Store,
    namespace_id: NamespaceId,
}

impl<'a> NamespacePendingOpRepository<'a> {
    pub fn new(store: &'a Store, namespace_id: NamespaceId) -> Self {
        Self {
            store,
            namespace_id,
        }
    }

    /// Park `op` for later re-attempt. Idempotent on the op's content hash; only
    /// a genuinely new op counts against the cap.
    pub fn park(&self, op: &SignedNamespaceOp) -> EyreResult<()> {
        let delta_id = op
            .content_hash()
            .map_err(|e| eyre::eyre!("content_hash: {e}"))?;
        let key = NamespacePendingGovOp::new(self.namespace_id.to_bytes(), delta_id);
        let mut handle = self.store.handle();
        if !handle.has(&key)? {
            self.evict_if_full()?;
        }
        let bytes = borsh::to_vec(op).map_err(|e| eyre::eyre!("borsh: {e}"))?;
        handle.put(&key, &bytes)?;
        Ok(())
    }

    /// Remove a parked op by its content hash. Idempotent — a no-op if absent.
    pub fn remove(&self, delta_id: [u8; 32]) -> EyreResult<()> {
        let mut handle = self.store.handle();
        let key = NamespacePendingGovOp::new(self.namespace_id.to_bytes(), delta_id);
        handle.delete(&key)?;
        Ok(())
    }

    /// Every parked op for this namespace as `(delta_id, op)` pairs.
    pub fn list(&self) -> EyreResult<Vec<([u8; 32], SignedNamespaceOp)>> {
        let ns = self.namespace_id.to_bytes();
        let keys = collect_keys_with_prefix(
            self.store,
            NamespacePendingGovOp::new(ns, [0u8; 32]),
            NAMESPACE_PENDING_GOV_OP_PREFIX,
            |key: &NamespacePendingGovOp| key.namespace_id() == ns,
        )?;
        let handle = self.store.handle();
        let mut ops = Vec::new();
        for key in keys {
            if let Some(bytes) = handle.get(&key)? {
                match SignedNamespaceOp::try_from_slice(&bytes) {
                    Ok(op) => ops.push((key.delta_id(), op)),
                    // A corrupt entry can never re-apply — drop it, don't leak a cap slot.
                    Err(e) => {
                        tracing::warn!(
                            namespace_id = %hex::encode(ns),
                            delta_id = %hex::encode(key.delta_id()),
                            %e,
                            "namespace pending-op buffer: corrupt entry; discarding"
                        );
                        let mut h = self.store.handle();
                        h.delete(&key)?;
                    }
                }
            }
        }
        Ok(ops)
    }

    /// Number of parked ops for this namespace.
    pub fn count(&self) -> EyreResult<usize> {
        let ns = self.namespace_id.to_bytes();
        Ok(collect_keys_with_prefix(
            self.store,
            NamespacePendingGovOp::new(ns, [0u8; 32]),
            NAMESPACE_PENDING_GOV_OP_PREFIX,
            |key: &NamespacePendingGovOp| key.namespace_id() == ns,
        )?
        .len())
    }

    /// Evict one entry (lowest `delta_id`) when at capacity. Not strictly FIFO —
    /// content hashes carry no arrival time — but bounding memory is all we need.
    fn evict_if_full(&self) -> EyreResult<()> {
        let ns = self.namespace_id.to_bytes();
        let keys = collect_keys_with_prefix(
            self.store,
            NamespacePendingGovOp::new(ns, [0u8; 32]),
            NAMESPACE_PENDING_GOV_OP_PREFIX,
            |key: &NamespacePendingGovOp| key.namespace_id() == ns,
        )?;
        if keys.len() >= MAX_PENDING_OPS_PER_NAMESPACE {
            if let Some(victim) = keys.first() {
                let mut handle = self.store.handle();
                handle.delete(victim)?;
                tracing::debug!(
                    namespace_id = %hex::encode(ns),
                    evicted = %hex::encode(victim.delta_id()),
                    cap = MAX_PENDING_OPS_PER_NAMESPACE,
                    "namespace pending-op buffer at capacity; evicted oldest parked op"
                );
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use calimero_context_client::local_governance::{NamespaceOp, RootOp};
    use calimero_context_config::types::ContextGroupId;
    use calimero_primitives::identity::PrivateKey;
    use rand::rngs::OsRng;

    use super::*;
    use crate::test_fixtures::test_store;

    fn signed_op(sk: &PrivateKey, ns: [u8; 32], nonce: u64) -> SignedNamespaceOp {
        SignedNamespaceOp::sign(
            sk,
            ns.into(),
            vec![],
            nonce,
            NamespaceOp::Root(RootOp::MemberJoinedOpen {
                member: sk.public_key(),
                group_id: ContextGroupId::from([0x11; 32]),
            }),
        )
        .unwrap()
    }

    #[test]
    fn park_list_remove_round_trip_and_is_idempotent() {
        let store = test_store();
        let sk = PrivateKey::random(&mut OsRng);
        let ns = [0x01; 32];
        let repo = NamespacePendingOpRepository::new(&store, ns.into());

        let op = signed_op(&sk, ns, 1);
        let delta_id = op.content_hash().unwrap();

        repo.park(&op).unwrap();
        repo.park(&op).unwrap(); // re-park overwrites, does not duplicate
        assert_eq!(repo.count().unwrap(), 1);

        let listed = repo.list().unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].0, delta_id);
        assert_eq!(listed[0].1.signer, sk.public_key());

        repo.remove(delta_id).unwrap();
        assert_eq!(repo.count().unwrap(), 0);
        repo.remove(delta_id).unwrap(); // idempotent
    }

    #[test]
    fn park_is_bounded_and_evicts_at_capacity() {
        let store = test_store();
        let sk = PrivateKey::random(&mut OsRng);
        let ns = [0x02; 32];
        let repo = NamespacePendingOpRepository::new(&store, ns.into());

        // A flood of distinct ops (distinct nonces => distinct content hashes)
        // must never exceed the per-namespace cap.
        for nonce in 0..(MAX_PENDING_OPS_PER_NAMESPACE as u64 + 25) {
            repo.park(&signed_op(&sk, ns, nonce)).unwrap();
            assert!(repo.count().unwrap() <= MAX_PENDING_OPS_PER_NAMESPACE);
        }
        assert_eq!(repo.count().unwrap(), MAX_PENDING_OPS_PER_NAMESPACE);
    }

    #[test]
    fn buffers_are_isolated_per_namespace() {
        let store = test_store();
        let sk = PrivateKey::random(&mut OsRng);
        let ns_a = [0x0A; 32];
        let ns_b = [0x0B; 32];
        NamespacePendingOpRepository::new(&store, ns_a.into())
            .park(&signed_op(&sk, ns_a, 1))
            .unwrap();
        assert_eq!(
            NamespacePendingOpRepository::new(&store, ns_a.into())
                .count()
                .unwrap(),
            1
        );
        assert_eq!(
            NamespacePendingOpRepository::new(&store, ns_b.into())
                .count()
                .unwrap(),
            0,
            "a namespace's parked ops must not leak into another namespace's buffer"
        );
    }
}
