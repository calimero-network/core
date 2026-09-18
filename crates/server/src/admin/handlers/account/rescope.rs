use std::sync::Arc;

use axum::extract::Path;
use axum::response::IntoResponse;
use axum::Extension;
use calimero_account::DeviceId;
use calimero_context_client::group::{BindOutcome, RescopeDeviceRequest, ScopeRequest};
use calimero_primitives::application::ApplicationId;
use calimero_server_primitives::admin::{
    DeviceScopeApiRequest, RelinkOutcomeApiEntry, RelinkSkipApiEntry, RescopeDescopeApiEntry,
    RescopeDeviceApiRequest, RescopeDeviceApiResponse, RescopeDeviceApiResponseData,
};
use reqwest::StatusCode;
use tracing::info;

use crate::admin::handlers::account::decode32;
use crate::admin::handlers::validation::ValidatedJson;
use crate::admin::service::{parse_api_error, ApiError, ApiResponse};
use crate::AdminState;

/// Replace the scope of a device this account already certified.
///
/// Run on the node that holds the account root - it is the only one whose key can
/// sign the replacement. The device is not consulted and need not be online.
pub async fn handler(
    Path(device_id_str): Path<String>,
    Extension(state): Extension<Arc<AdminState>>,
    ValidatedJson(req): ValidatedJson<RescopeDeviceApiRequest>,
) -> impl IntoResponse {
    let device = match decode32(&device_id_str, "deviceId") {
        Ok(bytes) => DeviceId::from(bytes),
        Err(err) => return err.into_response(),
    };

    let scope = match req.scope {
        DeviceScopeApiRequest::All => ScopeRequest::All,
        DeviceScopeApiRequest::Only(named) => {
            let mut applications = Vec::with_capacity(named.len());
            for application_id in &named {
                match application_id.parse::<ApplicationId>() {
                    Ok(id) => applications.push(id),
                    Err(_) => {
                        return ApiError {
                            status_code: StatusCode::BAD_REQUEST,
                            message: format!("Invalid application id: {application_id}"),
                        }
                        .into_response()
                    }
                }
            }
            ScopeRequest::Only(applications)
        }
    };

    info!(device = %device_id_str, "replacing a device's scope");

    let result = state
        .ctx_client
        .rescope_device(RescopeDeviceRequest { device, scope })
        .await
        .map_err(parse_api_error);

    match result {
        Ok(resp) => {
            let mut descoped = Vec::new();
            let mut linked_in = Vec::new();
            let mut skipped = Vec::new();
            for (namespace, outcome) in &resp.outcomes {
                let namespace_id = hex::encode(namespace.to_bytes());
                // Exhaustive on purpose: a new outcome gets a wire name rather
                // than falling into a catch-all and being misreported.
                let reason = match *outcome {
                    BindOutcome::Descoped { key_rotated } => {
                        descoped.push(RescopeDescopeApiEntry {
                            namespace_id,
                            key_rotated,
                        });
                        continue;
                    }
                    BindOutcome::Linked { key_delivered } => {
                        linked_in.push(RelinkOutcomeApiEntry {
                            namespace_id,
                            key_delivered,
                        });
                        continue;
                    }
                    BindOutcome::OutOfScope => "outOfScope",
                    BindOutcome::AlreadyBound => "alreadyBound",
                    BindOutcome::NoScopeKey => "noScopeKey",
                    BindOutcome::Revoked => "revoked",
                    BindOutcome::OwnDevice => "ownDevice",
                    BindOutcome::Failed => "failed",
                };
                skipped.push(RelinkSkipApiEntry {
                    namespace_id,
                    reason: reason.to_owned(),
                });
            }

            info!(
                account = %resp.account,
                device = %resp.device,
                applications = resp.applications.len(),
                "device rescoped"
            );
            ApiResponse {
                payload: RescopeDeviceApiResponse {
                    data: RescopeDeviceApiResponseData {
                        account_id: hex::encode(resp.account.as_bytes()),
                        device_id: hex::encode(resp.device.as_bytes()),
                        applications: resp.applications.iter().map(ToString::to_string).collect(),
                        descoped,
                        linked_in,
                        skipped,
                    },
                },
            }
            .into_response()
        }
        Err(err) => err.into_response(),
    }
}
