use std::sync::Arc;

use axum::extract::Path;
use axum::response::IntoResponse;
use axum::Extension;
use axum::Json;
use serde::{Deserialize, Serialize};
use tracing::{error, info};

use crate::admin::service::{parse_api_error, ApiResponse};
use crate::AdminState;

#[derive(Debug, Deserialize)]
pub struct CreateGroupInNamespaceBody {
    pub group_alias: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct CreateGroupInNamespaceResponse {
    pub group_id: String,
}

pub async fn handler(
    Path(namespace_id_hex): Path<String>,
    Extension(state): Extension<Arc<AdminState>>,
    Json(_body): Json<CreateGroupInNamespaceBody>,
) -> impl IntoResponse {
    let namespace_id: [u8; 32] = match hex::decode(&namespace_id_hex)
        .ok()
        .and_then(|b| <[u8; 32]>::try_from(b).ok())
    {
        Some(id) => id,
        None => {
            return parse_api_error(eyre::eyre!("invalid namespace_id hex")).into_response();
        }
    };

    info!(
        namespace_id = %namespace_id_hex,
        "Creating group in namespace via namespace governance"
    );

    let group_id: [u8; 32] = {
        use rand::Rng;
        rand::thread_rng().gen()
    };

    let ns_gid = calimero_context_config::types::ContextGroupId::from(namespace_id);
    let (_ns_id, _pk, sk_bytes, _sender) =
        match calimero_context::group_store::get_or_create_namespace_identity(&state.store, &ns_gid)
        {
            Ok(r) => r,
            Err(err) => {
                error!(?err, "Failed to resolve namespace identity");
                return parse_api_error(err).into_response();
            }
        };

    let signer_sk = calimero_primitives::identity::PrivateKey::from(sk_bytes);

    let op = calimero_context_client::local_governance::NamespaceOp::Root(
        calimero_context_client::local_governance::RootOp::GroupCreated { group_id },
    );

    match calimero_context::group_store::sign_apply_and_publish_namespace_op(
        &state.store,
        &state.node_client,
        namespace_id,
        &signer_sk,
        op,
    )
    .await
    {
        Ok(()) => ApiResponse {
            payload: CreateGroupInNamespaceResponse {
                group_id: hex::encode(group_id),
            },
        }
        .into_response(),
        Err(err) => {
            error!(?err, "Failed to create group in namespace");
            parse_api_error(err).into_response()
        }
    }
}
