use std::sync::Arc;

use axum::extract::Path;
use axum::response::IntoResponse;
use axum::Extension;
use calimero_account::DeviceId;
use calimero_context_client::group::{BindOutcome, RelinkDeviceRequest};
use calimero_primitives::application::ApplicationId;
use calimero_server_primitives::admin::{
    RelinkDeviceApiRequest, RelinkDeviceApiResponse, RelinkDeviceApiResponseData,
    RelinkOutcomeApiEntry, RelinkSkipApiEntry,
};
use reqwest::StatusCode;
use tracing::info;

use crate::admin::handlers::account::decode32;
use crate::admin::handlers::validation::ValidatedJson;
use crate::admin::service::{parse_api_error, ApiError, ApiResponse};
use crate::AdminState;

/// Repair or widen the reach of a device this account already certified.
///
/// Run on the node that holds the account - it is the only one with the stored
/// certificate, and the only one whose root signed it. The device is not
/// consulted and need not be online.
pub async fn handler(
    Path(device_id_str): Path<String>,
    Extension(state): Extension<Arc<AdminState>>,
    ValidatedJson(req): ValidatedJson<RelinkDeviceApiRequest>,
) -> impl IntoResponse {
    let device = match decode32(&device_id_str, "deviceId") {
        Ok(bytes) => DeviceId::from(bytes),
        Err(err) => return err.into_response(),
    };

    let mut applications = Vec::with_capacity(req.applications.len());
    for application_id in &req.applications {
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

    info!(
        device = %device_id_str,
        extending_by = applications.len(),
        "relinking a device of this account"
    );

    let result = state
        .ctx_client
        .relink_device(RelinkDeviceRequest {
            device,
            applications,
        })
        .await
        .map_err(parse_api_error);

    match result {
        Ok(resp) => {
            let mut linked_in = Vec::new();
            let mut skipped = Vec::new();
            for outcome in &resp.outcomes {
                let namespace_id = hex::encode(outcome.namespace_id.to_bytes());
                // The wire names are produced here and the match is exhaustive on
                // purpose: a new outcome has to be given a name rather than fall
                // into a catch-all and be reported as something it is not.
                let reason = match outcome.outcome {
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
                linked = linked_in.len(),
                skipped = skipped.len(),
                "device relinked"
            );
            ApiResponse {
                payload: RelinkDeviceApiResponse {
                    data: RelinkDeviceApiResponseData {
                        account_id: hex::encode(resp.account.as_bytes()),
                        device_id: hex::encode(resp.device.as_bytes()),
                        applications: resp.applications.iter().map(ToString::to_string).collect(),
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
