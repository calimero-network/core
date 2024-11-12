use proposal::ProposalRequest;
use proposals::ProposalsRequest;

use crate::client::env::utils;
use crate::client::transport::Transport;
use crate::client::{CallClient, ClientError, Operation};
use crate::{Proposal, ProposalId};

mod proposal;
mod proposals;

#[derive(Debug)]
pub struct ContextProxyQuery<'a, T> {
    pub client: CallClient<'a, T>,
}

impl<'a, T: Transport> ContextProxyQuery<'a, T> {
    pub async fn proposals(
        &self,
        offset: usize,
        length: usize,
    ) -> Result<Vec<Proposal>, ClientError<T>> {
        let params = ProposalsRequest { offset, length };
        utils::send_near_or_starknet(&self.client, Operation::Read(params)).await
    }

    pub async fn proposal(&self, proposal_id: String) -> Result<Option<Proposal>, ClientError<T>> {
        let proposal_id_vec = bs58::decode(proposal_id).into_vec().unwrap();
        let proposal_id: [u8; 32] = proposal_id_vec
            .try_into()
            .expect("slice with incorrect length");

        let params = ProposalRequest { proposal_id };
        utils::send_near_or_starknet(&self.client, Operation::Read(params)).await
    }
}
