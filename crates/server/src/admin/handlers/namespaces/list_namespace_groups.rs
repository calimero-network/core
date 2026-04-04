use std::sync::Arc;

use axum::extract::Path;
use axum::response::IntoResponse;
use axum::Extension;
use calimero_server_primitives::admin::{
    ListNamespaceGroupsApiResponse, NamespaceGroupEntryApiResponse,
};
use tracing::error;

use crate::admin::handlers::groups::parse_group_id;
use crate::admin::service::{parse_api_error, ApiResponse};
use crate::AdminState;

pub async fn handler(
    Path(namespace_id_str): Path<String>,
    Extension(state): Extension<Arc<AdminState>>,
) -> impl IntoResponse {
    let namespace_id = match parse_group_id(&namespace_id_str) {
        Ok(id) => id,
        Err(err) => return err.into_response(),
    };

    let groups = match calimero_context::group_store::list_child_groups(&state.store, &namespace_id)
    {
        Ok(groups) => groups,
        Err(err) => return parse_api_error(err).into_response(),
    };

    let mut entries = Vec::with_capacity(groups.len());
    for group_id in groups {
        let alias = match calimero_context::group_store::get_group_alias(&state.store, &group_id) {
            Ok(alias) => alias,
            Err(err) => {
                error!(
                    ?err,
                    "Failed to resolve group alias while listing namespace groups"
                );
                return parse_api_error(err).into_response();
            }
        };
        entries.push(NamespaceGroupEntryApiResponse {
            group_id: hex::encode(group_id.to_bytes()),
            alias,
        });
    }

    ApiResponse {
        payload: ListNamespaceGroupsApiResponse { data: entries },
    }
    .into_response()
}
