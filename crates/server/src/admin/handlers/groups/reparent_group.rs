use std::sync::Arc;

use axum::extract::Path;
use axum::response::IntoResponse;
use axum::Extension;
use calimero_context::governance_broadcast::ObserveDelivery;
use calimero_context_client::local_governance::{NamespaceOp, RootOp};
use calimero_primitives::identity::PrivateKey;
use calimero_server_primitives::admin::{ReparentGroupApiRequest, ReparentGroupApiResponse};
use tracing::{error, info};

use super::parse_group_id;
use crate::admin::handlers::validation::ValidatedJson;
use crate::admin::service::{parse_api_error, ApiResponse};
use crate::auth::AuthenticatedKey;
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
    auth_key: Option<Extension<AuthenticatedKey>>,
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

    // Prefer the authenticated identity over the caller-supplied requester.
    let requester = auth_key.map(|Extension(k)| k.0).or(req.requester);

    // Resolve the namespace this group belongs to. Reparent is only valid on
    // a child that has a parent — i.e. NOT the namespace root — so the walk
    // always terminates at a real namespace identity (no orphan path).
    let namespace_anchor_group_id =
        match calimero_context::group_store::resolve_namespace(&state.store, &child_group_id) {
            Ok(id) => id,
            Err(err) => return parse_api_error(err).into_response(),
        };
    let (namespace_id, signer_pk, sk_bytes, _sender) =
        match calimero_context::group_store::get_or_create_namespace_identity(
            &state.store,
            &namespace_anchor_group_id,
        ) {
            Ok(result) => result,
            Err(err) => return parse_api_error(err).into_response(),
        };

    if let Some(requester) = requester {
        if requester != signer_pk {
            return parse_api_error(eyre::eyre!(
                "requester does not match local namespace identity"
            ))
            .into_response();
        }
    }

    let signer_sk = PrivateKey::from(sk_bytes);
    let op = NamespaceOp::Root(RootOp::GroupReparented {
        child_group_id: child_group_id.to_bytes(),
        new_parent_id: new_parent_id.to_bytes(),
    });

    info!(child=%group_id_str, new_parent=%req.new_parent_id, "Reparenting subgroup");

    // Pre-check: was this group already under new_parent? If so, the op
    // application will be a no-op. We compute this BEFORE publishing so the
    // response accurately reflects whether anything changed. (Reading the
    // local store is sufficient — if the local view says "already there",
    // the no-op will replicate as a no-op everywhere.)
    let was_already_there = matches!(
        calimero_context::group_store::get_parent_group(&state.store, &child_group_id),
        Ok(Some(p)) if p == new_parent_id,
    );

    match calimero_context::group_store::sign_apply_and_publish_namespace_op(
        &state.store,
        &state.node_client,
        state.ctx_client.ack_router(),
        namespace_id.to_bytes(),
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
