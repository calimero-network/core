use borsh::BorshSerialize;

use crate::client::env::proxy::requests::{
    ActiveProposalRequest, ContextStorageEntriesRequest, ContextVariableRequest,
    ProposalApprovalsRequest, ProposalApproversRequest, ProposalRequest, ProposalsRequest,
};
use crate::client::env::Method;
use crate::client::protocol::mock_relayer::MockRelayer;
use crate::repr::Repr;
use crate::types::{ContextIdentity, ContextStorageEntry};
use crate::{Proposal, ProposalId, ProposalWithApprovals};

impl Method<MockRelayer> for ProposalsRequest {
    const METHOD: &'static str = "proposals";

    type Returns = Vec<Proposal>;

    fn encode(self) -> eyre::Result<Vec<u8>> {
        #[derive(BorshSerialize)]
        struct Request {
            offset: usize,
            length: usize,
        }

        let req = Request {
            offset: self.offset,
            length: self.length,
        };

        borsh::to_vec(&req).map_err(Into::into)
    }

    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        borsh::from_slice(&response).map_err(Into::into)
    }
}

impl Method<MockRelayer> for ProposalRequest {
    const METHOD: &'static str = "proposal";

    type Returns = Option<Proposal>;

    fn encode(self) -> eyre::Result<Vec<u8>> {
        #[derive(BorshSerialize)]
        struct Request {
            proposal_id: Repr<ProposalId>,
        }

        let req = Request {
            proposal_id: self.proposal_id,
        };

        borsh::to_vec(&req).map_err(Into::into)
    }

    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        borsh::from_slice(&response).map_err(Into::into)
    }
}

impl Method<MockRelayer> for ActiveProposalRequest {
    const METHOD: &'static str = "get_number_of_active_proposals";

    type Returns = u16;

    fn encode(self) -> eyre::Result<Vec<u8>> {
        let _ = self;
        Ok(Vec::new())
    }

    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        borsh::from_slice(&response).map_err(Into::into)
    }
}

impl Method<MockRelayer> for ProposalApprovalsRequest {
    const METHOD: &'static str = "get_number_of_proposal_approvals";

    type Returns = ProposalWithApprovals;

    fn encode(self) -> eyre::Result<Vec<u8>> {
        #[derive(BorshSerialize)]
        struct Request {
            proposal_id: Repr<ProposalId>,
        }

        let req = Request {
            proposal_id: self.proposal_id,
        };

        borsh::to_vec(&req).map_err(Into::into)
    }

    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        serde_json::from_slice(&response).map_err(Into::into)
    }
}

impl Method<MockRelayer> for ProposalApproversRequest {
    const METHOD: &'static str = "get_proposal_approvers";

    type Returns = Vec<ContextIdentity>;

    fn encode(self) -> eyre::Result<Vec<u8>> {
        #[derive(BorshSerialize)]
        struct Request {
            proposal_id: Repr<ProposalId>,
        }

        let req = Request {
            proposal_id: self.proposal_id,
        };

        borsh::to_vec(&req).map_err(Into::into)
    }

    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        borsh::from_slice(&response).map_err(Into::into)
    }
}

impl Method<MockRelayer> for ContextVariableRequest {
    const METHOD: &'static str = "get_context_value";

    type Returns = Vec<u8>;

    fn encode(self) -> eyre::Result<Vec<u8>> {
        #[derive(BorshSerialize)]
        struct Request {
            key: Vec<u8>,
        }

        let req = Request { key: self.key };

        borsh::to_vec(&req).map_err(Into::into)
    }

    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        borsh::from_slice(&response).map_err(Into::into)
    }
}

impl Method<MockRelayer> for ContextStorageEntriesRequest {
    const METHOD: &'static str = "get_context_storage_entries";

    type Returns = Vec<ContextStorageEntry>;

    fn encode(self) -> eyre::Result<Vec<u8>> {
        #[derive(BorshSerialize)]
        struct Request {
            offset: usize,
            limit: usize,
        }

        let req = Request {
            offset: self.offset,
            limit: self.limit,
        };

        borsh::to_vec(&req).map_err(Into::into)
    }

    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        borsh::from_slice(&response).map_err(Into::into)
    }
}
