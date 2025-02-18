use std::sync::Arc;

use axum::extract::Path;
use axum::response::IntoResponse;
use axum::{Extension, Json};
use calimero_context_config::repr::{Repr, ReprTransmute};
use calimero_context_config::types::ProposalId;
use calimero_primitives::context::ContextId;
use calimero_server_primitives::admin::{
    GetContextStorageEntriesRequest, GetContextStorageEntriesResponse, GetContextValueRequest,
    GetContextValueResponse, GetNumberOfActiveProposalsResponse,
    GetNumberOfProposalApprovalsResponse, GetProposalApproversResponse, GetProposalResponse,
    GetProposalsRequest, GetProposalsResponse, GetProxyContractResponse,
};
use serde::{Deserialize, Serialize};

use crate::admin::service::{parse_api_error, ApiResponse};
use crate::AdminState;

//todo split it up into separate files

#[derive(Debug, Deserialize, Serialize, Copy, Clone)]
#[serde(rename_all = "camelCase")]
pub enum ActionType {
    ExternalFunctionCall,
    Transfer,
    SetNumApprovals,
    SetActiveProposalsLimit,
    SetContextValue,
}

pub async fn get_proposals_handler(
    Path(context_id): Path<ContextId>,
    Extension(state): Extension<Arc<AdminState>>,
    Json(req): Json<GetProposalsRequest>,
) -> impl IntoResponse {
    match state
        .ctx_manager
        .get_proposals(context_id, req.offset, req.limit)
        .await
    {
        Ok(context_proposals) => ApiResponse {
            payload: GetProposalsResponse {
                data: context_proposals,
            },
        }
        .into_response(),
        Err(err) => parse_api_error(err).into_response(),
    }
}

pub async fn get_proposal_handler(
    Path((context_id, proposal_id)): Path<(ContextId, Repr<ProposalId>)>,
    Extension(state): Extension<Arc<AdminState>>,
) -> impl IntoResponse {
    match state
        .ctx_manager
        .get_proposal(context_id, proposal_id.rt().expect("infallible conversion"))
        .await
    {
        Ok(context_proposal) => ApiResponse {
            payload: GetProposalResponse {
                data: context_proposal,
            },
        }
        .into_response(),
        Err(err) => parse_api_error(err).into_response(),
    }
}

pub async fn get_proxy_contract_handler(
    Path(context_id): Path<ContextId>,
    Extension(state): Extension<Arc<AdminState>>,
) -> impl IntoResponse {
    match state.ctx_manager.get_proxy_id(context_id).await {
        Ok(proxy_contract) => ApiResponse {
            payload: GetProxyContractResponse {
                data: proxy_contract,
            },
        }
        .into_response(),
        Err(err) => parse_api_error(err).into_response(),
    }
}

pub async fn get_context_value_handler(
    Path(context_id): Path<ContextId>,
    Extension(state): Extension<Arc<AdminState>>,
    Json(req): Json<GetContextValueRequest>,
) -> impl IntoResponse {
    match state
        .ctx_manager
        .get_context_value(context_id, req.key.as_bytes().to_vec())
        .await
    {
        Ok(context_value) => ApiResponse {
            payload: GetContextValueResponse {
                data: context_value,
            },
        }
        .into_response(),
        Err(err) => parse_api_error(err).into_response(),
    }
}

pub async fn get_context_storage_entries_handler(
    Path(context_id): Path<ContextId>,
    Extension(state): Extension<Arc<AdminState>>,
    Json(req): Json<GetContextStorageEntriesRequest>,
) -> impl IntoResponse {
    match state
        .ctx_manager
        .get_context_storage_entries(context_id, req.offset, req.limit)
        .await
    {
        Ok(context_storage_entries) => ApiResponse {
            payload: GetContextStorageEntriesResponse {
                data: context_storage_entries,
            },
        }
        .into_response(),
        Err(err) => parse_api_error(err).into_response(),
    }
}

// TODO - proxy missing function to fetch number of all
pub async fn get_number_of_active_proposals_handler(
    Path(context_id): Path<ContextId>,
    Extension(state): Extension<Arc<AdminState>>,
) -> impl IntoResponse {
    match state
        .ctx_manager
        .get_number_of_active_proposals(context_id)
        .await
    {
        Ok(active_proposals_number) => ApiResponse {
            payload: GetNumberOfActiveProposalsResponse {
                data: active_proposals_number,
            },
        }
        .into_response(),
        Err(err) => parse_api_error(err).into_response(),
    }
}

pub async fn get_number_of_proposal_approvals_handler(
    Path((context_id, proposal_id)): Path<(ContextId, Repr<ProposalId>)>,
    Extension(state): Extension<Arc<AdminState>>,
) -> impl IntoResponse {
    match state
        .ctx_manager
        .get_number_of_proposal_approvals(
            context_id,
            proposal_id.rt().expect("infallible conversion"),
        )
        .await
    {
        Ok(number_of_proposal_approvals) => ApiResponse {
            payload: GetNumberOfProposalApprovalsResponse {
                data: number_of_proposal_approvals,
            },
        }
        .into_response(),
        Err(err) => parse_api_error(err).into_response(),
    }
}

// return list of users who approved
pub async fn get_proposal_approvers_handler(
    Path((context_id, proposal_id)): Path<(ContextId, Repr<ProposalId>)>,
    Extension(state): Extension<Arc<AdminState>>,
) -> impl IntoResponse {
    match state
        .ctx_manager
        .get_proposal_approvers(context_id, proposal_id.rt().expect("infallible conversion"))
        .await
    {
        Ok(proposal_approvers) => ApiResponse {
            payload: GetProposalApproversResponse {
                data: proposal_approvers.into_iter().map(Repr::new).collect(),
            },
        }
        .into_response(),
        Err(err) => parse_api_error(err).into_response(),
    }
}
