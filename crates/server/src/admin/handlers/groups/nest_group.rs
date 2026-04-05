use std::sync::Arc;

use axum::extract::Path;
use axum::response::IntoResponse;
use axum::Extension;
use calimero_context_client::local_governance::{NamespaceOp, RootOp};
use calimero_primitives::identity::PrivateKey;
use calimero_server_primitives::admin::{NestGroupApiRequest, NestGroupApiResponse};
use tracing::{error, info};

use super::parse_group_id;
use crate::admin::handlers::validation::ValidatedJson;
use crate::admin::service::{parse_api_error, ApiResponse};
use crate::auth::AuthenticatedKey;
use crate::AdminState;

pub async fn handler(
    Path(group_id_str): Path<String>,
    Extension(state): Extension<Arc<AdminState>>,
    auth_key: Option<Extension<AuthenticatedKey>>,
    ValidatedJson(req): ValidatedJson<NestGroupApiRequest>,
) -> impl IntoResponse {
    let parent_group_id = match parse_group_id(&group_id_str) {
        Ok(id) => id,
        Err(err) => return err.into_response(),
    };
    let child_group_id = match parse_group_id(&req.child_group_id) {
        Ok(id) => id,
        Err(err) => return err.into_response(),
    };

    let requester = auth_key.map(|Extension(k)| k.0).or(req.requester);

    let namespace_anchor_group_id =
        match calimero_context::group_store::resolve_namespace(&state.store, &parent_group_id) {
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
    let op = NamespaceOp::Root(RootOp::GroupNested {
        parent_group_id: parent_group_id.to_bytes(),
        child_group_id: child_group_id.to_bytes(),
    });

    info!(parent=%group_id_str, child=%req.child_group_id, "Nesting subgroup");

    match calimero_context::group_store::sign_apply_and_publish_namespace_op(
        &state.store,
        &state.node_client,
        namespace_id.to_bytes(),
        &signer_sk,
        op,
    )
    .await
    {
        Ok(()) => ApiResponse {
            payload: NestGroupApiResponse {},
        }
        .into_response(),
        Err(err) => {
            error!(parent=%group_id_str, child=%req.child_group_id, error=?err, "Failed to nest subgroup");
            parse_api_error(err).into_response()
        }
    }
}
