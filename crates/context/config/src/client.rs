use core::convert::Infallible;
use core::error::Error as CoreError;
use core::marker::PhantomData;
use core::ptr;
use std::borrow::Cow;
use std::collections::BTreeMap;

use ed25519_dalek::Signature;
use either::Either;
use serde::{Deserialize, Serialize};
use serde_json::{json, Error as JsonError};
use thiserror::Error;

use crate::repr::Repr;
use crate::types::{
    self, Application, Capability, ContextId, ContextIdentity, Proposal, Signed, SignerId,
};
use crate::{ContextRequest, ContextRequestKind, Request, RequestKind};

pub mod config;
pub mod near;
pub mod relayer;

use config::{ContextConfigClientConfig, ContextConfigClientSelectedSigner};

pub trait Transport {
    type Error: CoreError;

    #[expect(async_fn_in_trait, reason = "Should be fine")]
    async fn send(
        &self,
        request: TransportRequest<'_>,
        payload: Vec<u8>,
    ) -> Result<Vec<u8>, Self::Error>;
}

impl<L: Transport, R: Transport> Transport for Either<L, R> {
    type Error = Either<L::Error, R::Error>;

    async fn send(
        &self,
        request: TransportRequest<'_>,
        payload: Vec<u8>,
    ) -> Result<Vec<u8>, Self::Error> {
        match self {
            Self::Left(left) => left.send(request, payload).await.map_err(Either::Left),
            Self::Right(right) => right.send(request, payload).await.map_err(Either::Right),
        }
    }
}

#[derive(Debug)]
#[non_exhaustive]
pub struct TransportRequest<'a> {
    pub network_id: Cow<'a, str>,
    pub contract_id: Cow<'a, str>,
    pub operation: Operation<'a>,
}

impl<'a> TransportRequest<'a> {
    #[must_use]
    pub const fn new(
        network_id: Cow<'a, str>,
        contract_id: Cow<'a, str>,
        operation: Operation<'a>,
    ) -> Self {
        Self {
            network_id,
            contract_id,
            operation,
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[expect(clippy::exhaustive_enums, reason = "Considered to be exhaustive")]
pub enum Operation<'a> {
    Read { method: Cow<'a, str> },
    Write { method: Cow<'a, str> },
}

#[derive(Clone, Debug)]
pub struct ContextConfigClient<T> {
    transport: T,
}

impl<T: Transport> ContextConfigClient<T> {
    pub const fn new(transport: T) -> Self {
        Self { transport }
    }
}

pub type RelayOrNearTransport = Either<relayer::RelayerTransport, near::NearTransport<'static>>;

#[derive(Clone, Debug)]
pub struct ContextProxyClient<T> {
    transport: T,
}

impl<T: Transport> ContextProxyClient<T> {
    pub const fn new(transport: T) -> Self {
        Self { transport }
    }
}

pub trait FromConfig: Sized {
    fn from_config(config: &ContextConfigClientConfig) -> Self;
}

pub trait NewFromTransport<T: Transport> {
    fn new_from_transport(transport: T) -> Self;
}

impl<T: Transport> NewFromTransport<T> for ContextConfigClient<T> {
    fn new_from_transport(transport: T) -> Self {
        Self::new(transport)
    }
}

impl<T: Transport> NewFromTransport<T> for ContextProxyClient<T> {
    fn new_from_transport(transport: T) -> Self {
        Self::new(transport)
    }
}

impl<C> FromConfig for C
where
    C: NewFromTransport<RelayOrNearTransport>,
    RelayOrNearTransport: Transport,
{
    fn from_config(config: &ContextConfigClientConfig) -> Self {
        let transport = match config.signer.selected {
            ContextConfigClientSelectedSigner::Relayer => {
                Either::Left(relayer::RelayerTransport::new(&relayer::RelayerConfig {
                    url: config.signer.relayer.url.clone(),
                }))
            }
            ContextConfigClientSelectedSigner::Local => {
                Either::Right(near::NearTransport::new(&near::NearConfig {
                    networks: config
                        .signer
                        .local
                        .iter()
                        .map(|(network, config)| {
                            (
                                network.clone().into(),
                                near::NetworkConfig {
                                    rpc_url: config.rpc_url.clone(),
                                    account_id: config.credentials.account_id.clone(),
                                    access_key: config.credentials.secret_key.clone(),
                                },
                            )
                        })
                        .collect(),
                }))
            }
        };
        Self::new_from_transport(transport)
    }
}

pub trait Environment {
    type Query<'a>
    where
        Self: 'a;
    type Mutate<'a>
    where
        Self: 'a;

    fn query<'a>(&'a self, network_id: Cow<'a, str>, contract_id: Cow<'a, str>) -> Self::Query<'a>;

    fn mutate<'a>(
        &'a self,
        network_id: Cow<'a, str>,
        contract_id: Cow<'a, str>,
        signer_id: SignerId,
    ) -> Self::Mutate<'a>;
}

#[derive(Debug)]
pub struct ContextConfigQuery<'a, T> {
    client: QueryClient<'a, T>,
}

#[derive(Debug)]
pub struct ContextConfigMutate<'a, T> {
    client: MutateClient<'a, T>,
}

impl<T: Transport> Environment for ContextConfigClient<T>
where
    T: 'static,
{
    type Query<'a> = ContextConfigQuery<'a, T>;
    type Mutate<'a> = ContextConfigMutate<'a, T>;

    fn query<'a>(&'a self, network_id: Cow<'a, str>, contract_id: Cow<'a, str>) -> Self::Query<'a> {
        let client = QueryClient {
            network_id,
            contract_id,
            transport: &self.transport,
        };
        ContextConfigQuery { client }
    }

    fn mutate<'a>(
        &'a self,
        network_id: Cow<'a, str>,
        contract_id: Cow<'a, str>,
        signer_id: SignerId,
    ) -> Self::Mutate<'a> {
        let client = MutateClient {
            network_id,
            contract_id,
            signer_id,
            transport: &self.transport,
        };

        ContextConfigMutate { client }
    }
}

#[derive(Debug)]
pub struct ContextProxyQuery<'a, T> {
    client: QueryClient<'a, T>,
}

#[allow(dead_code)]
#[derive(Debug)]
pub struct ContextProxyMutate<'a, T> {
    client: MutateClient<'a, T>,
}

impl<T: Transport> Environment for ContextProxyClient<T>
where
    T: 'static,
{
    type Query<'a> = ContextProxyQuery<'a, T>;
    type Mutate<'a> = ContextProxyMutate<'a, T>;

    fn query<'a>(&'a self, network_id: Cow<'a, str>, contract_id: Cow<'a, str>) -> Self::Query<'a> {
        let client = QueryClient {
            network_id,
            contract_id,
            transport: &self.transport,
        };
        ContextProxyQuery { client }
    }

    fn mutate<'a>(
        &'a self,
        network_id: Cow<'a, str>,
        contract_id: Cow<'a, str>,
        signer_id: SignerId,
    ) -> Self::Mutate<'a> {
        let client = MutateClient {
            network_id,
            contract_id,
            signer_id,
            transport: &self.transport,
        };

        ContextProxyMutate { client }
    }
}

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ConfigError<T: Transport> {
    #[error("transport error: {0}")]
    Transport(T::Error),
    #[error(transparent)]
    Other(#[from] types::ConfigError<Infallible>),
}

#[derive(Debug)]
pub struct Response<T> {
    bytes: Vec<u8>,
    _priv: PhantomData<T>,
}

impl<T> Response<T> {
    const fn new(bytes: Vec<u8>) -> Self {
        Self {
            bytes,
            _priv: PhantomData,
        }
    }

    pub fn parse<'a>(&'a self) -> Result<T, JsonError>
    where
        T: Deserialize<'a>,
    {
        serde_json::from_slice(&self.bytes)
    }
}

#[derive(Debug, Clone)]
pub struct QueryClient<'a, T> {
    network_id: Cow<'a, str>,
    contract_id: Cow<'a, str>,
    transport: &'a T,
}

impl<'a, T: Transport> QueryClient<'a, T> {
    async fn read<I: Serialize, O>(
        &self,
        method: &str,
        body: I,
    ) -> Result<Response<O>, ConfigError<T>> {
        let payload = serde_json::to_vec(&body).map_err(|err| ConfigError::Other(err.into()))?;

        let request = TransportRequest {
            network_id: Cow::Borrowed(&self.network_id),
            contract_id: Cow::Borrowed(&self.contract_id),
            operation: Operation::Read {
                method: Cow::Borrowed(method),
            },
        };

        let response = self
            .transport
            .send(request, payload)
            .await
            .map_err(ConfigError::Transport)?;

        Ok(Response::new(response))
    }
}

impl<'a, T: Transport> ContextConfigQuery<'a, T> {
    pub async fn application(
        &self,
        context_id: ContextId,
    ) -> Result<Response<Application<'static>>, ConfigError<T>> {
        self.client
            .read(
                "application",
                json!({
                    "context_id": Repr::new(context_id),
                }),
            )
            .await
    }

    pub async fn members(
        &self,
        context_id: ContextId,
        offset: usize,
        length: usize,
    ) -> Result<Response<Vec<Repr<ContextIdentity>>>, ConfigError<T>> {
        self.client
            .read(
                "members",
                json!({
                    "context_id": Repr::new(context_id),
                    "offset": offset,
                    "length": length,
                }),
            )
            .await
    }

    pub async fn privileges(
        &self,
        context_id: ContextId,
        identities: &[ContextIdentity],
    ) -> Result<Response<BTreeMap<Repr<SignerId>, Vec<Capability>>>, ConfigError<T>> {
        let identities = unsafe {
            &*(ptr::from_ref::<[ContextIdentity]>(identities) as *const [Repr<ContextIdentity>])
        };

        self.client
            .read(
                "privileges",
                json!({
                    "context_id": Repr::new(context_id),
                    "identities": identities,
                }),
            )
            .await
    }
}

#[derive(Debug)]
pub struct MutateClient<'a, T> {
    network_id: Cow<'a, str>,
    contract_id: Cow<'a, str>,
    signer_id: SignerId,
    transport: &'a T,
}

#[derive(Debug)]
pub struct ClientRequest<'a, 'b, T> {
    client: &'a MutateClient<'a, T>,
    kind: RequestKind<'b>,
}

impl<T: Transport> ClientRequest<'_, '_, T> {
    pub async fn send<F: FnOnce(&[u8]) -> Signature>(self, sign: F) -> Result<(), ConfigError<T>> {
        let signed = Signed::new(&Request::new(self.client.signer_id, self.kind), sign)?;

        let request = TransportRequest {
            network_id: Cow::Borrowed(&self.client.network_id),
            contract_id: Cow::Borrowed(&self.client.contract_id),
            operation: Operation::Write {
                method: Cow::Borrowed("mutate"),
            },
        };

        let payload = serde_json::to_vec(&signed).map_err(|err| ConfigError::Other(err.into()))?;

        let _unused = self
            .client
            .transport
            .send(request, payload)
            .await
            .map_err(ConfigError::Transport)?;

        Ok(())
    }
}

impl<T: Transport> ContextConfigMutate<'_, T> {
    #[must_use]
    pub const fn add_context<'a>(
        &self,
        context_id: ContextId,
        author_id: ContextIdentity,
        application: Application<'a>,
    ) -> ClientRequest<'_, 'a, T> {
        let kind = RequestKind::Context(ContextRequest {
            context_id: Repr::new(context_id),
            kind: ContextRequestKind::Add {
                author_id: Repr::new(author_id),
                application,
            },
        });

        ClientRequest {
            client: &self.client,
            kind,
        }
    }

    #[must_use]
    pub const fn update_application<'a>(
        &self,
        context_id: ContextId,
        application: Application<'a>,
    ) -> ClientRequest<'_, 'a, T> {
        let kind = RequestKind::Context(ContextRequest {
            context_id: Repr::new(context_id),
            kind: ContextRequestKind::UpdateApplication { application },
        });

        ClientRequest {
            client: &self.client,
            kind,
        }
    }

    #[must_use]
    pub const fn add_members(
        &self,
        context_id: ContextId,
        members: &[ContextIdentity],
    ) -> ClientRequest<'_, 'static, T> {
        let members = unsafe {
            &*(ptr::from_ref::<[ContextIdentity]>(members) as *const [Repr<ContextIdentity>])
        };

        let kind = RequestKind::Context(ContextRequest {
            context_id: Repr::new(context_id),
            kind: ContextRequestKind::AddMembers {
                members: Cow::Borrowed(members),
            },
        });

        ClientRequest {
            client: &self.client,
            kind,
        }
    }

    #[must_use]
    pub const fn remove_members(
        &self,
        context_id: ContextId,
        members: &[ContextIdentity],
    ) -> ClientRequest<'_, 'static, T> {
        let members = unsafe {
            &*(ptr::from_ref::<[ContextIdentity]>(members) as *const [Repr<ContextIdentity>])
        };

        let kind = RequestKind::Context(ContextRequest {
            context_id: Repr::new(context_id),
            kind: ContextRequestKind::RemoveMembers {
                members: Cow::Borrowed(members),
            },
        });

        ClientRequest {
            client: &self.client,
            kind,
        }
    }

    #[must_use]
    pub const fn grant(
        &self,
        context_id: ContextId,
        capabilities: &[(ContextIdentity, Capability)],
    ) -> ClientRequest<'_, 'static, T> {
        let capabilities = unsafe {
            &*(ptr::from_ref::<[(ContextIdentity, Capability)]>(capabilities)
                as *const [(Repr<ContextIdentity>, Capability)])
        };

        let kind = RequestKind::Context(ContextRequest {
            context_id: Repr::new(context_id),
            kind: ContextRequestKind::Grant {
                capabilities: Cow::Borrowed(capabilities),
            },
        });

        ClientRequest {
            client: &self.client,
            kind,
        }
    }

    #[must_use]
    pub const fn revoke(
        &self,
        context_id: ContextId,
        capabilities: &[(ContextIdentity, Capability)],
    ) -> ClientRequest<'_, 'static, T> {
        let capabilities = unsafe {
            &*(ptr::from_ref::<[(ContextIdentity, Capability)]>(capabilities)
                as *const [(Repr<ContextIdentity>, Capability)])
        };

        let kind = RequestKind::Context(ContextRequest {
            context_id: Repr::new(context_id),
            kind: ContextRequestKind::Revoke {
                capabilities: Cow::Borrowed(capabilities),
            },
        });

        ClientRequest {
            client: &self.client,
            kind,
        }
    }
}

impl<'a, T: Transport> ContextProxyQuery<'a, T> {
    pub async fn get_requests(
        &self,
        offset: usize,
        length: usize,
    ) -> Result<Response<Vec<(&u32, &Proposal)>>, ConfigError<T>> {
        self.client
            .read("requests", json!({"offset": offset, "length": length}))
            .await
    }
}
