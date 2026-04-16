use std::sync::Arc;

use axum::extract::Path;
use axum::response::IntoResponse;
use axum::Extension;
use calimero_server_primitives::admin::{ListSubgroupsApiResponse, SubgroupEntryApiResponse};
use tracing::info;

use crate::admin::handlers::groups::parse_group_id;
use crate::admin::service::{parse_api_error, ApiResponse};
use crate::AdminState;

pub async fn handler(
    Path(group_id_str): Path<String>,
    Extension(state): Extension<Arc<AdminState>>,
) -> impl IntoResponse {
    let group_id = match parse_group_id(&group_id_str) {
        Ok(id) => id,
        Err(err) => return err.into_response(),
    };

    info!(group_id=%group_id_str, "Listing subgroups");

    let children = match calimero_context::group_store::list_child_groups(&state.store, &group_id) {
        Ok(children) => children,
        Err(err) => return parse_api_error(err).into_response(),
    };

    let mut subgroups = Vec::with_capacity(children.len());
    for child in children {
        let alias = match calimero_context::group_store::get_group_alias(&state.store, &child) {
            Ok(alias) => alias,
            Err(err) => return parse_api_error(err).into_response(),
        };
        subgroups.push(SubgroupEntryApiResponse {
            group_id: hex::encode(child.to_bytes()),
            alias,
        });
    }

    ApiResponse {
        payload: ListSubgroupsApiResponse { subgroups },
    }
    .into_response()
}
