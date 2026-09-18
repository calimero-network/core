use std::sync::Arc;

use axum::extract::Path;
use axum::response::IntoResponse;
use axum::Extension;
use calimero_account::DeviceId;
use calimero_context_client::group::LabelDeviceRequest;
use calimero_server_primitives::admin::{
    LabelDeviceApiRequest, LabelDeviceApiResponse, LabelDeviceApiResponseData,
};
use tracing::info;

use crate::admin::handlers::account::decode32;
use crate::admin::handlers::validation::ValidatedJson;
use crate::admin::service::{parse_api_error, ApiResponse};
use crate::AdminState;

/// Name a device of this node's own account.
pub async fn handler(
    Path(device_id_str): Path<String>,
    Extension(state): Extension<Arc<AdminState>>,
    ValidatedJson(req): ValidatedJson<LabelDeviceApiRequest>,
) -> impl IntoResponse {
    let device = match decode32(&device_id_str, "deviceId") {
        Ok(bytes) => DeviceId::from(bytes),
        Err(err) => return err.into_response(),
    };

    let result = state
        .ctx_client
        .label_device(LabelDeviceRequest {
            device,
            label: req.label,
        })
        .await
        .map_err(parse_api_error);

    match result {
        Ok(resp) => {
            info!(device = %resp.device, label_epoch = resp.label_epoch, "device named");
            ApiResponse {
                payload: LabelDeviceApiResponse {
                    data: LabelDeviceApiResponseData {
                        account_id: hex::encode(resp.account.as_bytes()),
                        device_id: hex::encode(resp.device.as_bytes()),
                        label: resp.label,
                        label_epoch: resp.label_epoch,
                    },
                },
            }
            .into_response()
        }
        Err(err) => err.into_response(),
    }
}
