use std::sync::Arc;

use axum::extract::Path;
use axum::response::IntoResponse;
use axum::{Extension, Json};
use calimero_context_config::repr::{Repr, ReprTransmute};
use calimero_context_config::types::{ContextIdentity, ProposalId};
use calimero_context_config::{Proposal as ProposalConfig, ProposalWithApprovals};
use calimero_primitives::context::ContextId;
use serde::{Deserialize, Serialize};
use serde_json::Value;

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

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Action {
    ExternalFunctionCall(ExternalFunctionCall),
    Transfer(Transfer),
    SetNumApprovals(SetNumApprovals),
    SetActiveProposalsLimit(SetActiveProposalsLimit),
    SetContextValue(SetContextValue),
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExternalFunctionCall {
    pub(crate) receiver_id: Repr<ContextIdentity>,
    pub(crate) method_name: String,
    pub(crate) args: Value,
    pub(crate) deposit: String,
    pub(crate) gas: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Transfer {
    pub(crate) amount: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SetNumApprovals {
    pub(crate) num_of_approvals: u32,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
pub struct SetActiveProposalsLimit {
    pub(crate) active_proposals_limit: u32,
}

// Use generics to allow any type for `value` in `SetContextValue`
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SetContextValue {
    pub(crate) key: String,
    pub(crate) value: Value,
}

// Define Proposal struct
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Proposal {
    pub id: String,
    pub author: Repr<ContextIdentity>,
    pub(crate) actions: Vec<Action>,
    pub title: String,
    pub description: String,
    pub(crate) created_at: String,
}

// Define Members struct
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Members {
    pub public_key: String,
}

// Define Message struct
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Message {
    pub public_key: String,
}

//ENDPOINTS

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GetProposalsResponse {
    pub data: Vec<ProposalConfig>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GetProposalResponse {
    pub data: ProposalConfig,
}

#[derive(Copy, Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GetProposalsRequest {
    pub offset: usize,
    pub limit: usize,
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

#[derive(Debug, Copy, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GetNumberOfActiveProposalsResponse {
    pub data: u16,
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

#[derive(Debug, Copy, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GetNumberOfProposalApprovalsResponse {
    pub data: ProposalWithApprovals,
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

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GetProposalApproversResponse {
    pub data: Vec<Repr<ContextIdentity>>,
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
