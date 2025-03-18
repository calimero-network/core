use std::str::FromStr;

use alloy::primitives::{Address, B256, U256};
use alloy_sol_types::{sol, SolType, SolValue};
use ethabi::{Function, Param, ParamType, Token};

use crate::repr::{Repr, ReprTransmute};
use crate::types::{Identity, ProposalId, SignerId};
use crate::{Proposal, ProposalAction, ProxyMutateRequest};

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

impl From<ProposalAction> for SolProposalActionKind {
    fn from(action: ProposalAction) -> Self {
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
impl From<SolProposal> for Proposal {
    fn from(sol_proposal: SolProposal) -> Self {
        Proposal {
            id: Repr::new(ProposalId(Identity(sol_proposal.id.into()))),
            author_id: Repr::new(SignerId(Identity(sol_proposal.authorId.into()))),
            actions: sol_proposal.actions.into_iter().map(Into::into).collect(),
        }
    }
}

impl From<SolProposalAction> for ProposalAction {
    fn from(action: SolProposalAction) -> Self {
        match action.kind {
            SolProposalActionKind::ExternalFunctionCall => {
                let data: ExternalFunctionCallData =
                    SolValue::abi_decode(&action.data, false).expect("Invalid external call data");
                ProposalAction::ExternalFunctionCall {
                    receiver_id: format!("{:?}", data.target),
                    method_name: hex::encode(&data.callData[..4]),
                    args: hex::encode(&data.callData[4..]),
                    deposit: data
                        .value
                        .try_into()
                        .expect("Amount too large for native token"),
                }
            }
            SolProposalActionKind::Transfer => {
                let data: TransferData =
                    SolValue::abi_decode(&action.data, false).expect("Invalid transfer data");
                ProposalAction::Transfer {
                    receiver_id: format!("{:?}", data.recipient),
                    amount: data
                        .amount
                        .try_into()
                        .expect("Amount too large for native token"),
                }
            }
            SolProposalActionKind::SetNumApprovals => {
                let num_approvals: u32 =
                    SolValue::abi_decode(&action.data, false).expect("Invalid num approvals data");
                ProposalAction::SetNumApprovals { num_approvals }
            }
            SolProposalActionKind::SetActiveProposalsLimit => {
                let active_proposals_limit: u32 = SolValue::abi_decode(&action.data, false)
                    .expect("Invalid proposals limit data");
                ProposalAction::SetActiveProposalsLimit {
                    active_proposals_limit,
                }
            }
            SolProposalActionKind::SetContextValue => {
                let data: ContextValueData =
                    SolValue::abi_decode(&action.data, false).expect("Invalid context value data");
                ProposalAction::SetContextValue {
                    key: data.key.to_vec().into_boxed_slice(),
                    value: data.value.to_vec().into_boxed_slice(),
                }
            }
            SolProposalActionKind::DeleteProposal => {
                let proposal_id: [u8; 32] =
                    SolValue::abi_decode(&action.data, false).expect("Invalid proposal id data");
                ProposalAction::DeleteProposal {
                    proposal_id: Repr::new(ProposalId(Identity(proposal_id))),
                }
            }
            SolProposalActionKind::__Invalid => {
                panic!("Invalid proposal action kind encountered in response")
            }
        }
    }
}

impl TryFrom<ProposalAction> for SolProposalAction {
    type Error = String;

    fn try_from(action: ProposalAction) -> Result<Self, Self::Error> {
        let kind: <SolProposalActionKind as SolType>::RustType = action.clone().into();
        let data: Result<Vec<u8>, Self::Error> = match action {
            ProposalAction::ExternalFunctionCall {
                receiver_id,
                method_name,
                args,
                deposit,
            } => {
                let parsed_args: Vec<(String, String)> =
                    serde_json::from_str(&args).map_err(|_| "Invalid args format".to_string())?;

                let (tokens, inputs): (Vec<Token>, Vec<Param>) = parsed_args
                    .iter()
                    .enumerate()
                    .map(|(index, (key, value))| {
                        let (token, param_type) = match key.as_str() {
                            "bool" => (
                                Token::Bool(value.parse().map_err(|_| "Invalid bool".to_string())?),
                                ParamType::Bool,
                            ),
                            "string" => (Token::String(value.clone()), ParamType::String),
                            "address" => (
                                Token::Address(
                                    value.parse().map_err(|_| "Invalid address".to_string())?,
                                ),
                                ParamType::Address,
                            ),
                            "bytes" => (
                                Token::Bytes(
                                    hex::decode(value)
                                        .map_err(|_| "Invalid hex bytes".to_string())?,
                                ),
                                ParamType::Bytes,
                            ),
                            "int256" => (
                                Token::Int(
                                    value.parse().map_err(|_| "Invalid int256".to_string())?,
                                ),
                                ParamType::Int(256),
                            ),
                            "uint256" => (
                                Token::Uint(
                                    value.parse().map_err(|_| "Invalid uint256".to_string())?,
                                ),
                                ParamType::Uint(256),
                            ),
                            "array(bool)" => (
                                Token::Array(
                                    serde_json::from_str(value)
                                        .map_err(|_| "Invalid array".to_string())?,
                                ),
                                ParamType::Array(Box::new(ParamType::Bool)),
                            ),
                            "array(string)" => (
                                Token::Array(
                                    serde_json::from_str(value)
                                        .map_err(|_| "Invalid array".to_string())?,
                                ),
                                ParamType::Array(Box::new(ParamType::String)),
                            ),
                            "array(address)" => (
                                Token::Array(
                                    serde_json::from_str(value)
                                        .map_err(|_| "Invalid array".to_string())?,
                                ),
                                ParamType::Array(Box::new(ParamType::Address)),
                            ),
                            "array(bytes)" => (
                                Token::Array(
                                    serde_json::from_str(value)
                                        .map_err(|_| "Invalid array".to_string())?,
                                ),
                                ParamType::Array(Box::new(ParamType::Bytes)),
                            ),
                            "array(int256)" => (
                                Token::Array(
                                    serde_json::from_str(value)
                                        .map_err(|_| "Invalid array".to_string())?,
                                ),
                                ParamType::Array(Box::new(ParamType::Int(256))),
                            ),
                            "array(uint256)" => (
                                Token::Array(
                                    serde_json::from_str(value)
                                        .map_err(|_| "Invalid array".to_string())?,
                                ),
                                ParamType::Array(Box::new(ParamType::Uint(256))),
                            ),
                            "tuple" => (
                                Token::Tuple(
                                    serde_json::from_str(value)
                                        .map_err(|_| "Invalid tuple".to_string())?,
                                ),
                                ParamType::Tuple(vec![]),
                            ),
                            _ => return Err(format!("Unsupported type: {}", key)),
                        };
                        let param = Param {
                            name: format!("param{}", index),
                            kind: param_type,
                            internal_type: None,
                        };
                        Ok((token, param))
                    })
                    .collect::<Result<(Vec<_>, Vec<_>), _>>()?;

                let state_mutability = if deposit > 0 {
                    ethabi::StateMutability::Payable
                } else {
                    ethabi::StateMutability::NonPayable
                };
                let amount = U256::from(deposit);

                #[allow(deprecated)]
                let function = Function {
                    name: method_name,
                    inputs,
                    outputs: vec![],
                    constant: Some(false),
                    state_mutability,
                };

                let call_data = function
                    .encode_input(&tokens)
                    .map_err(|_| "Encoding error".to_string())?;

                let contract_address =
                    Address::from_str(&receiver_id).map_err(|_| "Invalid address".to_string())?;

                Ok((contract_address, call_data, amount).abi_encode().into())
            }
            ProposalAction::Transfer {
                receiver_id,
                amount,
            } => {
                let data = TransferData {
                    recipient: Address::from_str(&receiver_id)
                        .map_err(|e| format!("Invalid receiver address format: {}", e))?,
                    amount: U256::from(amount),
                };
                Ok(data.abi_encode().into())
            }
            ProposalAction::SetNumApprovals { num_approvals } => {
                Ok(num_approvals.abi_encode().into())
            }
            ProposalAction::SetActiveProposalsLimit {
                active_proposals_limit,
            } => Ok(active_proposals_limit.abi_encode().into()),
            ProposalAction::SetContextValue { key, value } => {
                let data = ContextValueData {
                    key: key.to_vec().into(),
                    value: value.to_vec().into(),
                };
                Ok(data.abi_encode().into())
            }
            ProposalAction::DeleteProposal { proposal_id } => {
                let proposal_id: [u8; 32] = proposal_id
                    .rt()
                    .map_err(|_| "Invalid proposal ID".to_string())?;
                Ok(proposal_id.abi_encode().into())
            }
        };

        match data {
            Ok(data) => Ok(SolProposalAction {
                kind,
                data: data.into(),
            }),
            Err(e) => Err(e),
        }
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

impl TryFrom<&ProxyMutateRequest> for Vec<u8> {
    type Error = String;

    fn try_from(request: &ProxyMutateRequest) -> Result<Self, Self::Error> {
        match request {
            ProxyMutateRequest::Propose { proposal } => {
                let proposal_action: Result<Vec<SolProposalAction>, Self::Error> = proposal
                    .actions
                    .iter()
                    .map(|action| {
                        SolProposalAction::try_from(action.clone())
                            .map_err(|e| format!("Invalid proposal action: {}", e))
                    })
                    .collect();
                let proposal_id: [u8; 32] = proposal
                    .id
                    .rt()
                    .map_err(|_| "Invalid proposal ID".to_string())?;
                let signer_id: [u8; 32] = proposal
                    .author_id
                    .rt()
                    .map_err(|_| "Invalid signer ID".to_string())?;

                match proposal_action {
                    Ok(proposal_action) => {
                        let sol_proposal = SolProposal {
                            id: B256::from(proposal_id),
                            authorId: B256::from(signer_id),
                            actions: proposal_action,
                        };
                        Ok(sol_proposal.abi_encode())
                    }
                    Err(e) => Err(e),
                }
            }
            ProxyMutateRequest::Approve { approval } => {
                let proposal_id: [u8; 32] = approval
                    .proposal_id
                    .rt()
                    .map_err(|_| "Invalid proposal ID".to_string())?;
                let signer_id: [u8; 32] = approval
                    .signer_id
                    .rt()
                    .map_err(|_| "Invalid signer ID".to_string())?;
                let proposal_approval = SolProposalApprovalWithSigner {
                    proposalId: B256::from(proposal_id),
                    userId: B256::from(signer_id),
                };
                Ok(proposal_approval.abi_encode())
            }
        }
    }
}
