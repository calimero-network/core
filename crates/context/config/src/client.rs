use core::fmt;
use std::{borrow::Cow, collections::BTreeMap, mem};

use ed25519_dalek::Signature;
use serde::{Deserialize, Serialize};
use serde_json::json;
use thiserror::Error;

use crate::{
    repr::Repr,
    types::{self, Application, Capability, ContextId, ContextIdentity, Signed, SignerId},
    ContextRequest, ContextRequestKind, Request, RequestKind,
};

mod relayer;

#[derive(Debug, Serialize, Deserialize)]
pub enum Operation<'a> {
    Read { method: Cow<'a, str> },
    Write { method: Cow<'a, str> },
}

pub trait Transport {
    type Error: fmt::Display;

    #[allow(async_fn_in_trait)]
    async fn send(
        &self,
        operation: Operation<'_>,
        payload: Vec<u8>,
    ) -> Result<Vec<u8>, Self::Error>;
}

#[derive(Debug)]
pub struct ContextConfigClient<T> {
    transport: T,
}

impl<T: Transport> ContextConfigClient<T> {
    pub fn new(transport: T) -> Self {
        Self { transport }
    }
}

impl<T: Transport> ContextConfigClient<T> {
    pub fn query(&self) -> ContextConfigQueryClient<'_, T> {
        ContextConfigQueryClient {
            transport: &self.transport,
        }
    }

    pub fn mutate(&self, signer_id: SignerId) -> ContextConfigMutateClient<'_, T> {
        ContextConfigMutateClient {
            signer_id,
            transport: &self.transport,
        }
    }
}

#[derive(Debug, Error)]
pub enum Error<T: Transport> {
    #[error("transport error: {0}")]
    Transport(T::Error),
    #[error(transparent)]
    Other(#[from] types::Error<std::convert::Infallible>),
}

#[derive(Debug)]
pub struct Response<T> {
    bytes: Vec<u8>,
    _priv: std::marker::PhantomData<T>,
}

impl<T> Response<T> {
    fn new(bytes: Vec<u8>) -> Self {
        Self {
            bytes,
            _priv: Default::default(),
        }
    }

    pub fn parse(&self) -> Result<T, serde_json::Error>
    where
        T: for<'a> Deserialize<'a>,
    {
        serde_json::from_slice(&self.bytes)
    }
}

#[derive(Debug)]
pub struct ContextConfigQueryClient<'a, T> {
    transport: &'a T,
}

impl<'a, T: Transport> ContextConfigQueryClient<'a, T> {
    async fn read<I: Serialize, O>(&self, method: &str, body: I) -> Result<Response<O>, Error<T>> {
        let payload = serde_json::to_vec(&body).map_err(|err| Error::Other(err.into()))?;

        let operation = Operation::Read {
            method: Cow::Borrowed(method),
        };

        let response = self
            .transport
            .send(operation, payload)
            .await
            .map_err(Error::Transport)?;

        Ok(Response::new(response))
    }

    pub async fn application(
        &self,
        context_id: ContextId,
    ) -> Result<Response<Application<'static>>, Error<T>> {
        self.read(
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
    ) -> Result<Response<Vec<Repr<ContextIdentity>>>, Error<T>> {
        self.read(
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
    ) -> Result<Response<BTreeMap<Repr<SignerId>, Vec<Capability>>>, Error<T>> {
        let identities = unsafe { mem::transmute::<_, &[Repr<ContextIdentity>]>(identities) };

        self.read(
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
pub struct ClientRequest<'a, R, T> {
    request: R,
    transport: &'a T,
}

impl<'a, R: Serialize, T: Transport> ClientRequest<'a, R, T> {
    async fn send(&self, sign: impl FnOnce(&[u8]) -> Signature) -> Result<(), Error<T>> {
        let signed = Signed::new(&self.request, sign)?;

        let operation = Operation::Write {
            method: Cow::Borrowed("mutate"),
        };

        let payload = serde_json::to_vec(&signed).map_err(|err| Error::Other(err.into()))?;

        let _unused = self
            .transport
            .send(operation, payload)
            .await
            .map_err(Error::Transport)?;

        Ok(())
    }
}

#[derive(Debug)]
pub struct ContextConfigMutateClient<'a, T> {
    signer_id: SignerId,
    transport: &'a T,
}

impl<T: Transport> ContextConfigMutateClient<'_, T> {
    pub async fn add_context<'a>(
        &self,
        context_id: ContextId,
        author_id: ContextIdentity,
        application: Application<'a>,
    ) -> ClientRequest<'_, Request<'a>, T> {
        let request = Request::new(
            self.signer_id,
            RequestKind::Context(ContextRequest {
                context_id: Repr::new(context_id),
                kind: ContextRequestKind::Add {
                    author_id: Repr::new(author_id),
                    application,
                },
            }),
        );

        ClientRequest {
            request,
            transport: &self.transport,
        }
    }

    pub async fn update_application<'a>(
        &self,
        context_id: ContextId,
        application: Application<'a>,
    ) -> ClientRequest<'_, Request<'a>, T> {
        let request = Request::new(
            self.signer_id,
            RequestKind::Context(ContextRequest {
                context_id: Repr::new(context_id),
                kind: ContextRequestKind::UpdateApplication { application },
            }),
        );

        ClientRequest {
            request,
            transport: &self.transport,
        }
    }

    pub async fn add_members(
        &self,
        context_id: ContextId,
        members: &[ContextIdentity],
    ) -> ClientRequest<'_, Request<'static>, T> {
        let members = unsafe { mem::transmute(members) };

        let request = Request::new(
            self.signer_id,
            RequestKind::Context(ContextRequest {
                context_id: Repr::new(context_id),
                kind: ContextRequestKind::AddMembers {
                    members: Cow::Borrowed(members),
                },
            }),
        );

        ClientRequest {
            request,
            transport: &self.transport,
        }
    }

    pub async fn remove_members(
        &self,
        context_id: ContextId,
        members: &[ContextIdentity],
    ) -> ClientRequest<'_, Request<'static>, T> {
        let members = unsafe { mem::transmute(members) };

        let request = Request::new(
            self.signer_id,
            RequestKind::Context(ContextRequest {
                context_id: Repr::new(context_id),
                kind: ContextRequestKind::RemoveMembers {
                    members: Cow::Borrowed(members),
                },
            }),
        );

        ClientRequest {
            request,
            transport: &self.transport,
        }
    }

    pub async fn grant(
        &self,
        context_id: ContextId,
        capabilities: &[(ContextIdentity, Capability)],
    ) -> ClientRequest<'_, Request<'static>, T> {
        let capabilities = unsafe { mem::transmute(capabilities) };

        let request = Request::new(
            self.signer_id,
            RequestKind::Context(ContextRequest {
                context_id: Repr::new(context_id),
                kind: ContextRequestKind::Grant {
                    capabilities: Cow::Borrowed(capabilities),
                },
            }),
        );

        ClientRequest {
            request,
            transport: &self.transport,
        }
    }

    pub async fn revoke(
        &self,
        context_id: ContextId,
        capabilities: &[(ContextIdentity, Capability)],
    ) -> ClientRequest<'_, Request<'static>, T> {
        let capabilities = unsafe { mem::transmute(capabilities) };

        let request = Request::new(
            self.signer_id,
            RequestKind::Context(ContextRequest {
                context_id: Repr::new(context_id),
                kind: ContextRequestKind::Revoke {
                    capabilities: Cow::Borrowed(capabilities),
                },
            }),
        );

        ClientRequest {
            request,
            transport: &self.transport,
        }
    }
}
