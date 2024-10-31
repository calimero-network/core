use crate::client::protocol::near::Near;
use crate::client::protocol::starknet::Starknet;
use crate::client::protocol::Method;
use crate::client::{CallClient, ConfigError, Environment, Protocol, Transport};

pub enum ContextProxy {}

pub struct ContextProxyQuery<'a, T> {
    client: CallClient<'a, T>,
}

pub struct ContextProxyMutate<'a, T> {
    client: CallClient<'a, T>,
}

impl<'a, T: 'a> Environment<'a, T> for ContextProxy {
    type Query = ContextProxyQuery<'a, T>;
    type Mutate = ContextProxyMutate<'a, T>;

    fn query(client: CallClient<'a, T>) -> Self::Query {
        todo!()
    }

    fn mutate(client: CallClient<'a, T>) -> Self::Mutate {
        todo!()
    }
}

impl<'a, T: Transport> ContextProxyQuery<'a, T> {
    async fn proposals(&self, offset: usize, length: usize) -> Result<Vec<String>, ConfigError<T>> {
        todo!()
    }
}

enum Proposal {
    Transfer { recipient: String, amount: u64 },
}

impl<'a, T> ContextProxyMutate<'a, T> {
    pub fn propose(self, proposal: Proposal) -> ContextProxyProposeRequest<'a, T> {
        ContextProxyProposeRequest {
            client: self.client,
            proposal,
        }
    }
}

pub struct ContextProxyProposeRequest<'a, T> {
    client: CallClient<'a, T>,
    proposal: Proposal,
}

struct Propose {
    signer_id: String,
    proposal: Proposal,
}

impl Method<Propose> for Near {
    const METHOD: &'static str = "propose";

    type Returns = ();

    fn encode(params: &Propose) -> eyre::Result<Vec<u8>> {
        // sign the params, encode it and return
        todo!()
    }

    fn decode(response: &[u8]) -> eyre::Result<Self::Returns> {
        todo!()
    }
}

impl Method<Propose> for Starknet {
    type Returns = ();

    const METHOD: &'static str = "propose";

    fn encode(params: &Propose) -> eyre::Result<Vec<u8>> {
        todo!()
    }

    fn decode(response: &[u8]) -> eyre::Result<Self::Returns> {
        todo!()
    }
}

impl<'a, T: Transport> ContextProxyProposeRequest<'a, T> {
    pub async fn send(self, signing_key: [u8; 32]) -> Result<(), ConfigError<T>> {
        let request = Propose {
            signer_id: todo!(),
            proposal: self.proposal,
        };

        match self.client.protocol {
            Protocol::Near => self.client.mutate::<Near, _>(request).await?,
            Protocol::Starknet => self.client.mutate::<Starknet, _>(request).await?,
        }

        Ok(())
    }
}
