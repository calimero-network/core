use std::sync::Arc;
use std::vec;

use axum::extract::Path;
use axum::response::IntoResponse;
use axum::{Extension, Json};
use calimero_context_config::Proposal as ProposalConfig;
use calimero_primitives::context::ContextId;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tower_sessions::Session;

use crate::admin::service::ApiResponse;
use crate::AdminState;

//todo split it up into separate files

#[derive(Debug, Deserialize, Serialize)]
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
pub struct User {
    pub identity_public_key: String,
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
    pub(crate) receiver_id: User,
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

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SetNumApprovals {
    pub(crate) num_of_approvals: u32,
}

#[derive(Debug, Deserialize, Serialize)]
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
    pub author: User,
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
    pub offset: u32,
    pub length: u32,
}

pub async fn get_proposals_handler(
    Path(context_id): Path<String>,
    Extension(state): Extension<Arc<AdminState>>,
    Json(req): Json<GetProposalsRequest>,
) -> impl IntoResponse {
    let context_id: ContextId = context_id.parse().expect("Invalid context_id format");

    match state
        .ctx_manager
        .get_proposals(context_id, req.offset as usize, req.length as usize)
        .await
    {
        Ok(context_proposals) => ApiResponse {
            payload: GetProposalsResponse {
                data: context_proposals,
            },
        }
        .into_response(),
        Err(_) => "failed to fetch proposals".into_response(),
    }
}

pub async fn get_proposal_handler(
    Path((context_id, proposal_id)): Path<(String, String)>,
    session: Session,
    Extension(state): Extension<Arc<AdminState>>,
) -> impl IntoResponse {
    let context_id: ContextId = context_id.parse().expect("Invalid context_id format");

    match state
        .ctx_manager
        .get_proposal(context_id, proposal_id)
        .await
    {
        Ok(context_proposal) => ApiResponse {
            payload: GetProposalResponse {
                data: context_proposal,
            },
        }
        .into_response(),
        Err(_) => "failed to fetch proposal".into_response(),
    }
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GetNumberOfActiveProposalsResponse {
    pub data: u16,
}

pub async fn get_number_of_active_proposals_handler(
    Path(context_id): Path<String>,
    session: Session,
    Extension(state): Extension<Arc<AdminState>>,
) -> impl IntoResponse {
    ApiResponse {
        payload: GetNumberOfActiveProposalsResponse { data: 4 },
    }
    .into_response()
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GetNumberOfProposalApprovalsResponse {
    pub data: u16,
}

pub async fn get_number_of_proposal_approvals_handler(
    Path((context_id, proposal_id)): Path<(String, String)>,
    session: Session,
    Extension(state): Extension<Arc<AdminState>>,
) -> impl IntoResponse {
    ApiResponse {
        payload: GetNumberOfProposalApprovalsResponse { data: 5 },
    }
    .into_response()
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GetProposalApproversResponse {
    pub data: Vec<User>,
}

pub async fn get_proposal_approvers_handler(
    Path((context_id, proposal_id)): Path<(String, String)>,
    session: Session,
    Extension(state): Extension<Arc<AdminState>>,
    //Json(req): Json<GetProposalApproversResponse>,
) -> impl IntoResponse {
    ApiResponse {
        payload: GetProposalApproversResponse {
            data: vec![get_mock_user()],
        },
    }
    .into_response()
}

pub fn get_mock_user() -> User {
    User {
        identity_public_key: "sample_public_key".to_owned(),
    }
}

pub fn get_mock_actions() -> Vec<Action> {
    vec![
        Action::ExternalFunctionCall(ExternalFunctionCall {
            receiver_id: get_mock_user(),
            method_name: "sampleMethod".to_owned(),
            args: serde_json::json!({"example": "value"}),
            deposit: "100".to_owned(),
            gas: "5000".to_owned(),
        }),
        Action::Transfer(Transfer {
            amount: "250".to_owned(),
        }),
        Action::SetNumApprovals(SetNumApprovals {
            num_of_approvals: 3,
        }),
        Action::SetActiveProposalsLimit(SetActiveProposalsLimit {
            active_proposals_limit: 10,
        }),
        Action::SetContextValue(SetContextValue {
            key: "sampleKey".to_owned(),
            value: serde_json::json!({"example": "value"}), // Using serde_json::Value for any JSON-compatible structure
        }),
    ]
}
