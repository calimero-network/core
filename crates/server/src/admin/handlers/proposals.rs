use std::sync::Arc;
use std::vec;

use axum::extract::Path;
use axum::response::IntoResponse;
use axum::{Extension, Json};
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
    pub(crate) identity_public_key: String,
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
    pub(crate) id: String,
    pub(crate) author: User,
    pub(crate) actions: Vec<Action>,
    pub(crate) title: String,
    pub(crate) description: String,
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
    data: Vec<Proposal>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GetProposalResponse {
    data: Proposal,
}

pub async fn get_proposals_handler(
    session: Session,
    Extension(state): Extension<Arc<AdminState>>,
    Json(req): Json<GetProposalsResponse>,
) -> impl IntoResponse {
    let sample_action = Action::ExternalFunctionCall(ExternalFunctionCall {
        receiver_id: get_mock_user(),
        method_name: "sampleMethod".to_string(),
        args: serde_json::json!({"example": "value"}),
        deposit: "100".to_string(),
        gas: "10".to_string(),
    });

    let proposals = vec![Proposal {
        id: "proposal_1".to_string(),
        author: get_mock_user(),
        actions: vec![sample_action],
        title: "Proposal 1".to_string(),
        description: "This is the first proposal.".to_string(),
        created_at: "2024-10-31T12:00:00Z".to_string(),
    }];

    ApiResponse {
        payload: GetProposalsResponse { data: proposals },
    }
    .into_response()
}

pub async fn get_proposal_handler(
    Path(proposal_id): Path<String>,
    session: Session,
    Extension(state): Extension<Arc<AdminState>>,
    Json(req): Json<GetProposalResponse>,
) -> impl IntoResponse {
    let proposal = Proposal {
        id: "proposal_1".to_string(),
        author: get_mock_user(),
        actions: get_mock_actions(),
        title: "Proposal Title".to_string(),
        description: "Proposal Description".to_string(),
        created_at: "2024-10-31T12:00:00Z".to_string(),
    };

    ApiResponse {
        payload: GetProposalResponse { data: proposal },
    }
    .into_response()
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GetNumberOfActiveProposalsResponse {
    pub(crate) data: u16,
}

pub async fn get_number_of_active_proposals_handler(
    session: Session,
    Extension(state): Extension<Arc<AdminState>>,
    Json(req): Json<GetNumberOfActiveProposalsResponse>,
) -> impl IntoResponse {
    ApiResponse {
        payload: GetNumberOfActiveProposalsResponse { data: 4 },
    }
    .into_response()
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GetNumberOfProposalApprovalsResponse {
    data: u16,
}

pub async fn get_number_of_proposal_approvals_handler(
    Path(proposal_id): Path<String>,
    session: Session,
    Extension(state): Extension<Arc<AdminState>>,
    Json(req): Json<GetNumberOfProposalApprovalsResponse>,
) -> impl IntoResponse {
    ApiResponse {
        payload: GetNumberOfProposalApprovalsResponse { data: 5 },
    }
    .into_response()
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GetProposalApproversResponse {
    data: Vec<User>,
}

pub async fn get_proposal_approvers_handler(
    Path(proposal_id): Path<String>,
    session: Session,
    Extension(state): Extension<Arc<AdminState>>,
    Json(req): Json<GetProposalApproversResponse>,
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
        identity_public_key: "sample_public_key".to_string(),
    }
}

pub fn get_mock_actions() -> Vec<Action> {
    vec![
        Action::ExternalFunctionCall(ExternalFunctionCall {
            receiver_id: get_mock_user(),
            method_name: "sampleMethod".to_string(),
            args: serde_json::json!({"example": "value"}),
            deposit: "100".to_string(),
            gas: "5000".to_string(),
        }),
        Action::Transfer(Transfer {
            amount: "250".to_string(),
        }),
        Action::SetNumApprovals(SetNumApprovals {
            num_of_approvals: 3,
        }),
        Action::SetActiveProposalsLimit(SetActiveProposalsLimit {
            active_proposals_limit: 10,
        }),
        Action::SetContextValue(SetContextValue {
            key: "sampleKey".to_string(),
            value: serde_json::json!({"example": "value"}), // Using serde_json::Value for any JSON-compatible structure
        }),
    ]
}
