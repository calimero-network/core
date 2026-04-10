use std::sync::Arc;

use axum::extract::Path;
use axum::response::IntoResponse;
use axum::Extension;
use axum::Json;
use serde::{Deserialize, Serialize};
use tracing::{error, info, warn};

use crate::admin::handlers::groups::parse_group_id;
use crate::admin::service::{parse_api_error, ApiResponse};
use crate::auth::AuthenticatedKey;
use crate::AdminState;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateGroupInNamespaceBody {
    pub group_alias: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateGroupInNamespaceResponseData {
    pub group_id: String,
}

#[derive(Debug, Serialize)]
pub struct CreateGroupInNamespaceResponse {
    pub data: CreateGroupInNamespaceResponseData,
}

pub async fn handler(
    Path(namespace_id_hex): Path<String>,
    Extension(state): Extension<Arc<AdminState>>,
    auth_key: Option<Extension<AuthenticatedKey>>,
    Json(body): Json<CreateGroupInNamespaceBody>,
) -> impl IntoResponse {
    let namespace_id = match parse_group_id(&namespace_id_hex) {
        Ok(id) => id,
        Err(err) => return err.into_response(),
    };
    let requester = auth_key.map(|Extension(k)| k.0);

    info!(
        namespace_id = %namespace_id_hex,
        "Creating group in namespace via namespace governance"
    );

    let group_id: [u8; 32] = {
        use rand::Rng;
        rand::thread_rng().gen()
    };

    match calimero_context::group_store::get_parent_group(&state.store, &namespace_id) {
        Ok(Some(_)) => {
            return parse_api_error(eyre::eyre!("namespace_id must reference a root group"))
                .into_response();
        }
        Ok(None) => {}
        Err(err) => return parse_api_error(err).into_response(),
    }

    let (resolved_ns_id, signer_pk, sk_bytes, _sender) =
        match calimero_context::group_store::get_or_create_namespace_identity(
            &state.store,
            &namespace_id,
        ) {
            Ok(r) => r,
            Err(err) => {
                error!(?err, "Failed to resolve namespace identity");
                return parse_api_error(err).into_response();
            }
        };

    if let Some(requester) = requester {
        if requester != signer_pk {
            return parse_api_error(eyre::eyre!(
                "requester does not match local namespace identity"
            ))
            .into_response();
        }
    }

    let signer_sk = calimero_primitives::identity::PrivateKey::from(sk_bytes);

    let op = calimero_context_client::local_governance::NamespaceOp::Root(
        calimero_context_client::local_governance::RootOp::GroupCreated { group_id },
    );

    match calimero_context::group_store::sign_apply_and_publish_namespace_op(
        &state.store,
        &state.node_client,
        resolved_ns_id.to_bytes(),
        &signer_sk,
        op,
    )
    .await
    {
        Ok(()) => {
            let group_id = calimero_context_config::types::ContextGroupId::from(group_id);

            if let Some(alias) = body.group_alias.as_deref() {
                if let Err(err) =
                    calimero_context::group_store::set_group_alias(&state.store, &group_id, alias)
                {
                    warn!(
                        group_id=%hex::encode(group_id.to_bytes()),
                        ?err,
                        "Group created but failed to persist alias"
                    );
                }
            }

            ApiResponse {
                payload: CreateGroupInNamespaceResponse {
                    data: CreateGroupInNamespaceResponseData {
                        group_id: hex::encode(group_id.to_bytes()),
                    },
                },
            }
            .into_response()
        }
        Err(err) => {
            error!(?err, "Failed to create group in namespace");
            parse_api_error(err).into_response()
        }
    }
}
