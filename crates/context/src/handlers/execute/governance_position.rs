//! Computes the [`GovernancePosition`] embedded in outbound state deltas for a
//! group context (the governance-DAG head reference receivers use to gate
//! membership). Extracted from the execute handler.

use calimero_context_config::types::GovernancePosition;
use calimero_governance_store::{MetaRepository, NamespaceRepository};
use calimero_primitives::context::ContextId;
use calimero_store::Store;

/// Compute the [`GovernancePosition`] to embed in the next state delta from
/// this context.
///
/// Returns `None` for non-group contexts (which have no governance DAG to
/// reference) and on any read failure — receivers will surface the missing
/// position via the apply-time membership check rather than silently
/// relying on it.
pub(super) fn compute_governance_position_for_context(
    datastore: &Store,
    context_id: &ContextId,
) -> Option<GovernancePosition> {
    let group_id = match calimero_governance_store::get_group_for_context(datastore, context_id) {
        Ok(Some(gid)) => gid,
        Ok(None) => return None,
        Err(err) => {
            tracing::warn!(
                %context_id,
                %err,
                "compute_governance_position: get_group_for_context failed"
            );
            return None;
        }
    };

    let namespace_id = match NamespaceRepository::new(datastore).resolve(&group_id) {
        Ok(ns_id) => ns_id,
        Err(err) => {
            tracing::warn!(
                %context_id,
                group_id = ?group_id,
                %err,
                "compute_governance_position: resolve_namespace failed"
            );
            return None;
        }
    };

    let dag =
        calimero_governance_store::NamespaceDagService::new(datastore, namespace_id.to_bytes());

    // Double-read pattern: governance ops can apply between reading heads
    // and computing the state hash, producing an internally-inconsistent
    // position whose hash and heads disagree. Re-read heads after the hash
    // and bail if they changed — the receiver's heads-equal fast path
    // treats hash mismatch as a hard rejection, so shipping a stale value
    // would spuriously reject legitimate deltas. A true atomic read would
    // require refactoring `compute_group_state_hash` and `read_head_record`
    // to share a `Handle` (snapshot view); the double-read covers the
    // race window with a single extra cheap read.
    let heads_before = match dag.read_head_record() {
        Ok(head) => head.parent_hashes,
        Err(err) => {
            tracing::warn!(
                %context_id,
                group_id = ?group_id,
                %err,
                "compute_governance_position: read_head_record failed (before)"
            );
            return None;
        }
    };

    let group_state_hash = match MetaRepository::new(datastore).compute_state_hash(&group_id) {
        Ok(hash) => hash,
        Err(err) => {
            tracing::warn!(
                %context_id,
                group_id = ?group_id,
                %err,
                "compute_governance_position: compute_group_state_hash failed"
            );
            return None;
        }
    };

    let heads_after = match dag.read_head_record() {
        Ok(head) => head.parent_hashes,
        Err(err) => {
            tracing::warn!(
                %context_id,
                group_id = ?group_id,
                %err,
                "compute_governance_position: read_head_record failed (after)"
            );
            return None;
        }
    };

    // Set-equality, not Vec equality. Storage iteration order isn't guaranteed
    // to be stable across two reads, so a Vec equality check would treat
    // [h1, h2] vs [h2, h1] as a stale read and emit None for every state delta
    // — receivers then reject the delta on the no-position-on-group-context
    // anti-bypass branch and the wire wedges even when the underlying head
    // set didn't actually change.
    let heads_changed = {
        use std::collections::HashSet;
        heads_before.len() != heads_after.len()
            || heads_before.iter().collect::<HashSet<_>>()
                != heads_after.iter().collect::<HashSet<_>>()
    };
    if heads_changed {
        tracing::warn!(
            %context_id,
            group_id = ?group_id,
            "compute_governance_position: governance heads changed mid-read; \
             skipping position to avoid hash/heads divergence"
        );
        return None;
    }

    match GovernancePosition::new(group_id, group_state_hash, heads_after) {
        Ok(pos) => Some(pos),
        Err(err) => {
            // Local DAG has more heads than MAX_GOVERNANCE_DAG_HEADS allows
            // on the wire — refuse to emit rather than ship a position that
            // the receiver's bounded BorshDeserialize will reject. Indicates
            // either pathological concurrent admin activity or local
            // corruption; logging here surfaces it for operators.
            tracing::warn!(
                %context_id,
                group_id = ?group_id,
                %err,
                "compute_governance_position: refusing to embed oversized position"
            );
            None
        }
    }
}
