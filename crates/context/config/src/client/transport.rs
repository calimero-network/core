use core::error::Error;
use std::borrow::Cow;

use either::Either;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::protocol::Protocol;

pub trait ProtocolTransport {
    type Error: Error;

    #[expect(async_fn_in_trait, reason = "constraints are upheld for now")]
    async fn send(
        &self,
        request: TransportRequest<'_>,
        payload: Vec<u8>,
    ) -> Result<Vec<u8>, Self::Error>;
}

#[derive(Debug)]
pub struct TransportRequest<'a> {
    pub network_id: Cow<'a, str>,
    pub contract_id: Cow<'a, str>,
    pub operation: Operation<'a>,
}

#[derive(Debug, Serialize, Deserialize)]
#[expect(clippy::exhaustive_enums, reason = "Considered to be exhaustive")]
pub enum Operation<'a> {
    Read { method: Cow<'a, str> },
    Write { method: Cow<'a, str> },
}

pub trait Transport {
    type Error: Error;

    #[expect(async_fn_in_trait, reason = "constraints are upheld for now")]
    async fn try_send<'a>(
        &self,
        args: TransportArguments<'a>,
    ) -> Result<Result<Vec<u8>, Self::Error>, UnsupportedProtocol<'a>>;
}

#[derive(Debug)]
pub struct TransportArguments<'a> {
    pub protocol: Cow<'a, str>,
    pub request: TransportRequest<'a>,
    pub payload: Vec<u8>,
}

#[derive(Debug)]
pub struct UnsupportedProtocol<'a> {
    pub args: TransportArguments<'a>,
    pub expected: Cow<'static, [Cow<'static, str>]>,
}

#[derive(Debug, Error)]
pub enum EitherError<L, R> {
    #[error(transparent)]
    Left(L),
    #[error(transparent)]
    Right(R),
}

impl<L, R> Transport for Either<L, R>
where
    L: Transport,
    R: Transport,
{
    type Error = EitherError<L::Error, R::Error>;

    async fn try_send<'a>(
        &self,
        args: TransportArguments<'a>,
    ) -> Result<Result<Vec<u8>, Self::Error>, UnsupportedProtocol<'a>> {
        match self {
            Self::Left(left) => Ok(left.try_send(args).await?.map_err(EitherError::Left)),
            Self::Right(right) => Ok(right.try_send(args).await?.map_err(EitherError::Right)),
        }
    }
}

#[expect(clippy::exhaustive_structs, reason = "this is exhaustive")]
#[derive(Debug, Clone)]
pub struct Both<L, R> {
    pub left: L,
    pub right: R,
}

impl<L, R> Transport for Both<L, R>
where
    L: Transport,
    R: Transport,
{
    type Error = EitherError<L::Error, R::Error>;

    async fn try_send<'a>(
        &self,
        args: TransportArguments<'a>,
    ) -> Result<Result<Vec<u8>, Self::Error>, UnsupportedProtocol<'a>> {
        let left = self.left.try_send(args).await;

        let UnsupportedProtocol {
            args,
            expected: left_expected,
        } = match left {
            Ok(res) => return Ok(res.map_err(EitherError::Left)),
            Err(err) => err,
        };

        let right = self.right.try_send(args).await;

        let UnsupportedProtocol {
            args,
            expected: right_expected,
        } = match right {
            Ok(res) => return Ok(res.map_err(EitherError::Right)),
            Err(err) => err,
        };

        let mut expected = left_expected.into_owned();

        expected.extend(right_expected.into_owned());

        Err(UnsupportedProtocol {
            args,
            expected: expected.into(),
        })
    }
}

impl<T: AssociatedTransport> Transport for Option<T> {
    type Error = T::Error;

    async fn try_send<'a>(
        &self,
        args: TransportArguments<'a>,
    ) -> Result<Result<Vec<u8>, Self::Error>, UnsupportedProtocol<'a>> {
        let Some(inner) = self else {
            return Err(UnsupportedProtocol {
                args,
                expected: Cow::Borrowed(&[]),
            });
        };

        inner.try_send(args).await
    }
}

pub trait AssociatedTransport: ProtocolTransport {
    type Protocol: Protocol;

    #[inline]
    #[must_use]
    fn protocol() -> &'static str {
        Self::Protocol::PROTOCOL
    }
}

impl<T: AssociatedTransport> Transport for T {
    type Error = T::Error;

    async fn try_send<'a>(
        &self,
        args: TransportArguments<'a>,
    ) -> Result<Result<Vec<u8>, Self::Error>, UnsupportedProtocol<'a>> {
        let protocol = Self::protocol();

        if args.protocol != protocol {
            return Err(UnsupportedProtocol {
                args,
                expected: vec![protocol.into()].into(),
            });
        }

        Ok(self.send(args.request, args.payload).await)
    }
}
