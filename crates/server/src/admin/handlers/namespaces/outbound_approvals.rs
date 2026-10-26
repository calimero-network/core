//! A namespace's outbound approvals: which URL prefixes it lets each
//! publisher's apps send outbound requests under (`GroupOp::OutboundApprovalSet`).
//!
//! `GET` lists them for anyone who may read the namespace; `PUT` sets one
//! publisher's set exactly, which is how a prefix is revoked. An install into a
//! namespace and an upgrade approve through [`approve`], in the same call.

use std::sync::Arc;

use axum::extract::Path;
use axum::response::IntoResponse;
use axum::Extension;
use calimero_context_client::group::{ApproveOutboundRequest, ApproveOutboundResponse};
use calimero_context_config::types::ContextGroupId;
use calimero_governance_store::{NamespaceRepository, OutboundApprovalRepository};
use calimero_primitives::application::ApplicationId;
use calimero_primitives::outbound::OutboundSelection;
use calimero_server_primitives::admin::{
    ListOutboundApprovalsApiResponse, OutboundApprovalApiData, SetOutboundApprovalApiRequest,
    SetOutboundApprovalApiResponse, SetOutboundApprovalApiResponseData,
};
use tracing::{debug, error, info};

use crate::admin::handlers::groups::parse_group_id;
use crate::admin::handlers::validation::ValidatedJson;
use crate::admin::service::{parse_api_error, ApiError, ApiResponse};
use crate::AdminState;

/// Approve `selection` for the publisher of `application_id` in the namespace
/// `group_id` belongs to: added to what it approves, or (`replace`) instead of it.
pub(crate) async fn approve(
    state: &AdminState,
    group_id: ContextGroupId,
    application_id: ApplicationId,
    selection: OutboundSelection,
    replace: bool,
) -> Result<ApproveOutboundResponse, ApiError> {
    state
        .ctx_client
        .approve_outbound(ApproveOutboundRequest {
            group_id,
            application_id,
            selection,
            replace,
        })
        .await
        .map_err(parse_api_error)
}

pub(crate) fn api_data(response: &ApproveOutboundResponse) -> OutboundApprovalApiData {
    OutboundApprovalApiData {
        signer: response.signer.clone(),
        prefixes: response.prefixes.clone(),
        reviewed: response.reviewed.clone(),
    }
}

/// `GET /admin-api/namespaces/:namespace_id/outbound-approvals`
pub async fn list(
    Path(namespace_id_str): Path<String>,
    Extension(state): Extension<Arc<AdminState>>,
) -> impl IntoResponse {
    let namespace_id = match parse_group_id(&namespace_id_str) {
        Ok(id) => id,
        Err(err) => return err.into_response(),
    };
    let listed = NamespaceRepository::new(&state.store)
        .resolve(&namespace_id)
        .and_then(|root| OutboundApprovalRepository::new(&state.store).list(&root));
    match listed {
        Ok(approvals) => {
            debug!(namespace_id=%namespace_id_str, publishers=%approvals.len(), "Outbound approvals listed");
            ApiResponse {
                payload: ListOutboundApprovalsApiResponse {
                    data: approvals
                        .into_iter()
                        .map(|a| OutboundApprovalApiData {
                            signer: a.signer,
                            prefixes: a.prefixes,
                            reviewed: a.reviewed,
                        })
                        .collect(),
                },
            }
            .into_response()
        }
        Err(err) => {
            error!(namespace_id=%namespace_id_str, error=?err, "Failed to list outbound approvals");
            parse_api_error(err).into_response()
        }
    }
}

/// `PUT /admin-api/namespaces/:namespace_id/outbound-approvals`
pub async fn set(
    Path(namespace_id_str): Path<String>,
    Extension(state): Extension<Arc<AdminState>>,
    client_key: Option<Extension<crate::auth::ClientKeyScope>>,
    ValidatedJson(req): ValidatedJson<SetOutboundApprovalApiRequest>,
) -> impl IntoResponse {
    let namespace_id = match parse_group_id(&namespace_id_str) {
        Ok(id) => id,
        Err(err) => return err.into_response(),
    };
    if let Some(refusal) = crate::admin::client_key_scope::refuse_target_outside_binding(
        &state,
        client_key.as_deref(),
        Some(&namespace_id),
        Some(&req.application_id),
    ) {
        return refusal;
    }
    match approve(&state, namespace_id, req.application_id, req.approve, true).await {
        Ok(response) => {
            info!(namespace_id=%namespace_id_str, signer=%response.signer, published=response.published, "Outbound approval set");
            ApiResponse {
                payload: SetOutboundApprovalApiResponse {
                    data: SetOutboundApprovalApiResponseData {
                        namespace_id: hex::encode(response.namespace_id.to_bytes()),
                        approval: api_data(&response),
                        published: response.published,
                    },
                },
            }
            .into_response()
        }
        Err(err) => {
            error!(namespace_id=%namespace_id_str, error=?err, "Failed to set outbound approval");
            err.into_response()
        }
    }
}
