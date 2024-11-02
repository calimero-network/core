use super::ContextProxyMutate;
use crate::client::env::{utils, Method};
use crate::client::protocol::near::Near;
use crate::client::protocol::starknet::Starknet;
use crate::client::transport::Transport;
use crate::client::{CallClient, Error, Operation};

// todo! this should be replaced with primitives lib
#[derive(Debug)]
pub enum Proposal {
    Transfer { recipient: String, amount: u64 },
    // __
}

impl<'a, T> ContextProxyMutate<'a, T> {
    pub fn propose(self, proposal: Proposal) -> ContextProxyProposeRequest<'a, T> {
        ContextProxyProposeRequest {
            client: self.client,
            proposal,
        }
    }
}

#[derive(Debug)]
pub struct ContextProxyProposeRequest<'a, T> {
    client: CallClient<'a, T>,
    proposal: Proposal,
}

struct Propose {
    signing_key: [u8; 32],
    proposal: Proposal,
}

impl Method<Near> for Propose {
    const METHOD: &'static str = "propose";

    type Returns = ();

    fn encode(self) -> eyre::Result<Vec<u8>> {
        // sign the params, encode it and return
        todo!()
    }

    fn decode(_response: Vec<u8>) -> eyre::Result<Self::Returns> {
        todo!()
    }
}

impl Method<Starknet> for Propose {
    type Returns = ();

    const METHOD: &'static str = "propose";

    fn encode(self) -> eyre::Result<Vec<u8>> {
        todo!()
    }

    fn decode(_response: Vec<u8>) -> eyre::Result<Self::Returns> {
        todo!()
    }
}

impl<'a, T: Transport> ContextProxyProposeRequest<'a, T> {
    pub async fn send(self, signing_key: [u8; 32]) -> Result<(), Error<T>> {
        let request = Propose {
            signing_key,
            proposal: self.proposal,
        };

        utils::send_near_or_starknet(&self.client, Operation::Write(request)).await
    }
}
