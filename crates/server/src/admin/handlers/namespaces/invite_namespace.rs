use calimero_governance_store::{MembershipRepository, MetadataRepository, NamespaceRepository};
use std::sync::Arc;

use axum::extract::Path;
use axum::response::IntoResponse;
use axum::Extension;
use calimero_context_client::group::CreateGroupInvitationRequest;
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
use crate::AdminState;

pub async fn handler(
    Path(namespace_id_str): Path<String>,
    Extension(state): Extension<Arc<AdminState>>,
    ValidatedJson(req): ValidatedJson<CreateGroupInvitationApiRequest>,
) -> impl IntoResponse {
    let namespace_id = match parse_group_id(&namespace_id_str) {
        Ok(id) => id,
        Err(err) => return err.into_response(),
    };

    match NamespaceRepository::new(&state.store).parent(&namespace_id) {
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

    // Clamped, not merely defaulted — see MAX_INVITATION_VALIDITY_SECS. An
    // invitation is redeemable by whoever holds it, so its lifetime is a
    // security parameter rather than a caller preference.
    let expiration_secs = req
        .expiration_timestamp
        .unwrap_or(calimero_context_config::types::MAX_INVITATION_VALIDITY_SECS)
        .min(calimero_context_config::types::MAX_INVITATION_VALIDITY_SECS);

    // Parsed before anything is signed: an admitter the caller cannot spell is
    // a restriction that would silently not apply, and an invitation restricted
    // to nobody reachable is worse than an unrestricted one.
    let admitters = match req
        .admitters
        .iter()
        .map(|a| a.parse::<calimero_account::AccountId>())
        .collect::<Result<Vec<_>, _>>()
    {
        Ok(list) => list,
        Err(_) => {
            return ApiError {
                status_code: StatusCode::BAD_REQUEST,
                message: "admitters must each be 64 hex characters (32 bytes)".to_owned(),
            }
            .into_response();
        }
    };

    if req.recursive.unwrap_or(false) {
        // The node signs as itself, with the one key it holds.
        let (node_pk, signing_key) =
            match NamespaceRepository::new(&state.store).resolve_identity(&namespace_id) {
                Ok(Some(pair)) => pair,
                Ok(None) => {
                    return ApiError {
                        status_code: StatusCode::BAD_REQUEST,
                        message: "this node has no signing identity for this namespace".into(),
                    }
                    .into_response();
                }
                Err(err) => return parse_api_error(err).into_response(),
            };
        let signer_account = match calimero_governance_store::member_account_in_namespace(
            &state.store,
            &namespace_id,
            &node_pk,
        ) {
            Ok(Some(account)) => account,
            Ok(None) => {
                // 403, not the 500 `parse_api_error` gives an untyped report.
                // A caller who never joined asking to invite is a permission
                // answer, and dressing it as an internal error both misleads the
                // caller and hides real backend faults among identical 500s.
                return ApiError {
                    status_code: StatusCode::FORBIDDEN,
                    message: "The requesting identity is bound to no account in this namespace"
                        .into(),
                }
                .into_response();
            }
            Err(err) => return parse_api_error(err).into_response(),
        };
        if let Err(err) = MembershipRepository::new(&state.store).require_admin_or_capability(
            &namespace_id,
            &signer_account,
            calimero_context_config::MemberCapabilities::CAN_INVITE_MEMBERS.bits(),
            "create namespace invitation",
        ) {
            return parse_api_error(err).into_response();
        }

        let inviter_sk = PrivateKey::from(signing_key);
        let invitations = match NamespaceRepository::new(&state.store).create_recursive_invitations(
            &namespace_id,
            &inviter_sk,
            expiration_secs,
            1,
            &admitters,
        ) {
            Ok(entries) => entries,
            Err(err) => return parse_api_error(err).into_response(),
        };

        let mut data = Vec::with_capacity(invitations.len());
        for (group_id, invitation) in invitations {
            let group_name = match MetadataRepository::new(&state.store).group_metadata(&group_id) {
                Ok(rec) => rec.and_then(|r| r.name),
                Err(err) => return parse_api_error(err).into_response(),
            };
            data.push(RecursiveInvitationEntry {
                group_id: hex::encode(group_id.to_bytes()),
                invitation,
                group_name,
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
            expiration_timestamp: req.expiration_timestamp,
            admitters,
            admitter_hints: req.admitter_hints,
        })
        .await
        .map_err(parse_api_error);

    match result {
        Ok(resp) => ApiResponse {
            payload: CreateGroupInvitationApiResponse {
                data: CreateGroupInvitationApiResponseData {
                    invitation: resp.invitation,
                    group_name: resp.group_name,
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
