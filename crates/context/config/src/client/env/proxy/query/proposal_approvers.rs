use serde::Serialize;

use super::ProposalId;
use crate::client::env::Method;
use crate::client::protocol::near::Near;
use crate::client::protocol::starknet::Starknet;
use crate::repr::{Repr, ReprTransmute};
use crate::types::ContextIdentity;

#[derive(Clone, Debug, Serialize)]
pub(super) struct ProposalApproversRequest {
    pub(super) proposal_id: Repr<ProposalId>,
}

impl Method<Near> for ProposalApproversRequest {
    const METHOD: &'static str = "get_proposal_approvers";

    type Returns = Vec<ContextIdentity>;

    fn encode(self) -> eyre::Result<Vec<u8>> {
        serde_json::to_vec(&self).map_err(Into::into)
    }

    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        let signers: Option<Vec<Repr<ContextIdentity>>> = serde_json::from_slice(&response)?;

        Ok(signers
            .unwrap_or_default()
            .into_iter()
            .map(|signer_id| signer_id.rt().expect("infallible conversion"))
            .collect())
    }
}

impl Method<Starknet> for ProposalApproversRequest {
    const METHOD: &'static str = "proposal_approvers";

    type Returns = Vec<ContextIdentity>;

    fn encode(self) -> eyre::Result<Vec<u8>> {
        todo!()
    }

    fn decode(_response: Vec<u8>) -> eyre::Result<Self::Returns> {
        todo!()
    }
}
