use std::sync::Arc;

use axum::extract::{Path, Query};
use axum::response::IntoResponse;
use axum::Extension;
use calimero_context_client::group::ListNamespacesForApplicationRequest;
use calimero_primitives::application::ApplicationId;
use calimero_server_primitives::admin::{
    ListNamespacesApiResponse, ListNamespacesForApplicationQuery, NamespaceApiResponse,
};
use reqwest::StatusCode;
use tracing::{error, info};

use crate::admin::service::{ApiError, ApiResponse};
use crate::AdminState;

pub async fn handler(
    Path(app_id_str): Path<String>,
    Query(query): Query<ListNamespacesForApplicationQuery>,
    Extension(state): Extension<Arc<AdminState>>,
) -> impl IntoResponse {
    let application_id: ApplicationId = match app_id_str.parse() {
        Ok(id) => id,
        Err(_) => {
            return ApiError {
                status_code: StatusCode::BAD_REQUEST,
                message: "Invalid application id".to_owned(),
            }
            .into_response()
        }
    };

    let offset = query.offset.unwrap_or(0);
    let limit = query.limit.unwrap_or(100);

    info!(application_id=%app_id_str, %offset, %limit, "Listing namespaces for application");

    let result = state
        .ctx_client
        .list_namespaces_for_application(ListNamespacesForApplicationRequest {
            application_id,
            offset,
            limit,
        })
        .await;

    match result {
        Ok(entries) => {
            let data = entries
                .into_iter()
                .map(|ns| NamespaceApiResponse {
                    namespace_id: hex::encode(ns.namespace_id.to_bytes()),
                    app_key: hex::encode(ns.app_key.to_bytes()),
                    target_application_id: ns.target_application_id.to_string(),
                    upgrade_policy: format!("{:?}", ns.upgrade_policy),
                    created_at: ns.created_at,
                    alias: ns.alias,
                    member_count: ns.member_count,
                    context_count: ns.context_count,
                    subgroup_count: ns.subgroup_count,
                })
                .collect();
            ApiResponse {
                payload: ListNamespacesApiResponse { data },
            }
            .into_response()
        }
        Err(err) => {
            error!(error=?err, "Failed to list namespaces for application");
            ApiError {
                status_code: StatusCode::INTERNAL_SERVER_ERROR,
                message: "Failed to list namespaces".to_owned(),
            }
            .into_response()
        }
    }
}
