//! External client for protocol communication

use std::borrow::Cow;
use std::fmt::Debug;
use std::ops::Deref;
use std::str::FromStr;

use thiserror::Error;

use crate::client::config::{ClientConfig, ClientSelectedSigner, Credentials};
use crate::client::transport::{Transport, TransportArguments, TransportRequest, UnsupportedProtocol};

#[derive(Clone, Debug)]
pub struct ExternalClient<T> {
    transport: T,
}

impl<T: Transport> ExternalClient<T> {
    pub const fn new(transport: T) -> Self {
        Self { transport }
    }
}

impl<T: Transport> Transport for ExternalClient<T> {
    type Error = T::Error;

    async fn try_send<'a>(
        &self,
        args: TransportArguments<'a>,
    ) -> Result<Result<Vec<u8>, Self::Error>, UnsupportedProtocol<'a>> {
        self.transport.try_send(args).await
    }
}

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ExternalClientError<T: Transport> {
    #[error("transport error: {0}")]
    Transport(T::Error),
    #[error("codec error: {0}")]
    Codec(#[from] eyre::Report),
    #[error(
        "unsupported protocol: `{found}`, expected {}",
        "protocols" // Simplified for now
    )]
    UnsupportedProtocol {
        found: String,
        expected: Cow<'static, [Cow<'static, str>]>,
    },
}

impl<'a, T: Transport> From<UnsupportedProtocol<'a>> for ExternalClientError<T> {
    fn from(err: UnsupportedProtocol<'a>) -> Self {
        Self::UnsupportedProtocol {
            found: err.args.protocol.into_owned(),
            expected: err.expected,
        }
    }
}
