use std::sync::Arc;

use axum::extract::{Path, Query};
use axum::response::IntoResponse;
use axum::Extension;
use calimero_context_client::group::ListGroupDevicesRequest;
use calimero_server_primitives::admin::{
    GroupDeviceApiEntry, ListGroupDevicesApiResponse, ListGroupDevicesQuery,
};
use tracing::{error, info};

use super::{parse_group_id, DEFAULT_LIST_LIMIT, MAX_LIST_LIMIT};
use crate::admin::service::{parse_api_error, ApiResponse};
use crate::AdminState;

pub async fn handler(
    Path(group_id_str): Path<String>,
    Query(query): Query<ListGroupDevicesQuery>,
    Extension(state): Extension<Arc<AdminState>>,
) -> impl IntoResponse {
    let group_id = match parse_group_id(&group_id_str) {
        Ok(id) => id,
        Err(err) => return err.into_response(),
    };

    let offset = query.offset.unwrap_or(0);
    let limit = query
        .limit
        .unwrap_or(DEFAULT_LIST_LIMIT)
        .min(MAX_LIST_LIMIT);

    info!(group_id=%group_id_str, %offset, %limit, "Listing group devices");

    let result = state
        .ctx_client
        .list_group_devices(ListGroupDevicesRequest {
            group_id,
            member: query.member,
            offset,
            limit,
        })
        .await
        .map_err(parse_api_error);

    match result {
        Ok(resp) => {
            info!(group_id=%group_id_str, count=%resp.devices.len(), "Group devices retrieved successfully");
            let devices = resp
                .devices
                .into_iter()
                .map(|d| GroupDeviceApiEntry {
                    device: d.device,
                    account: d.account,
                    signing_key: d.signing_key,
                    device_epoch: d.device_epoch,
                })
                .collect();
            ApiResponse {
                payload: ListGroupDevicesApiResponse { devices },
            }
            .into_response()
        }
        Err(err) => {
            error!(group_id=%group_id_str, error=?err, "Failed to list group devices");
            err.into_response()
        }
    }
}
