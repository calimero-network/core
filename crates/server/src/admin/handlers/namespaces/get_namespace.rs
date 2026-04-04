use std::sync::Arc;

use axum::extract::Path;
use axum::response::IntoResponse;
use axum::Extension;
use calimero_context_client::group::GetNamespaceIdentityRequest;
use reqwest::StatusCode;
use tracing::{error, info};

use crate::admin::handlers::groups::parse_group_id;
use crate::admin::service::{ApiError, ApiResponse};
use crate::AdminState;

pub async fn handler(
    Path(namespace_id_str): Path<String>,
    Extension(state): Extension<Arc<AdminState>>,
) -> impl IntoResponse {
    let group_id = match parse_group_id(&namespace_id_str) {
        Ok(id) => id,
        Err(err) => return err.into_response(),
    };

    let node_pk = match state
        .ctx_client
        .get_namespace_identity(GetNamespaceIdentityRequest { group_id })
        .await
    {
        Ok(Some((_, node_pk))) => node_pk,
        Ok(None) => {
            return ApiError {
                status_code: StatusCode::NOT_FOUND,
                message: "No namespace identity found".to_owned(),
            }
            .into_response();
        }
        Err(err) => {
            error!(error=?err, "Failed to get namespace identity");
            return ApiError {
                status_code: StatusCode::INTERNAL_SERVER_ERROR,
                message: "Failed to get namespace identity".to_owned(),
            }
            .into_response();
        }
    };

    info!(namespace_id=%namespace_id_str, "Getting namespace summary");

    let meta = match calimero_context::group_store::load_group_meta(&state.store, &group_id) {
        Ok(Some(meta)) => meta,
        Ok(None) => {
            return ApiError {
                status_code: StatusCode::NOT_FOUND,
                message: "Namespace not found".to_owned(),
            }
            .into_response();
        }
        Err(err) => {
            error!(?err, "Failed to load namespace metadata");
            return ApiError {
                status_code: StatusCode::INTERNAL_SERVER_ERROR,
                message: "Failed to load namespace metadata".to_owned(),
            }
            .into_response();
        }
    };

    match calimero_context::group_store::build_namespace_summary(
        &state.store,
        &group_id,
        &meta,
        &node_pk,
    ) {
        Ok(Some(ns)) => ApiResponse {
            payload: calimero_server_primitives::admin::NamespaceApiResponse {
                namespace_id: hex::encode(ns.namespace_id.to_bytes()),
                app_key: hex::encode(ns.app_key.to_bytes()),
                target_application_id: ns.target_application_id.to_string(),
                upgrade_policy: format!("{:?}", ns.upgrade_policy),
                created_at: ns.created_at,
                alias: ns.alias,
                member_count: ns.member_count,
                context_count: ns.context_count,
                subgroup_count: ns.subgroup_count,
            },
        }
        .into_response(),
        Ok(None) => ApiError {
            status_code: StatusCode::NOT_FOUND,
            message: "Namespace not found".to_owned(),
        }
        .into_response(),
        Err(err) => {
            error!(?err, "Failed to build namespace summary");
            ApiError {
                status_code: StatusCode::INTERNAL_SERVER_ERROR,
                message: "Failed to build namespace summary".to_owned(),
            }
            .into_response()
        }
    }
}
