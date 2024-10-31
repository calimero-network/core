use crate::client::protocol::near::Near;
use crate::client::protocol::starknet::Starknet;
use crate::client::protocol::Method;
use crate::client::{CallClient, Environment, Protocol, Transport};

enum ContextProxy {}

struct ContextProxyQuery<'a, T> {
    client: CallClient<'a, T>,
}

struct ContextProxyMutate<'a, T> {
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

impl<'a, T> ContextProxyQuery<'a, T> {
    async fn proposals(&self, offset: usize, length: usize) -> Result<Vec<String>, Error> {
        todo!()
    }
}

enum Proposal {
    Transfer { recipient: String, amount: u64 },
}

impl<'a, T> ContextProxyMutate<'a, T> {
    fn propose(self, proposal: Proposal) -> ContextProxyProposeRequest<'a, T> {
        ContextProxyProposeRequest {
            client: self.client,
            proposal,
        }
    }
}

struct ContextProxyProposeRequest<'a, T> {
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

    fn encode(params: &Propose) -> Result<Vec<u8>, Error> {
        // sign the params, encode it and return
        todo!()
    }

    fn decode(response: &[u8]) -> Result<Self::Returns, Error> {
        todo!()
    }
}

impl Method<Propose> for Starknet {
    type Returns = ();

    const METHOD: &'static str = "propose";

    fn encode(params: &Propose) -> Result<Vec<u8>, Error> {
        todo!()
    }

    fn decode(response: &[u8]) -> Result<Self::Returns, Error> {
        todo!()
    }
}

impl<'a, T: Transport> ContextProxyProposeRequest<'a, T> {
    async fn send(self, signing_key: [u8; 32]) -> Result<(), Error> {
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
