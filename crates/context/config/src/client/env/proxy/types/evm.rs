use std::str::FromStr;

use alloy::primitives::{Address, B256, U256};
use alloy_sol_types::{sol, SolValue};
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
                    method_name: String::from_utf8(data.callData[..4].to_vec())
                        .expect("Invalid method name"),
                    args: String::from_utf8(data.callData[4..].to_vec()).expect("Invalid args"),
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

impl From<ProposalAction> for SolProposalAction {
    fn from(action: ProposalAction) -> Self {
        SolProposalAction {
            kind: action.clone().into(),
            data: match action {
                ProposalAction::ExternalFunctionCall {
                    receiver_id,
                    method_name,
                    args,
                    deposit,
                } => {
                    let parsed_args: Vec<(String, String)> =
                        serde_json::from_str(&args).expect("Invalid args format");

                    let tokens: Vec<Token> = parsed_args
                        .iter()
                        .flat_map(|(key, value)| {
                            let token = match key.as_str() {
                                "bool" => Token::Bool(value.parse().expect("Invalid bool")),
                                "string" => Token::String(value.clone()),
                                "address" => {
                                    Token::Address(value.parse().expect("Invalid address"))
                                }
                                "bytes" => {
                                    Token::Bytes(hex::decode(value).expect("Invalid hex bytes"))
                                }
                                "int256" => Token::Int(value.parse().expect("Invalid int256")),
                                "uint256" => Token::Uint(value.parse().expect("Invalid uint256")),
                                "array(string)" => Token::Array(
                                    serde_json::from_str(value).expect("Invalid array"),
                                ),
                                "array(uint256)" => Token::Array(
                                    serde_json::from_str(value).expect("Invalid array"),
                                ),
                                "array(int256)" => Token::Array(
                                    serde_json::from_str(value).expect("Invalid array"),
                                ),
                                "array(address)" => Token::Array(
                                    serde_json::from_str(value).expect("Invalid array"),
                                ),
                                "array(bool)" => Token::Array(
                                    serde_json::from_str(value).expect("Invalid array"),
                                ),
                                "array(bytes)" => Token::Array(
                                    serde_json::from_str(value).expect("Invalid array"),
                                ),
                                "tuple" => Token::Tuple(
                                    serde_json::from_str(value).expect("Invalid tuple"),
                                ),
                                _ => panic!("Unsupported type: {}", key),
                            };
                            vec![token]
                        })
                        .collect();

                    let state_mutability = if deposit > 0 {
                        ethabi::StateMutability::Payable
                    } else {
                        ethabi::StateMutability::NonPayable
                    };
                    let amount = U256::from(deposit);

                    #[allow(deprecated)]
                    let function = Function {
                        name: method_name,
                        inputs: parsed_args
                            .iter()
                            .enumerate()
                            .map(|(index, (key, _value))| {
                                let param_type = match key.as_str() {
                                    "string" => ParamType::String,
                                    "uint256" => ParamType::Uint(256),
                                    "int256" => ParamType::Int(256),
                                    "address" => ParamType::Address,
                                    "bool" => ParamType::Bool,
                                    "bytes" => ParamType::Bytes,
                                    "array(string)" => {
                                        ParamType::Array(Box::new(ParamType::String))
                                    }
                                    "array(uint256)" => {
                                        ParamType::Array(Box::new(ParamType::Uint(256)))
                                    }
                                    "array(int256)" => {
                                        ParamType::Array(Box::new(ParamType::Int(256)))
                                    }
                                    "array(address)" => {
                                        ParamType::Array(Box::new(ParamType::Address))
                                    }
                                    "array(bool)" => ParamType::Array(Box::new(ParamType::Bool)),
                                    "array(bytes)" => ParamType::Array(Box::new(ParamType::Bytes)),
                                    "tuple" => ParamType::Tuple(vec![]),
                                    _ => panic!("Unsupported parameter type: {}", key),
                                };

                                Param {
                                    name: format!("param{}", index),
                                    kind: param_type,
                                    internal_type: None,
                                }
                            })
                            .collect(),
                        outputs: vec![],
                        constant: Some(false),
                        state_mutability,
                    };

                    let call_data = function.encode_input(&tokens).unwrap();

                    let contract_address =
                        Address::from_str(&receiver_id).expect("Invalid address");
                    let data = SolValue::abi_encode(&(contract_address, call_data, amount));

                    data.into()
                }
                ProposalAction::Transfer {
                    receiver_id,
                    amount,
                } => {
                    let data = TransferData {
                        recipient: receiver_id.parse().expect("Invalid address"),
                        amount: U256::from(amount),
                    };
                    data.abi_encode().into()
                }
                ProposalAction::SetNumApprovals { num_approvals } => {
                    num_approvals.abi_encode().into()
                }
                ProposalAction::SetActiveProposalsLimit {
                    active_proposals_limit,
                } => active_proposals_limit.abi_encode().into(),
                ProposalAction::SetContextValue { key, value } => {
                    let data = ContextValueData {
                        key: key.to_vec().into(),
                        value: value.to_vec().into(),
                    };
                    data.abi_encode().into()
                }
                ProposalAction::DeleteProposal { proposal_id } => {
                    let proposal_id: [u8; 32] = proposal_id.rt().expect("infallible conversion");
                    proposal_id.abi_encode().into()
                }
            },
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

impl From<&ProxyMutateRequest> for Vec<u8> {
    fn from(request: &ProxyMutateRequest) -> Self {
        match request {
            ProxyMutateRequest::Propose { proposal } => {
                let proposal_action: Vec<SolProposalAction> = proposal
                    .actions
                    .iter()
                    .map(|action| SolProposalAction::from(action.clone()))
                    .collect();
                let proposal_id: [u8; 32] = proposal.id.rt().expect("infallible conversion");
                let signer_id: [u8; 32] = proposal.author_id.rt().expect("infallible conversion");

                let sol_proposal = SolProposal {
                    id: B256::from(proposal_id),
                    authorId: B256::from(signer_id),
                    actions: proposal_action,
                };

                SolValue::abi_encode(&sol_proposal)
            }
            ProxyMutateRequest::Approve { approval } => {
                let proposal_id: [u8; 32] =
                    approval.proposal_id.rt().expect("infallible conversion");
                let signer_id: [u8; 32] = approval.signer_id.rt().expect("infallible conversion");
                let proposal_approval = SolProposalApprovalWithSigner {
                    proposalId: B256::from(proposal_id),
                    userId: B256::from(signer_id),
                };
                SolValue::abi_encode(&proposal_approval)
            }
        }
    }
}
