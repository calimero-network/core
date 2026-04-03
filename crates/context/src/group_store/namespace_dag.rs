use std::collections::HashSet;

use calimero_context_client::local_governance::{OpaqueSkeleton, SignedNamespaceOp};
use calimero_store::Store;
use eyre::{bail, Result as EyreResult};

/// Namespace DAG head view used by governance workflows.
#[derive(Debug, Clone)]
pub struct NamespaceHead {
    pub parent_hashes: Vec<[u8; 32]>,
    pub next_nonce: u64,
}

impl NamespaceHead {
    pub fn into_tuple(self) -> (Vec<[u8; 32]>, u64) {
        (self.parent_hashes, self.next_nonce)
    }
}

/// Domain service for namespace DAG persistence and traversal.
pub struct NamespaceDagService<'a> {
    store: &'a Store,
    namespace_id: [u8; 32],
}

impl<'a> NamespaceDagService<'a> {
    pub fn new(store: &'a Store, namespace_id: [u8; 32]) -> Self {
        Self {
            store,
            namespace_id,
        }
    }

    /// Returns current DAG head as parent hashes + next nonce.
    pub fn read_head_record(&self) -> EyreResult<NamespaceHead> {
        let handle = self.store.handle();
        let key = calimero_store::key::NamespaceGovHead::new(self.namespace_id);
        let head = handle.get(&key)?;
        let parent_hashes = head
            .as_ref()
            .map(|h| h.dag_heads.clone())
            .unwrap_or_default();
        let next_nonce = head.as_ref().map_or(1, |h| h.sequence.saturating_add(1));
        Ok(NamespaceHead {
            parent_hashes,
            next_nonce,
        })
    }

    /// Backward-compatible tuple facade for existing call sites.
    pub fn read_head(&self) -> EyreResult<(Vec<[u8; 32]>, u64)> {
        Ok(self.read_head_record()?.into_tuple())
    }

    pub fn advance_dag_head(
        &self,
        delta_id: [u8; 32],
        parent_ids: &[[u8; 32]],
        sequence: u64,
    ) -> EyreResult<()> {
        let handle = self.store.handle();
        let ns_key = calimero_store::key::NamespaceGovHead::new(self.namespace_id);
        let current = handle.get(&ns_key)?;
        drop(handle);

        let parent_set: HashSet<[u8; 32]> = parent_ids.iter().copied().collect();
        let mut new_heads: Vec<[u8; 32]> = current
            .map(|h| h.dag_heads)
            .unwrap_or_default()
            .into_iter()
            .filter(|h| !parent_set.contains(h))
            .collect();
        new_heads.push(delta_id);

        let mut wh = self.store.handle();
        wh.put(
            &ns_key,
            &calimero_store::key::NamespaceGovHeadValue {
                sequence,
                dag_heads: new_heads,
            },
        )?;
        Ok(())
    }

    /// Persist a namespace governance op in the local DAG log.
    pub fn store_operation(&self, op: &SignedNamespaceOp) -> EyreResult<()> {
        if op.namespace_id != self.namespace_id {
            bail!(
                "namespace mismatch when storing op: handle={}, op={}",
                hex::encode(self.namespace_id),
                hex::encode(op.namespace_id)
            );
        }
        let delta_id = op
            .content_hash()
            .map_err(|e| eyre::eyre!("content_hash: {e}"))?;
        let key = calimero_store::key::NamespaceGovOp::new(self.namespace_id, delta_id);
        let value = calimero_store::key::NamespaceGovOpValue {
            skeleton_bytes: borsh::to_vec(op).map_err(|e| eyre::eyre!("borsh: {e}"))?,
        };
        let mut handle = self.store.handle();
        handle.put(&key, &value)?;
        Ok(())
    }

    pub fn collect_skeleton_delta_ids_for_group(
        &self,
        group_id: [u8; 32],
    ) -> EyreResult<Vec<[u8; 32]>> {
        let handle = self.store.handle();
        let start = calimero_store::key::NamespaceGovOp::new(self.namespace_id, [0u8; 32]);
        let mut iter = handle.iter::<calimero_store::key::NamespaceGovOp>()?;
        let first = iter.seek(start).transpose();
        let mut delta_ids = Vec::new();

        for key_result in first.into_iter().chain(iter.keys()) {
            let key = key_result?;
            if key.namespace_id() != self.namespace_id {
                break;
            }
            if let Some(value) = handle.get(&key)? {
                if let Ok(skeleton) = borsh::from_slice::<OpaqueSkeleton>(&value.skeleton_bytes) {
                    if skeleton.group_id == group_id {
                        delta_ids.push(skeleton.delta_id);
                    }
                }
            }
        }

        Ok(delta_ids)
    }
}
