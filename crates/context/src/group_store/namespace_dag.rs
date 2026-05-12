use std::collections::HashSet;

use calimero_context_client::local_governance::SignedNamespaceOp;
use calimero_store::Store;
use eyre::Result as EyreResult;

use super::namespace_op_log::NamespaceOpLogService;

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
    ///
    /// The persisted head set is unique by construction (see
    /// [`Self::advance_dag_head`]), but a node that was corrupted by an older
    /// build — or any not-yet-found path that double-appends — would otherwise
    /// keep emitting a `GovernancePosition` whose duplicate `governance_dag_heads`
    /// every peer rejects (issue #2327). De-dup defensively on read so such a
    /// store self-heals on its next governance op; the `warn!` makes the
    /// condition observable rather than silent.
    pub fn read_head_record(&self) -> EyreResult<NamespaceHead> {
        let handle = self.store.handle();
        let key = calimero_store::key::NamespaceGovHead::new(self.namespace_id);
        let head = handle.get(&key)?;
        let raw_heads = head
            .as_ref()
            .map(|h| h.dag_heads.clone())
            .unwrap_or_default();
        let parent_hashes = dedup_preserving_order(raw_heads);
        if let Some(h) = head.as_ref() {
            if h.dag_heads.len() != parent_hashes.len() {
                tracing::warn!(
                    namespace_id = %hex::encode(self.namespace_id),
                    stored = h.dag_heads.len(),
                    deduped = parent_hashes.len(),
                    "namespace governance DAG head set contained duplicates; \
                     de-duplicated on read (#2327)"
                );
            }
        }
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
        let mut seen: HashSet<[u8; 32]> = HashSet::new();
        let mut new_heads: Vec<[u8; 32]> = current
            .map(|h| h.dag_heads)
            .unwrap_or_default()
            .into_iter()
            // drop heads this op supersedes (they're its parents) ...
            .filter(|h| !parent_set.contains(h))
            // ... and never carry a duplicate forward: a stored head set must
            // be unique, otherwise `compute_governance_position` refuses to
            // embed a position and every peer rejects the node's deltas
            // ("author is not a member of the group at governance cut") — see
            // issue #2327.
            .filter(|h| seen.insert(*h))
            .collect();
        // `delta_id` may already be a head when this exact op is applied more
        // than once (e.g. a node re-receives, via sync backfill, an op it had
        // already applied or published locally — the in-memory DagStore's
        // dedup set doesn't cover the publisher path). Re-applying it must be
        // a no-op for the head set, not a second entry.
        if seen.insert(delta_id) {
            new_heads.push(delta_id);
        }
        debug_assert!(
            {
                let mut s = HashSet::with_capacity(new_heads.len());
                new_heads.iter().all(|h| s.insert(*h))
            },
            "advance_dag_head produced a head set with duplicate entries"
        );

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
        NamespaceOpLogService::new(self.store, self.namespace_id).store_signed_operation(op)
    }

    pub fn collect_skeleton_delta_ids_for_group(
        &self,
        group_id: [u8; 32],
    ) -> EyreResult<Vec<[u8; 32]>> {
        let op_log = NamespaceOpLogService::new(self.store, self.namespace_id);
        op_log.collect_opaque_skeleton_delta_ids_for_group(group_id)
    }
}

/// Drop repeated entries, keeping the first occurrence and the original order.
fn dedup_preserving_order(heads: Vec<[u8; 32]>) -> Vec<[u8; 32]> {
    let mut seen: HashSet<[u8; 32]> = HashSet::with_capacity(heads.len());
    heads.into_iter().filter(|h| seen.insert(*h)).collect()
}
