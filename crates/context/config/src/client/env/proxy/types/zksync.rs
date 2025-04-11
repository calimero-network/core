use std::str::FromStr;

use alloy_sol_types::sol;
use eyre::{bail, Context};
use serde::{};
use zksync_web3_rs::eip712::{Eip712Meta, Eip712TransactionRequest};
use zksync_web3_rs::types::{Address, BlockNumber, U256};

use crate::{ProposalAction, ProxyMutateRequest};

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

#[derive(Debug)]
pub struct ZkSyncProxyTransaction {
    pub to: Address,
    pub value: U256,
    pub meta: Option<Eip712Meta>,
    pub block_number: BlockNumber,
}

impl From<ZkSyncProxyTransaction> for Eip712TransactionRequest {
    fn from(tx: ZkSyncProxyTransaction) -> Self {
        let mut request = Self::new();
        request = request.to(tx.to);
        request = request.value(tx.value);
        if let Some(meta) = tx.meta {
            request = request.custom_data(meta);
        }
        request
    }
}

impl Default for ZkSyncProxyTransaction {
    fn default() -> Self {
        Self {
            to: Address::zero(),
            value: U256::zero(),
            meta: None,
            block_number: BlockNumber::Latest,
        }
    }
}

impl ZkSyncProxyTransaction {
    pub fn new(to: Address, value: U256) -> Self {
        Self {
            to,
            value,
            meta: None,
            block_number: BlockNumber::Latest,
        }
    }

    pub fn with_eip712_meta(mut self, meta: Eip712Meta) -> Self {
        self.meta = Some(meta);
        self
    }
}

impl TryFrom<ProxyMutateRequest> for ZkSyncProxyTransaction {
    type Error = eyre::Report;

    fn try_from(request: ProxyMutateRequest) -> Result<Self, Self::Error> {
        match request {
            ProxyMutateRequest::Propose { proposal } => {
                // Extract the first ExternalFunctionCall or Transfer action
                let action = proposal
                    .actions
                    .first()
                    .ok_or_else(|| eyre::eyre!("No actions in proposal"))?;
                match action {
                    ProposalAction::ExternalFunctionCall {
                        receiver_id,
                        deposit,
                        ..
                    } => {
                        let to = Address::from_str(receiver_id).wrap_err("Invalid address")?;
                        let value = U256::from(*deposit);
                        Ok(Self {
                            to,
                            value,
                            meta: None,
                            block_number: BlockNumber::Latest,
                        })
                    }
                    ProposalAction::Transfer {
                        receiver_id,
                        amount,
                    } => {
                        let to = Address::from_str(receiver_id).wrap_err("Invalid address")?;
                        let value = U256::from(*amount);
                        Ok(Self {
                            to,
                            value,
                            meta: None,
                            block_number: BlockNumber::Latest,
                        })
                    }
                    ProposalAction::SetNumApprovals { .. }
                    | ProposalAction::SetActiveProposalsLimit { .. }
                    | ProposalAction::SetContextValue { .. }
                    | ProposalAction::DeleteProposal { .. } => {
                        bail!("Unsupported proposal action type")
                    }
                }
            }
            ProxyMutateRequest::Approve { .. } => {
                bail!("Approve requests do not generate transactions")
            }
        }
    }
}
