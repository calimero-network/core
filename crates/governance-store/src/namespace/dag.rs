use calimero_governance_types::NamespaceId;
use std::collections::HashSet;

use calimero_context_client::local_governance::SignedNamespaceOp;
use calimero_store::Store;
use eyre::Result as EyreResult;

use super::op_log::NamespaceOpLogService;

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
    namespace_id: NamespaceId,
}

impl<'a> NamespaceDagService<'a> {
    pub fn new(store: &'a Store, namespace_id: NamespaceId) -> Self {
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
    /// keep emitting a `GovernanceParentEdge` whose duplicate `governance_dag_heads`
    /// every peer rejects (issue #2327). De-dup defensively on read so such a
    /// store self-heals on its next governance op; the `warn!` makes the
    /// condition observable rather than silent.
    pub fn read_head_record(&self) -> EyreResult<NamespaceHead> {
        let handle = self.store.handle();
        let key = calimero_store::key::NamespaceGovHead::new(self.namespace_id.to_bytes());
        let head = handle.get(&key)?;
        let raw_heads = head
            .as_ref()
            .map(|h| h.dag_heads.clone())
            .unwrap_or_default();
        let parent_hashes = dedup_preserving_order(raw_heads);
        if let Some(h) = head.as_ref() {
            if h.dag_heads.len() != parent_hashes.len() {
                tracing::warn!(
                    namespace_id = %hex::encode(self.namespace_id.as_bytes()),
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

    /// Advance the namespace DAG head: drop the heads this op supersedes (its
    /// parents) and add `delta_id`.
    ///
    /// This is a read-modify-write on the single `NamespaceGovHead` key; it is
    /// safe because namespace governance op application is serialized at the
    /// call site (the `ContextManager` actor handler holds the per-namespace
    /// `DagStore` lock — see `Handler<ApplySignedNamespaceOpRequest>`), so
    /// there is no concurrent writer to lose an update to.
    pub fn advance_dag_head(
        &self,
        delta_id: [u8; 32],
        parent_ids: &[[u8; 32]],
        sequence: u64,
    ) -> EyreResult<()> {
        let handle = self.store.handle();
        let ns_key = calimero_store::key::NamespaceGovHead::new(self.namespace_id.to_bytes());
        let current = handle.get(&ns_key)?;
        drop(handle);

        let current_heads: HashSet<[u8; 32]> = current
            .as_ref()
            .map(|h| h.dag_heads.iter().copied().collect())
            .unwrap_or_default();

        // Validate cited parents are RESOLVABLE — either a current head or an
        // op we have already applied and logged. A resolvable parent is trusted
        // to advance the head frontier that `compute_governance_position`
        // depends on; an unresolvable one (absent / fabricated) must not, or a
        // crafted op could pad the frontier and desync every peer's position
        // check. The in-memory DagStore already gates apply on parent
        // availability (an op with absent parents is held `Pending` and its
        // ancestors are back-filled first), so for ops arriving through the
        // normal path every parent is resolvable here; this guard defends the
        // governance-head write against a direct/edge caller that bypasses that
        // gate, and makes the anomaly observable rather than silent.
        let op_log = NamespaceOpLogService::new(self.store, self.namespace_id);
        let mut resolvable_parents: HashSet<[u8; 32]> = HashSet::with_capacity(parent_ids.len());
        for parent in parent_ids {
            if current_heads.contains(parent) || op_log.contains_op(*parent)? {
                let _ = resolvable_parents.insert(*parent);
            } else {
                tracing::warn!(
                    namespace_id = %hex::encode(self.namespace_id.as_bytes()),
                    delta_id = %hex::encode(delta_id),
                    parent = %hex::encode(parent),
                    "advance_dag_head: op cites an unresolvable parent (not a current \
                     head nor a known applied op); ignoring it for head supersession"
                );
            }
        }

        // Only resolvable parents may supersede current heads. Unresolvable ones
        // are dropped, so they can neither remove a real head nor otherwise
        // influence the frontier.
        let parent_set = resolvable_parents;
        // Drop the heads this op supersedes (its parents) and collapse any
        // pre-existing duplicates: a stored head set must be unique, otherwise
        // `compute_governance_position` refuses to embed a position and every
        // peer rejects the node's deltas ("author is not a member of the group
        // at governance cut") — see issue #2327. (Collapsing here also heals a
        // store corrupted by an older build on its next governance op.)
        let mut new_heads = dedup_preserving_order(
            current
                .map(|h| h.dag_heads)
                .unwrap_or_default()
                .into_iter()
                .filter(|h| !parent_set.contains(h))
                .collect(),
        );
        // `delta_id` may already be a head when this exact op is applied more
        // than once (e.g. a node re-receives, via sync backfill, an op it had
        // already applied or published locally — the in-memory DagStore's
        // dedup set doesn't cover the publisher path). Re-applying it must be
        // a no-op for the head set, not a second entry.
        if !new_heads.contains(&delta_id) {
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
