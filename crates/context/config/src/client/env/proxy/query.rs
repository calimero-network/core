use proposals::ProposalRequest;

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
    ) -> Result<Vec<(ProposalId, Proposal)>, ClientError<T>> {
        let params = ProposalRequest { offset, length };
        utils::send_near_or_starknet(&self.client, Operation::Read(params)).await
    }
}
