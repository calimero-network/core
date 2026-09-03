use calimero_governance_store::NamespaceRepository;
use std::sync::Arc;

use axum::extract::Path;
use axum::response::IntoResponse;
use axum::Extension;
use calimero_context_client::local_governance::RootOp;
use calimero_governance_store::governance_broadcast::ObserveDelivery;
use calimero_primitives::identity::PrivateKey;
use calimero_server_primitives::admin::{ReparentGroupApiRequest, ReparentGroupApiResponse};
use tracing::{error, info};

use super::parse_group_id;
use crate::admin::handlers::validation::ValidatedJson;
use crate::admin::service::{parse_api_error, ApiResponse};
use crate::AdminState;

/// `POST /admin-api/groups/:group_id/reparent`
///
/// Atomic edge swap: moves `group_id` (path) under `new_parent_id` (body).
/// Replaces the previous nest/unnest pair — orphan state is structurally
/// impossible. See spec
/// `docs/superpowers/specs/2026-04-22-strict-group-tree-and-cascade-delete.md`.
pub async fn handler(
    Path(group_id_str): Path<String>,
    Extension(state): Extension<Arc<AdminState>>,
    ValidatedJson(req): ValidatedJson<ReparentGroupApiRequest>,
) -> impl IntoResponse {
    let child_group_id = match parse_group_id(&group_id_str) {
        Ok(id) => id,
        Err(err) => return err.into_response(),
    };
    let new_parent_id = match parse_group_id(&req.new_parent_id) {
        Ok(id) => id,
        Err(err) => return err.into_response(),
    };

    // Resolve the namespace this group belongs to. Reparent is only valid on
    // a child that has a parent — i.e. NOT the namespace root — so the walk
    // always terminates at a real namespace identity (no orphan path).
    let namespace_anchor_group_id =
        match NamespaceRepository::new(&state.store).resolve(&child_group_id) {
            Ok(id) => id,
            Err(err) => return parse_api_error(err).into_response(),
        };
    let (namespace_id, _signer_pk, sk_bytes) =
        match NamespaceRepository::new(&state.store).participate_in(&namespace_anchor_group_id) {
            Ok(result) => result,
            Err(err) => return parse_api_error(err).into_response(),
        };

    let signer_sk = PrivateKey::from(sk_bytes);
    // Sealed under the namespace key: where a group sits in the tree is
    // members' business. This site does not decide that — the choke point does.
    let op = match calimero_governance_store::seal_root_op_for_publish(
        &state.store,
        namespace_id.to_bytes().into(),
        RootOp::GroupReparented {
            child_group_id: child_group_id.to_bytes().into(),
            new_parent_id: new_parent_id.to_bytes().into(),
        },
    ) {
        Ok(op) => op,
        Err(err) => return parse_api_error(err).into_response(),
    };

    info!(child=%group_id_str, new_parent=%req.new_parent_id, "Reparenting subgroup");

    // Pre-check: was this group already under new_parent? If so, the op
    // application will be a no-op. We compute this BEFORE publishing so the
    // response accurately reflects whether anything changed. (Reading the
    // local store is sufficient — if the local view says "already there",
    // the no-op will replicate as a no-op everywhere.)
    let was_already_there = matches!(
        NamespaceRepository::new(&state.store).parent(&child_group_id),
        Ok(Some(p)) if p == new_parent_id,
    );

    match calimero_governance_store::sign_apply_and_publish_namespace_op(
        &state.store,
        &state.node_client,
        state.ctx_client.ack_router(),
        namespace_id.to_bytes().into(),
        &signer_sk,
        op,
    )
    .await
    {
        Ok(report) => {
            report.observe("reparent_group", "GroupReparented");
            ApiResponse {
                payload: ReparentGroupApiResponse {
                    reparented: !was_already_there,
                },
            }
            .into_response()
        }
        Err(err) => {
            error!(child=%group_id_str, new_parent=%req.new_parent_id, error=?err, "Failed to reparent subgroup");
            parse_api_error(err).into_response()
        }
    }
}
