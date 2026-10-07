use std::sync::Arc;

use axum::response::IntoResponse;
use axum::Extension;
use calimero_context_client::group::CreateGroupRequest;
use calimero_context_config::types::BytecodeId;
use calimero_server_primitives::admin::{
    CreateGroupApiRequest, CreateGroupApiResponse, CreateGroupApiResponseData,
};
use reqwest::StatusCode;
use tracing::{error, info};

use crate::admin::handlers::validation::ValidatedJson;
use crate::admin::service::{parse_api_error, ApiError, ApiResponse};
use crate::AdminState;

use super::{parse_group_id, restricted_from};

pub async fn handler(
    Extension(state): Extension<Arc<AdminState>>,
    ValidatedJson(req): ValidatedJson<CreateGroupApiRequest>,
) -> impl IntoResponse {
    let request = match create_request(req) {
        Ok(request) => request,
        Err(err) => return err.into_response(),
    };

    info!(
        application_id=?request.application_id,
        parent_group_id=?request.parent_group_id,
        "Creating group"
    );

    let result = state
        .ctx_client
        .create_group(request)
        .await
        .map_err(parse_api_error);

    match result {
        Ok(response) => {
            let group_id_hex = hex::encode(response.group_id.to_bytes());
            info!(group_id=%group_id_hex, "Group created successfully");
            ApiResponse {
                payload: CreateGroupApiResponse {
                    data: CreateGroupApiResponseData {
                        group_id: group_id_hex,
                    },
                },
            }
            .into_response()
        }
        Err(err) => {
            error!(error=?err, "Failed to create group");
            err.into_response()
        }
    }
}

fn create_request(req: CreateGroupApiRequest) -> Result<CreateGroupRequest, ApiError> {
    let bytecode_id = match &req.bytecode_id {
        Some(hex_str) => {
            let bytes: [u8; 32] = hex::decode(hex_str)
                .map_err(|_| ())
                .and_then(|v| v.try_into().map_err(|_| ()))
                .map_err(|()| ApiError {
                    status_code: StatusCode::BAD_REQUEST,
                    message: "Invalid appKey: expected hex-encoded 32 bytes".into(),
                })?;
            Some(BytecodeId::from(bytes))
        }
        None => None,
    };

    let parent_group_id = req
        .parent_group_id
        .as_deref()
        .map(parse_group_id)
        .transpose()?;

    Ok(CreateGroupRequest {
        salt: None,
        bytecode_id,
        application_id: Some(req.application_id),
        name: req.name,
        parent_group_id,
        restricted,
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn nested_create(visibility: Option<&str>) -> Result<CreateGroupRequest, ApiError> {
        let mut body = json!({
            "applicationId": "0".repeat(64),
            "parentGroupId": "07".repeat(32),
        });
        if let Some(visibility) = visibility {
            body["visibility"] = json!(visibility);
        }
        create_request(serde_json::from_value(body).expect("a well-formed body"))
    }

    #[test]
    fn an_absent_visibility_creates_the_nested_group_open() {
        let request = nested_create(None).expect("accepted");
        assert!(!request.restricted);
    }

    #[test]
    fn a_restricted_visibility_creates_the_nested_group_restricted() {
        let request = nested_create(Some("restricted")).expect("accepted");
        assert!(request.restricted);
    }

    #[test]
    fn an_unknown_visibility_is_refused_with_a_400() {
        let err = nested_create(Some("public")).expect_err("must be refused");
        assert_eq!(err.status_code, StatusCode::BAD_REQUEST);
        assert!(
            err.message.contains("invalid visibility 'public'"),
            "got: {}",
            err.message
        );
    }
}
