//! Computes the [`GovernanceParentEdge`] embedded in outbound state deltas for a
//! group context (the governance-DAG head reference receivers use to gate
//! membership). Extracted from the execute handler.

use calimero_context_config::types::GovernanceParentEdge;
use calimero_governance_store::NamespaceRepository;
use calimero_primitives::context::ContextId;
use calimero_store::Store;

/// Compute the [`GovernanceParentEdge`] to embed in the next state delta from
/// this context.
///
/// Returns `None` for non-group contexts (which have no governance DAG to
/// reference) and on any read failure — receivers will surface the missing
/// position via the apply-time membership check rather than silently
/// relying on it.
pub(super) fn compute_governance_position_for_context(
    datastore: &Store,
    context_id: &ContextId,
) -> Option<GovernanceParentEdge> {
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

    let dag = calimero_governance_store::NamespaceDagService::new(
        datastore,
        namespace_id.to_bytes().into(),
    );

    // The parent edge is just the governance heads at sign time. There is no
    // embedded `group_state_hash` anymore, so the old double-read (which
    // existed only to keep that hash in step with the heads against a
    // concurrent governance apply) is gone — a single head read suffices.
    let heads = match dag.read_head_record() {
        Ok(head) => head.parent_hashes,
        Err(err) => {
            tracing::warn!(
                %context_id,
                group_id = ?group_id,
                %err,
                "compute_governance_edge: read_head_record failed"
            );
            return None;
        }
    };

    match GovernanceParentEdge::new(heads) {
        Ok(edge) => Some(edge),
        Err(err) => {
            // Local DAG has more heads than MAX_GOVERNANCE_DAG_HEADS allows
            // on the wire — refuse to emit rather than ship an edge the
            // receiver's bounded BorshDeserialize will reject. Indicates
            // either pathological concurrent admin activity or local
            // corruption; logging here surfaces it for operators.
            tracing::warn!(
                %context_id,
                group_id = ?group_id,
                %err,
                "compute_governance_edge: refusing to embed oversized edge"
            );
            None
        }
    }
}
