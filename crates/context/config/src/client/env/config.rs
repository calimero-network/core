use crate::client::protocol::near::Near;
use crate::client::protocol::starknet::Starknet;
use crate::client::protocol::Method;
use crate::client::{CallClient, ConfigError, Environment, Protocol, Transport};
pub enum ContextConfig {}

pub struct ContextConfigQuery<'a, T> {
    client: CallClient<'a, T>,
}

pub struct ContextConfigMutate<'a, T> {
    client: CallClient<'a, T>,
}

impl<'a, T: 'a> Environment<'a, T> for ContextConfig {
    type Query = ContextConfigQuery<'a, T>;
    type Mutate = ContextConfigMutate<'a, T>;

    fn query(client: CallClient<'a, T>) -> Self::Query {
        todo!()
    }

    fn mutate(client: CallClient<'a, T>) -> Self::Mutate {
        todo!()
    }
}

struct Members {
    offset: usize,
    length: usize,
}

impl Method<Members> for Near {
    const METHOD: &'static str = "members";

    type Returns = Vec<String>;

    fn encode(params: &Members) -> eyre::Result<Vec<u8>> {
        todo!()
    }

    fn decode(response: &[u8]) -> eyre::Result<Self::Returns> {
        todo!()
    }
}

impl Method<Members> for Starknet {
    type Returns = Vec<String>;

    const METHOD: &'static str = "members";

    fn encode(params: &Members) -> eyre::Result<Vec<u8>> {
        todo!()
    }

    fn decode(response: &[u8]) -> eyre::Result<Self::Returns> {
        todo!()
    }
}

impl<'a, T: Transport> ContextConfigQuery<'a, T> {
    pub async fn members(
        &self,
        offset: usize,
        length: usize,
    ) -> Result<Vec<String>, ConfigError<T>> {
        let params = Members { offset, length };
        match self.client.protocol {
            Protocol::Near => self.client.query::<Near, _>(params).await,
            Protocol::Starknet => self.client.query::<Starknet, _>(params).await,
        }
    }
}

impl<'a, T: Transport> ContextConfigMutate<'a, T> {
    pub fn add_context(self, context_id: String) -> ContextConfigMutateRequest<'a, T> {
        ContextConfigMutateRequest {
            client: self.client,
            kind: RequestKind::AddContext { context_id },
        }
    }
}

enum RequestKind {
    AddContext { context_id: String },
}

pub struct ContextConfigMutateRequest<'a, T> {
    client: CallClient<'a, T>,
    kind: RequestKind,
}

struct Mutate {
    signer_id: String,
    nonce: u64,
    kind: RequestKind,
}

impl Method<Mutate> for Near {
    const METHOD: &'static str = "mutate";

    type Returns = ();

    fn encode(params: &Mutate) -> eyre::Result<Vec<u8>> {
        // sign the params, encode it and return
        todo!()
    }

    fn decode(response: &[u8]) -> eyre::Result<Self::Returns> {
        todo!()
    }
}

impl Method<Mutate> for Starknet {
    type Returns = ();

    const METHOD: &'static str = "mutate";

    fn encode(params: &Mutate) -> eyre::Result<Vec<u8>> {
        // sign the params, encode it and return
        // since you will have a `Vec<Felt>` here, you can
        // `Vec::with_capacity(32 * calldata.len())` and then
        // extend the `Vec` with each `Felt::to_bytes_le()`
        // when this `Vec<u8>` makes it to `StarknetTransport`,
        // reconstruct the `Vec<Felt>` from it
        todo!()
    }

    fn decode(response: &[u8]) -> eyre::Result<Self::Returns> {
        todo!()
    }
}

impl<'a, T: Transport> ContextConfigMutateRequest<'a, T> {
    pub async fn send(self, signing_key: [u8; 32]) -> Result<(), ConfigError<T>> {
        let request = Mutate {
            signer_id: todo!(),
            nonce: 0,
            kind: self.kind,
        };

        match self.client.protocol {
            Protocol::Near => self.client.mutate::<Near, _>(request).await?,
            Protocol::Starknet => self.client.mutate::<Starknet, _>(request).await?,
        }

        Ok(())
    }
}
