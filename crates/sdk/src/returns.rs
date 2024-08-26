use serde::Serialize;
use serde_json::{to_vec as to_json_vec, Result as JsonResult};

use crate::returns::private::Sealed;

mod private {
    pub trait Sealed {}
}

pub trait IntoResult: Sealed {
    type Ok;
    type Err;

    fn into_result(self) -> ReturnsResult<Self::Ok, Self::Err>;
}

#[derive(Debug)]
pub struct ReturnsResult<T, E>(Result<T, E>);

impl<T, E> ReturnsResult<T, E>
where
    T: Serialize,
    E: Serialize,
{
    #[inline]
    pub fn to_json(&self) -> JsonResult<Result<Vec<u8>, Vec<u8>>> {
        Ok(match self {
            Self(Ok(ok)) => Ok(to_json_vec(&ok)?),
            Self(Err(err)) => Err(to_json_vec(&err)?),
        })
    }
}

#[derive(Debug)]
pub struct WrappedReturn<T>(T);

impl<T> WrappedReturn<T> {
    #[inline]
    pub const fn new(value: T) -> Self {
        Self(value)
    }
}

impl<T, E> WrappedReturn<Result<T, E>> {
    #[inline]
    pub fn into_result(self) -> ReturnsResult<T, E> {
        let Self(value) = self;
        ReturnsResult(value)
    }
}

#[derive(Clone, Copy, Debug, Serialize)]
pub enum Infallible {}

impl<T> Sealed for WrappedReturn<T> {}
impl<T> IntoResult for WrappedReturn<T>
where
    T: Serialize,
{
    type Ok = T;
    type Err = Infallible;

    #[inline]
    fn into_result(self) -> ReturnsResult<Self::Ok, Self::Err> {
        let Self(value) = self;
        ReturnsResult(Ok(value))
    }
}
