use std::sync::Arc;

use axum::extract::Path;
use axum::response::IntoResponse;
use axum::Extension;
use calimero_context_client::group::CreateGroupInvitationRequest;
use calimero_context_config::types::ContextGroupId;
use calimero_primitives::identity::PrivateKey;
use calimero_server_primitives::admin::{
    CreateGroupInvitationApiRequest, CreateGroupInvitationApiResponse,
    CreateGroupInvitationApiResponseData, CreateRecursiveInvitationApiResponse,
    CreateRecursiveInvitationApiResponseData, RecursiveInvitationEntry,
};
use reqwest::StatusCode;
use tracing::{error, info};

use crate::admin::handlers::groups::parse_group_id;
use crate::admin::handlers::validation::ValidatedJson;
use crate::admin::service::{parse_api_error, ApiError, ApiResponse};
use crate::auth::AuthenticatedKey;
use crate::AdminState;

pub async fn handler(
    Path(namespace_id_str): Path<String>,
    Extension(state): Extension<Arc<AdminState>>,
    auth_key: Option<Extension<AuthenticatedKey>>,
    ValidatedJson(req): ValidatedJson<CreateGroupInvitationApiRequest>,
) -> impl IntoResponse {
    let namespace_id = match parse_group_id(&namespace_id_str) {
        Ok(id) => id,
        Err(err) => return err.into_response(),
    };

    match calimero_context::group_store::get_parent_group(&state.store, &namespace_id) {
        Ok(Some(_)) => {
            return ApiError {
                status_code: StatusCode::BAD_REQUEST,
                message: "namespace_id must reference a root group (namespace)".into(),
            }
            .into_response();
        }
        Ok(None) => {}
        Err(err) => return parse_api_error(err).into_response(),
    }

    info!(namespace_id=%namespace_id_str, recursive=?req.recursive, "Creating namespace invitation");

    let requester = auth_key.map(|Extension(k)| k.0).or(req.requester);
    let expiration_secs = req.expiration_timestamp.unwrap_or(365 * 24 * 3600);

    if req.recursive.unwrap_or(false) {
        let requester = match requester {
            Some(pk) => pk,
            None => match calimero_context::group_store::resolve_namespace_identity(
                &state.store,
                &namespace_id,
            ) {
                Ok(Some((pk, _, _))) => pk,
                Ok(None) => {
                    return ApiError {
                        status_code: StatusCode::BAD_REQUEST,
                        message: "requester not provided and no namespace identity available".into(),
                    }
                    .into_response();
                }
                Err(err) => return parse_api_error(err).into_response(),
            },
        };

        if let Err(err) = calimero_context::group_store::require_group_admin_or_capability(
            &state.store,
            &namespace_id,
            &requester,
            calimero_context_config::MemberCapabilities::CAN_INVITE_MEMBERS,
            "create namespace invitation",
        ) {
            return parse_api_error(err).into_response();
        }

        let signing_key = match calimero_context::group_store::get_group_signing_key(
            &state.store,
            &namespace_id,
            &requester,
        ) {
            Ok(Some(sk)) => sk,
            Ok(None) => {
                return ApiError {
                    status_code: StatusCode::BAD_REQUEST,
                    message: "signing key not found for requester".into(),
                }
                .into_response();
            }
            Err(err) => return parse_api_error(err).into_response(),
        };

        let inviter_sk = PrivateKey::from(signing_key);
        let invitations = match calimero_context::group_store::create_recursive_invitations(
            &state.store,
            &namespace_id,
            &inviter_sk,
            expiration_secs,
            1,
        ) {
            Ok(entries) => entries,
            Err(err) => return parse_api_error(err).into_response(),
        };

        let mut data = Vec::with_capacity(invitations.len());
        for (group_id, invitation) in invitations {
            let group_alias = match calimero_context::group_store::get_group_alias(&state.store, &group_id)
            {
                Ok(alias) => alias,
                Err(err) => return parse_api_error(err).into_response(),
            };
            data.push(RecursiveInvitationEntry {
                group_id: hex::encode(group_id.to_bytes()),
                invitation,
                group_alias,
            });
        }

        return ApiResponse {
            payload: CreateRecursiveInvitationApiResponse {
                data: CreateRecursiveInvitationApiResponseData { invitations: data },
            },
        }
        .into_response();
    }

    let result = state
        .ctx_client
        .create_group_invitation(CreateGroupInvitationRequest {
            group_id: namespace_id,
            requester,
            expiration_timestamp: req.expiration_timestamp,
        })
        .await
        .map_err(parse_api_error);

    match result {
        Ok(resp) => ApiResponse {
            payload: CreateGroupInvitationApiResponse {
                data: CreateGroupInvitationApiResponseData {
                    invitation: resp.invitation,
                    group_alias: resp.group_alias,
                },
            },
        }
        .into_response(),
        Err(err) => {
            error!(namespace_id=%namespace_id_str, error=?err, "Failed to create namespace invitation");
            err.into_response()
        }
    }
}
