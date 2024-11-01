use std::collections::BTreeMap;

use query::appilcation_revision::{ApplicationRevision, Revision};
use query::application::ApplicationRequest;
use query::members::Members;
use query::members_revision::MembersRevision;
use query::privileges::IdentitiyPrivileges;

mod query;

use crate::client::protocol::near::Near;
use crate::client::protocol::starknet::Starknet;
use crate::client::protocol::Method;
use crate::client::{CallClient, ConfigError, Environment, Protocol, Transport};
use crate::repr::Repr;
use crate::types::{Application, Capability, ContextIdentity, SignerId};
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
        match client.protocol {
            Protocol::Near => client.client.query::<ContextConfig>(
                Protocol::Near,
                client.network_id,
                client.contract_id,
            ),
            Protocol::Starknet => client.client.query::<ContextConfig>(
                Protocol::Starknet,
                client.network_id,
                client.contract_id,
            ),
        }
    }

    fn mutate(client: CallClient<'a, T>) -> Self::Mutate {
        match client.protocol {
            Protocol::Near => client.client.mutate::<ContextConfig>(
                Protocol::Near,
                client.network_id,
                client.contract_id,
            ),
            Protocol::Starknet => todo!(),
        }
    }
}

impl<'a, T: Transport> ContextConfigQuery<'a, T> {
    pub async fn members(
        &self,
        offset: usize,
        length: usize,
    ) -> Result<Vec<Repr<ContextIdentity>>, ConfigError<T>> {
        let params = Members { offset, length };
        match self.client.protocol {
            Protocol::Near => self.client.query::<Near, Members>(params).await,
            Protocol::Starknet => self.client.query::<Starknet, Members>(params).await,
        }
    }
    pub async fn application_revision(&self) -> Result<Revision, ConfigError<T>> {
        let params = ApplicationRevision {};
        match self.client.protocol {
            Protocol::Near => self.client.query::<Near, ApplicationRevision>(params).await,
            Protocol::Starknet => {
                self.client
                    .query::<Starknet, ApplicationRevision>(params)
                    .await
            }
        }
    }
    pub async fn application(&self) -> Result<Application<'static>, ConfigError<T>> {
        let params = ApplicationRequest {};
        match self.client.protocol {
            Protocol::Near => self.client.query::<Near, ApplicationRequest>(params).await,
            Protocol::Starknet => {
                self.client
                    .query::<Starknet, ApplicationRequest>(params)
                    .await
            }
        }
    }

    pub async fn members_revision(&self) -> Result<Revision, ConfigError<T>> {
        let params = MembersRevision {};
        match self.client.protocol {
            Protocol::Near => self.client.query::<Near, MembersRevision>(params).await,
            Protocol::Starknet => self.client.query::<Starknet, MembersRevision>(params).await,
        }
    }

    pub async fn privileges(
        &self,
        identities: &[ContextIdentity],
    ) -> Result<BTreeMap<Repr<SignerId>, Vec<Capability>>, ConfigError<T>> {
        let params = IdentitiyPrivileges { identities };
        match self.client.protocol {
            Protocol::Near => {
                self.client
                    .query::<Near, IdentitiyPrivileges<'_>>(params)
                    .await
            }
            Protocol::Starknet => {
                self.client
                    .query::<Starknet, IdentitiyPrivileges<'_>>(params)
                    .await
            }
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
