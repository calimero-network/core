use std::str::FromStr;

use alloy::primitives::{Address, B256, U256};
use alloy_sol_types::{sol, SolValue};
use ethabi::{Function, Param, ParamType, Token};
use eyre::{bail, Context};

use crate::repr::ReprTransmute;
use crate::{Proposal, ProposalAction, ProposalApprovalWithSigner, ProxyMutateRequest};

sol! {
    #[derive(Debug)]
    enum SolProposalActionKind {
        ExternalFunctionCall,
        Transfer,
        SetNumApprovals,
        SetActiveProposalsLimit,
        SetContextValue,
        DeleteProposal
    }

    #[derive(Debug)]
    struct SolProposalAction {
        SolProposalActionKind kind;
        bytes data;
    }

    #[derive(Debug)]
    struct SolProposal {
        bytes32 id;
        bytes32 authorId;
        SolProposalAction[] actions;
    }

    struct ExternalFunctionCallData {
        address target;
        bytes callData;
        uint256 value;
    }

    struct TransferData {
        address recipient;
        uint256 amount;
    }

    struct ContextValueData {
        bytes key;
        bytes value;
    }

    struct SolProposalWithApprovals {
        bytes32 proposalId;
        uint32 numApprovals;
    }

    struct SolProposalApprovalWithSigner {
        bytes32 proposalId;
        bytes32 userId;
    }

    enum SolContextRequestKind {
        Add,
        AddMembers,
        RemoveMembers,
        AddCapability,
        RevokeCapability
    }

    enum SolCapability {
        ManageApplication,
        ManageMembers,
        Proxy
    }

    enum SolRequestKind {
        Propose,
        Approve
    }

    struct SolContextRequest {
        bytes32 contextId;
        SolContextRequestKind kind;
        bytes data;
    }

    struct SolRequest {
        bytes32 signerId;
        bytes32 userId;
        SolRequestKind kind;
        bytes data;
    }


    struct SolSignedRequest {
        SolRequest payload;
        bytes32 r;
        bytes32 s;
        uint8 v;
    }
}

impl From<&ProposalAction> for SolProposalActionKind {
    fn from(action: &ProposalAction) -> Self {
        match action {
            ProposalAction::ExternalFunctionCall { .. } => {
                SolProposalActionKind::ExternalFunctionCall
            }
            ProposalAction::Transfer { .. } => SolProposalActionKind::Transfer,
            ProposalAction::SetNumApprovals { .. } => SolProposalActionKind::SetNumApprovals,
            ProposalAction::SetActiveProposalsLimit { .. } => {
                SolProposalActionKind::SetActiveProposalsLimit
            }
            ProposalAction::SetContextValue { .. } => SolProposalActionKind::SetContextValue,
            ProposalAction::DeleteProposal { .. } => SolProposalActionKind::DeleteProposal,
        }
    }
}

// Add conversions from Sol types to our domain types
impl TryFrom<SolProposal> for Proposal {
    type Error = eyre::Report;

    fn try_from(sol_proposal: SolProposal) -> Result<Self, Self::Error> {
        let proposal = Proposal {
            id: sol_proposal.id.rt().wrap_err("Invalid proposal ID")?,
            author_id: sol_proposal.authorId.rt().wrap_err("Invalid author ID")?,
            actions: sol_proposal
                .actions
                .into_iter()
                .map(TryInto::try_into)
                .collect::<eyre::Result<_>>()?,
        };

        Ok(proposal)
    }
}

impl TryFrom<Proposal> for SolProposal {
    type Error = eyre::Report;

    fn try_from(proposal: Proposal) -> eyre::Result<Self> {
        let proposal_action = proposal
            .actions
            .into_iter()
            .map(SolProposalAction::try_from)
            .collect::<eyre::Result<Vec<_>>>()?;

        let proposal_id: [u8; 32] = proposal.id.rt().wrap_err("Invalid proposal ID")?;
        let signer_id: [u8; 32] = proposal.author_id.rt().wrap_err("Invalid signer ID")?;

        let proposal = SolProposal {
            id: B256::from(proposal_id),
            authorId: B256::from(signer_id),
            actions: proposal_action,
        };

        Ok(proposal)
    }
}

impl TryFrom<SolProposalAction> for ProposalAction {
    type Error = eyre::Report;

    fn try_from(action: SolProposalAction) -> Result<Self, Self::Error> {
        let res = match action.kind {
            SolProposalActionKind::ExternalFunctionCall => {
                let data: ExternalFunctionCallData = SolValue::abi_decode(&action.data, false)
                    .wrap_err("Invalid external call data")?;

                ProposalAction::ExternalFunctionCall {
                    receiver_id: format!("{:?}", data.target),
                    method_name: hex::encode(&data.callData[..4]),
                    args: hex::encode(&data.callData[4..]),
                    deposit: data
                        .value
                        .try_into()
                        .wrap_err("Amount too large for native token")?,
                }
            }
            SolProposalActionKind::Transfer => {
                let data: TransferData =
                    SolValue::abi_decode(&action.data, false).wrap_err("Invalid transfer data")?;
                ProposalAction::Transfer {
                    receiver_id: format!("{:?}", data.recipient),
                    amount: data
                        .amount
                        .try_into()
                        .wrap_err("Amount too large for native token")?,
                }
            }
            SolProposalActionKind::SetNumApprovals => {
                let num_approvals: u32 = SolValue::abi_decode(&action.data, false)
                    .wrap_err("Invalid num approvals data")?;
                ProposalAction::SetNumApprovals { num_approvals }
            }
            SolProposalActionKind::SetActiveProposalsLimit => {
                let active_proposals_limit: u32 = SolValue::abi_decode(&action.data, false)
                    .wrap_err("Invalid proposals limit data")?;
                ProposalAction::SetActiveProposalsLimit {
                    active_proposals_limit,
                }
            }
            SolProposalActionKind::SetContextValue => {
                let data: ContextValueData = SolValue::abi_decode(&action.data, false)
                    .wrap_err("Invalid context value data")?;
                ProposalAction::SetContextValue {
                    key: Vec::from(data.key).into_boxed_slice(),
                    value: Vec::from(data.value).into_boxed_slice(),
                }
            }
            SolProposalActionKind::DeleteProposal => {
                let proposal_id: [u8; 32] = SolValue::abi_decode(&action.data, false)
                    .wrap_err("Invalid proposal id data")?;
                ProposalAction::DeleteProposal {
                    proposal_id: proposal_id.rt().wrap_err("Invalid proposal id")?,
                }
            }
            SolProposalActionKind::__Invalid => {
                bail!("Invalid proposal action kind encountered in response")
            }
        };

        Ok(res)
    }
}

impl TryFrom<ProposalAction> for SolProposalAction {
    type Error = eyre::Report;

    fn try_from(action: ProposalAction) -> Result<Self, Self::Error> {
        let kind = SolProposalActionKind::from(&action);

        let data = match action {
            ProposalAction::ExternalFunctionCall {
                receiver_id,
                method_name,
                args,
                deposit,
            } => {
                let parsed_args: Vec<(String, String)> =
                    serde_json::from_str(&args).wrap_err("Invalid args format")?;

                let (tokens, inputs) = parsed_args
                    .iter()
                    .enumerate()
                    .map(|(index, (key, value))| {
                        let (token, param_type) = match key.as_str() {
                            "bool" => (
                                Token::Bool(value.parse().wrap_err("Invalid bool")?),
                                ParamType::Bool,
                            ),
                            "string" => (Token::String(value.clone()), ParamType::String),
                            "address" => (
                                Token::Address(value.parse().wrap_err("Invalid address")?),
                                ParamType::Address,
                            ),
                            "bytes" => (
                                Token::Bytes(hex::decode(value).wrap_err("Invalid hex bytes")?),
                                ParamType::Bytes,
                            ),
                            "int256" => (
                                Token::Int(value.parse().wrap_err("Invalid int256")?),
                                ParamType::Int(256),
                            ),
                            "uint256" => (
                                Token::Uint(value.parse().wrap_err("Invalid uint256")?),
                                ParamType::Uint(256),
                            ),
                            "array(bool)" => (
                                Token::Array(
                                    serde_json::from_str(value).wrap_err("Invalid array")?,
                                ),
                                ParamType::Array(Box::new(ParamType::Bool)),
                            ),
                            "array(string)" => (
                                Token::Array(
                                    serde_json::from_str(value).wrap_err("Invalid array")?,
                                ),
                                ParamType::Array(Box::new(ParamType::String)),
                            ),
                            "array(address)" => (
                                Token::Array(
                                    serde_json::from_str(value).wrap_err("Invalid array")?,
                                ),
                                ParamType::Array(Box::new(ParamType::Address)),
                            ),
                            "array(bytes)" => (
                                Token::Array(
                                    serde_json::from_str(value).wrap_err("Invalid array")?,
                                ),
                                ParamType::Array(Box::new(ParamType::Bytes)),
                            ),
                            "array(int256)" => (
                                Token::Array(
                                    serde_json::from_str(value).wrap_err("Invalid array")?,
                                ),
                                ParamType::Array(Box::new(ParamType::Int(256))),
                            ),
                            "array(uint256)" => (
                                Token::Array(
                                    serde_json::from_str(value).wrap_err("Invalid array")?,
                                ),
                                ParamType::Array(Box::new(ParamType::Uint(256))),
                            ),
                            "tuple" => (
                                Token::Tuple(
                                    serde_json::from_str(value).wrap_err("Invalid tuple")?,
                                ),
                                ParamType::Tuple(vec![]),
                            ),
                            _ => eyre::bail!("Unsupported type: {}", key),
                        };
                        let param = Param {
                            name: format!("param{}", index),
                            kind: param_type,
                            internal_type: None,
                        };
                        Ok((token, param))
                    })
                    .collect::<eyre::Result<(Vec<_>, Vec<_>)>>()?;

                let state_mutability = if deposit > 0 {
                    ethabi::StateMutability::Payable
                } else {
                    ethabi::StateMutability::NonPayable
                };
                let amount = U256::from(deposit);

                #[allow(deprecated, reason = "Using deprecated constant field")]
                let function = Function {
                    name: method_name,
                    inputs,
                    outputs: vec![],
                    constant: Some(false),
                    state_mutability,
                };

                let call_data = function.encode_input(&tokens).wrap_err("Encoding error")?;

                let contract_address =
                    Address::from_str(&receiver_id).wrap_err("Invalid address")?;

                (contract_address, call_data, amount).abi_encode()
            }
            ProposalAction::Transfer {
                receiver_id,
                amount,
            } => {
                let data = TransferData {
                    recipient: Address::from_str(&receiver_id)
                        .wrap_err(format!("Invalid receiver address format"))?,
                    amount: U256::from(amount),
                };
                data.abi_encode()
            }
            ProposalAction::SetNumApprovals { num_approvals } => num_approvals.abi_encode(),
            ProposalAction::SetActiveProposalsLimit {
                active_proposals_limit,
            } => active_proposals_limit.abi_encode(),
            ProposalAction::SetContextValue { key, value } => {
                let data = ContextValueData {
                    key: key.into(),
                    value: value.into(),
                };
                data.abi_encode()
            }
            ProposalAction::DeleteProposal { proposal_id } => {
                let proposal_id: [u8; 32] = proposal_id.rt().wrap_err("Invalid proposal ID")?;
                proposal_id.abi_encode()
            }
        };

        Ok(SolProposalAction {
            kind,
            data: data.into(),
        })
    }
}

impl From<&ProxyMutateRequest> for SolRequestKind {
    fn from(request: &ProxyMutateRequest) -> Self {
        match request {
            ProxyMutateRequest::Propose { .. } => SolRequestKind::Propose,
            ProxyMutateRequest::Approve { .. } => SolRequestKind::Approve,
        }
    }
}

impl From<ProposalApprovalWithSigner> for SolProposalApprovalWithSigner {
    fn from(approval: ProposalApprovalWithSigner) -> Self {
        let proposal_id: [u8; 32] = approval.proposal_id.rt().expect("infallible conversion");
        let signer_id: [u8; 32] = approval.signer_id.rt().expect("infallible conversion");

        SolProposalApprovalWithSigner {
            proposalId: B256::from(proposal_id),
            userId: B256::from(signer_id),
        }
    }
}
