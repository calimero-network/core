#![cfg(feature = "client")]

use std::borrow::Cow;
use std::fmt::Debug;

use eyre::Context;
use thiserror::Error;

pub mod config;
pub mod env;
mod macros;
pub mod method;

pub mod relayer;
pub mod transport;


use config::ClientConfig;

use macros::transport;

use transport::{Transport, TransportArguments, TransportRequest, UnsupportedProtocol};

transport! {
    pub type LocalTransports = ();
}

transport! {
    pub type AnyTransport = (
        LocalTransports,
        relayer::RelayerTransport
    );
}

#[derive(Clone, Debug)]
pub struct Client<T> {
    transport: T,
}

impl<T: Transport> Client<T> {
    pub const fn new(transport: T) -> Self {
        Self { transport }
    }
}

impl Client<AnyTransport> {
    #[must_use]
    pub fn from_config(config: &ClientConfig) -> Self {
        let relayer = relayer::RelayerTransport::new(&relayer::RelayerConfig {
            url: config.signer.relayer.url.clone(),
        });

        let local = Self::from_local_config(&config).expect("validation error");

        let transport = transport!(local.transport, relayer);

        Self::new(transport)
    }

    pub fn from_local_config(_config: &ClientConfig) -> eyre::Result<Client<LocalTransports>> {
        Ok(Client::new(()))
    }
}

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ClientError<T: Transport> {
    #[error("transport error: {0}")]
    Transport(T::Error),
    #[error("codec error: {0}")]
    Codec(#[from] eyre::Report),
    #[error(
        "unsupported protocol: `{found}`, expected {}",
        expected.iter().map(|s| s.as_ref()).collect::<Vec<_>>().join(", ")
    )]
    UnsupportedProtocol {
        found: String,
        expected: Cow<'static, [Cow<'static, str>]>,
    },
}

impl<'a, T: Transport> From<UnsupportedProtocol<'a>> for ClientError<T> {
    fn from(err: UnsupportedProtocol<'a>) -> Self {
        Self::UnsupportedProtocol {
            found: err.args.protocol.into_owned(),
            expected: err.expected,
        }
    }
}

impl<T: Transport> Client<T> {
    async fn send(
        &self,
        protocol: Cow<'_, str>,
        request: TransportRequest<'_>,
        payload: Vec<u8>,
    ) -> Result<Vec<u8>, ClientError<T>> {
        let res: Result<_, _> = self
            .transport
            .try_send(TransportArguments {
                protocol,
                request,
                payload,
            })
            .await
            .into();

        res?.map_err(ClientError::Transport)
    }

    pub fn query<'a, E: Environment<'a, T>>(
        &'a self,
        protocol: Cow<'a, str>,
        network_id: Cow<'a, str>,
        contract_id: Cow<'a, str>,
    ) -> E::Query {
        E::query(CallClient {
            protocol,
            network_id,
            contract_id,
            client: self,
        })
    }

    pub fn mutate<'a, E: Environment<'a, T>>(
        &'a self,
        protocol: Cow<'a, str>,
        network_id: Cow<'a, str>,
        contract_id: Cow<'a, str>,
    ) -> E::Mutate {
        E::mutate(CallClient {
            protocol,
            network_id,
            contract_id,
            client: self,
        })
    }
}

impl<T: Transport> Transport for Client<T> {
    type Error = T::Error;

    async fn try_send<'a>(
        &self,
        args: TransportArguments<'a>,
    ) -> Result<Result<Vec<u8>, Self::Error>, UnsupportedProtocol<'a>> {
        self.transport.try_send(args).await
    }
}

#[derive(Debug)]
pub struct CallClient<'a, T> {
    protocol: Cow<'a, str>,
    network_id: Cow<'a, str>,
    contract_id: Cow<'a, str>,
    client: &'a Client<T>,
}

#[derive(Debug)]
pub enum Operation<M> {
    Read(M),
    Write(M),
}

impl<'a, T: Transport> CallClient<'a, T> {
    async fn send<P, M: Method<P>>(
        &self,
        params: Operation<M>,
    ) -> Result<M::Returns, ClientError<T>>
    where
        P: Protocol,
    {
        let method = Cow::Borrowed(M::METHOD);

        let (operation, payload) = match params {
            Operation::Read(params) => (transport::Operation::Read { method }, params.encode()?),
            Operation::Write(params) => (transport::Operation::Write { method }, params.encode()?),
        };

        let request = TransportRequest {
            network_id: Cow::Borrowed(&self.network_id),
            contract_id: Cow::Borrowed(&self.contract_id),
            operation,
        };

        let response = self
            .client
            .send(self.protocol.as_ref().into(), request, payload)
            .await?;

        M::decode(response).map_err(ClientError::Codec)
    }
}

pub trait Environment<'a, T> {
    type Query;
    type Mutate;

    fn query(client: CallClient<'a, T>) -> Self::Query;
    fn mutate(client: CallClient<'a, T>) -> Self::Mutate;
}
