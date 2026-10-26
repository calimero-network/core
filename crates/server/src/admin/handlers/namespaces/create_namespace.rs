use std::sync::Arc;

use axum::response::IntoResponse;
use axum::Extension;
use calimero_context_client::group::CreateGroupRequest;
use calimero_context_config::types::BytecodeId;
use calimero_server_primitives::admin::{
    CreateNamespaceApiRequest, CreateNamespaceApiResponse, CreateNamespaceApiResponseData,
};
use reqwest::StatusCode;
use tracing::{error, info};

use crate::admin::handlers::validation::ValidatedJson;
use crate::admin::service::{parse_api_error, ApiError, ApiResponse};
use crate::AdminState;

pub async fn handler(
    Extension(state): Extension<Arc<AdminState>>,
    client_key: Option<Extension<crate::auth::ClientKeyScope>>,
    ValidatedJson(req): ValidatedJson<CreateNamespaceApiRequest>,
) -> impl IntoResponse {
    if let Some(refusal) = crate::admin::client_key_scope::refuse_target_outside_binding(
        &state,
        client_key.as_deref(),
        None,
        Some(&req.application_id),
    ) {
        return refusal;
    }

    // Optional version pin: hex blob id of an installed version. Existence
    // and package-match are verified by the create-group handler.
    let bytecode_id = match &req.bytecode_id {
        Some(hex_str) => {
            let bytes: [u8; 32] = match hex::decode(hex_str)
                .map_err(|_| ())
                .and_then(|v| v.try_into().map_err(|_| ()))
            {
                Ok(b) => b,
                Err(()) => {
                    return ApiError {
                        status_code: StatusCode::BAD_REQUEST,
                        message: "Invalid appKey: expected hex-encoded 32 bytes".into(),
                    }
                    .into_response();
                }
            };
            Some(BytecodeId::from(bytes))
        }
        None => None,
    };

    info!(application_id=%req.application_id, has_bytecode_id=bytecode_id.is_some(), "Creating namespace");

    let result = state
        .ctx_client
        .create_group(CreateGroupRequest {
            parent_group_id: None,
            salt: None,
            bytecode_id,
            application_id: Some(req.application_id),
            name: req.name,
            // Root creation has no `GroupCreated` op; `restricted` is ignored
            // here (kept Restricted for struct completeness). (#2771)
            restricted: true,
        })
        .await
        .map_err(parse_api_error);

    let response = match result {
        Ok(response) => response,
        Err(err) => {
            error!(error=?err, "Failed to create namespace");
            return err.into_response();
        }
    };
    // The namespace exists before it can approve anything, so this runs after.
    // A refusal is reported, naming the namespace, rather than undoing it.
    let outbound_approval = match req.approve_outbound {
        None => None,
        Some(selection) => {
            match super::outbound_approvals::approve(
                &state,
                response.group_id,
                req.application_id,
                selection,
                false,
            )
            .await
            {
                Ok(approved) => Some(super::outbound_approvals::api_data(&approved)),
                Err(err) => {
                    error!(error=?err, "Namespace created, but approving outbound requests failed");
                    return ApiError {
                        status_code: err.status_code,
                        message: format!(
                            "namespace {} was created, but approving outbound requests failed: {}",
                            hex::encode(response.group_id.to_bytes()),
                            err.message
                        ),
                    }
                    .into_response();
                }
            }
        }
    };
    ApiResponse {
        payload: CreateNamespaceApiResponse {
            data: CreateNamespaceApiResponseData {
                namespace_id: hex::encode(response.group_id.to_bytes()),
                founding: response
                    .founding
                    .map(|f| super::founding_api(&f.founder, &f.salt)),
                outbound_approval,
            },
        },
    }
    .into_response()
}
