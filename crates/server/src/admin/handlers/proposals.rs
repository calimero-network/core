use std::sync::Arc;

use axum::extract::Path;
use axum::response::IntoResponse;
use axum::{Extension, Json};
use calimero_context_config::repr::{Repr, ReprTransmute};
use calimero_context_config::types::ProposalId;
use calimero_context_config::{Proposal, ProposalWithApprovals};
use calimero_primitives::context::ContextId;
use calimero_server_primitives::admin::{
    ApproveProposalRequest, ApproveProposalResponse, CreateAndApproveProposalRequest,
    CreateAndApproveProposalResponse, GetContextStorageEntriesRequest,
    GetContextStorageEntriesResponse, GetContextValueRequest, GetContextValueResponse,
    GetNumberOfActiveProposalsResponse, GetNumberOfProposalApprovalsResponse,
    GetProposalApproversResponse, GetProposalResponse, GetProposalsRequest, GetProposalsResponse,
    GetProxyContractResponse,
};
use serde::{Deserialize, Serialize};

use crate::admin::service::{parse_api_error, ApiResponse};
use crate::AdminState;

//todo split it up into separate files

/// Generate a new ProposalId
fn generate_proposal_id() -> ProposalId {
    use calimero_context_config::repr::ReprBytes;
    use rand::RngCore;

    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);

    ReprBytes::from_bytes(|buf: &mut [u8; 32]| {
        *buf = bytes;
        Ok(buf.as_ref().len())
    })
    .expect("infallible conversion")
}

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
    let external_config = match state.ctx_client.context_config(&context_id) {
        Ok(Some(config)) => config,
        Ok(None) => return parse_api_error(eyre::eyre!("Context not found")).into_response(),
        Err(err) => return parse_api_error(err).into_response(),
    };

    let external_client = match state
        .ctx_client
        .external_client(&context_id, &external_config)
    {
        Ok(client) => client,
        Err(err) => return parse_api_error(err).into_response(),
    };

    match external_client
        .proxy()
        .get_proposals(req.offset, req.limit)
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
    let external_config = match state.ctx_client.context_config(&context_id) {
        Ok(Some(config)) => config,
        Ok(None) => return parse_api_error(eyre::eyre!("Context not found")).into_response(),
        Err(err) => return parse_api_error(err).into_response(),
    };

    let external_client = match state
        .ctx_client
        .external_client(&context_id, &external_config)
    {
        Ok(client) => client,
        Err(err) => return parse_api_error(err).into_response(),
    };

    match external_client.proxy().get_proposal(&proposal_id).await {
        Ok(Some(context_proposal)) => ApiResponse {
            payload: GetProposalResponse {
                data: context_proposal,
            },
        }
        .into_response(),
        Ok(None) => parse_api_error(eyre::eyre!("Proposal not found")).into_response(),
        Err(err) => parse_api_error(err).into_response(),
    }
}

pub async fn get_proxy_contract_handler(
    Path(context_id): Path<ContextId>,
    Extension(state): Extension<Arc<AdminState>>,
) -> impl IntoResponse {
    let external_config = match state.ctx_client.context_config(&context_id) {
        Ok(Some(config)) => config,
        Ok(None) => return parse_api_error(eyre::eyre!("Context not found")).into_response(),
        Err(err) => return parse_api_error(err).into_response(),
    };

    let external_client = match state
        .ctx_client
        .external_client(&context_id, &external_config)
    {
        Ok(client) => client,
        Err(err) => return parse_api_error(err).into_response(),
    };

    match external_client.config().get_proxy_contract().await {
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
    let external_config = match state.ctx_client.context_config(&context_id) {
        Ok(Some(config)) => config,
        Ok(None) => return parse_api_error(eyre::eyre!("Context not found")).into_response(),
        Err(err) => return parse_api_error(err).into_response(),
    };

    let external_client = match state
        .ctx_client
        .external_client(&context_id, &external_config)
    {
        Ok(client) => client,
        Err(err) => return parse_api_error(err).into_response(),
    };

    match external_client
        .proxy()
        .get_external_value(req.key.as_bytes().to_vec())
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
    let external_config = match state.ctx_client.context_config(&context_id) {
        Ok(Some(config)) => config,
        Ok(None) => return parse_api_error(eyre::eyre!("Context not found")).into_response(),
        Err(err) => return parse_api_error(err).into_response(),
    };

    let external_client = match state
        .ctx_client
        .external_client(&context_id, &external_config)
    {
        Ok(client) => client,
        Err(err) => return parse_api_error(err).into_response(),
    };

    match external_client
        .proxy()
        .get_external_storage_entries(req.offset, req.limit)
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
    let external_config = match state.ctx_client.context_config(&context_id) {
        Ok(Some(config)) => config,
        Ok(None) => return parse_api_error(eyre::eyre!("Context not found")).into_response(),
        Err(err) => return parse_api_error(err).into_response(),
    };

    let external_client = match state
        .ctx_client
        .external_client(&context_id, &external_config)
    {
        Ok(client) => client,
        Err(err) => return parse_api_error(err).into_response(),
    };

    match external_client.proxy().active_proposals().await {
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
    let external_config = match state.ctx_client.context_config(&context_id) {
        Ok(Some(config)) => config,
        Ok(None) => return parse_api_error(eyre::eyre!("Context not found")).into_response(),
        Err(err) => return parse_api_error(err).into_response(),
    };

    let external_client = match state
        .ctx_client
        .external_client(&context_id, &external_config)
    {
        Ok(client) => client,
        Err(err) => return parse_api_error(err).into_response(),
    };

    match external_client
        .proxy()
        .proposal_approvals(&proposal_id)
        .await
    {
        Ok(number_of_proposal_approvals) => {
            // Create a ProposalWithApprovals struct with the count
            let proposal_with_approvals = ProposalWithApprovals {
                proposal_id: proposal_id.rt().expect("Invalid proposal ID"),
                num_approvals: number_of_proposal_approvals,
            };

            ApiResponse {
                payload: GetNumberOfProposalApprovalsResponse {
                    data: proposal_with_approvals,
                },
            }
            .into_response()
        }
        Err(err) => parse_api_error(err).into_response(),
    }
}

pub async fn get_proposal_approvers_handler(
    Path((context_id, proposal_id)): Path<(ContextId, Repr<ProposalId>)>,
    Extension(state): Extension<Arc<AdminState>>,
) -> impl IntoResponse {
    let external_config = match state.ctx_client.context_config(&context_id) {
        Ok(Some(config)) => config,
        Ok(None) => return parse_api_error(eyre::eyre!("Context not found")).into_response(),
        Err(err) => return parse_api_error(err).into_response(),
    };

    let external_client = match state
        .ctx_client
        .external_client(&context_id, &external_config)
    {
        Ok(client) => client,
        Err(err) => return parse_api_error(err).into_response(),
    };

    match external_client
        .proxy()
        .get_proposal_approvers(&proposal_id)
        .await
    {
        Ok(proposal_approvers) => {
            match proposal_approvers
                // fixme! this is wrong, ContextIdentity is an implementation detail
                // fixme! it should be PublicKey, so we shouldn't be using rt() here
                .map(|pk| pk.rt())
                .collect::<Result<_, _>>()
            {
                Ok(context_identities) => ApiResponse {
                    payload: GetProposalApproversResponse {
                        data: context_identities,
                    },
                }
                .into_response(),
                Err(err) => parse_api_error(err.into()).into_response(),
            }
        }
        Err(err) => parse_api_error(err).into_response(),
    }
}

pub async fn create_and_approve_proposal_handler(
    Path(context_id): Path<ContextId>,
    Extension(state): Extension<Arc<AdminState>>,
    Json(req): Json<CreateAndApproveProposalRequest>,
) -> impl IntoResponse {
    let external_config = match state.ctx_client.context_config(&context_id) {
        Ok(Some(config)) => config,
        Ok(None) => return parse_api_error(eyre::eyre!("Context not found")).into_response(),
        Err(err) => return parse_api_error(err).into_response(),
    };

    let external_client = match state
        .ctx_client
        .external_client(&context_id, &external_config)
    {
        Ok(client) => client,
        Err(err) => return parse_api_error(err).into_response(),
    };

    let res = async {
        let signer_pk = req.signer_id;

        // Generate ProposalId
        let proposal_id = generate_proposal_id();
        let proposal = Proposal {
            id: Repr::new(proposal_id),
            author_id: Repr::new(req.author_id),
            actions: req.actions,
        };

        external_client
            .proxy()
            .propose(&signer_pk, &proposal_id, proposal.actions)
            .await?;

        external_client
            .proxy()
            .approve(&signer_pk, &proposal_id)
            .await?;

        let approvals = external_client
            .proxy()
            .proposal_approvals(&proposal_id)
            .await?;

        let data = Some(ProposalWithApprovals {
            proposal_id: Repr::new(proposal_id),
            num_approvals: approvals,
        });

        eyre::Result::<_, eyre::Report>::Ok(data)
    };

    match res.await {
        Ok(data) => ApiResponse {
            payload: CreateAndApproveProposalResponse { data },
        }
        .into_response(),
        Err(err) => parse_api_error(err).into_response(),
    }
}

pub async fn approve_proposal_handler(
    Path(context_id): Path<ContextId>,
    Extension(state): Extension<Arc<AdminState>>,
    Json(req): Json<ApproveProposalRequest>,
) -> impl IntoResponse {
    let external_config = match state.ctx_client.context_config(&context_id) {
        Ok(Some(config)) => config,
        Ok(None) => return parse_api_error(eyre::eyre!("Context not found")).into_response(),
        Err(err) => return parse_api_error(err).into_response(),
    };

    let external_client = match state
        .ctx_client
        .external_client(&context_id, &external_config)
    {
        Ok(client) => client,
        Err(err) => return parse_api_error(err).into_response(),
    };

    let res = async {
        let signer_pk = req.signer_id;
        let proposal_id = req.proposal_id;

        external_client
            .proxy()
            .approve(&signer_pk, &proposal_id)
            .await?;

        let approvals = external_client
            .proxy()
            .proposal_approvals(&proposal_id)
            .await?;

        let data = Some(ProposalWithApprovals {
            proposal_id: Repr::new(proposal_id),
            num_approvals: approvals,
        });

        eyre::Result::<_, eyre::Report>::Ok(data)
    };

    match res.await {
        Ok(data) => ApiResponse {
            payload: ApproveProposalResponse { data },
        }
        .into_response(),
        Err(err) => parse_api_error(err).into_response(),
    }
}
