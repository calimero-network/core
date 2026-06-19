use serde::Serialize;
use serde_json::{to_vec as to_json_vec, Result as JsonResult};

use crate::returns::private::Sealed;

mod private {
    pub trait Sealed {}
}

/// The bound an app method's return value (and its error type) must satisfy: it
/// is JSON-encoded for the caller, so it must be `serde::Serialize`.
///
/// This is an SDK-owned alias over `serde::Serialize` whose only job is to carry
/// a clear diagnostic — a non-serializable return otherwise fails deep inside
/// the return-encoding machinery (`WrappedReturn::…to_json()`), far from the
/// method. Blanket-implemented for every `Serialize` type, so it is exactly as
/// permissive as the bound it replaces.
#[diagnostic::on_unimplemented(
    message = "(calimero)> `{Self}` can't be returned from an app method — it is not JSON-serializable",
    label = "not serializable",
    note = "a method's return value (and error type) is encoded to JSON for the caller. Derive \
            `serde::Serialize` (with `#[serde(crate = \"calimero_sdk::serde\")]`), or return a type \
            that already implements it."
)]
pub trait AppReturn: Serialize {}

impl<T: Serialize + ?Sized> AppReturn for T {}

pub trait IntoResult: Sealed {
    type Ok;
    type Err;

    fn into_result(self) -> ReturnsResult<Self::Ok, Self::Err>;
}

#[derive(Debug)]
pub struct ReturnsResult<T, E>(Result<T, E>);

impl<T, E> ReturnsResult<T, E>
where
    T: AppReturn,
    E: AppReturn,
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
    T: AppReturn,
{
    type Ok = T;
    type Err = Infallible;

    #[inline]
    fn into_result(self) -> ReturnsResult<Self::Ok, Self::Err> {
        let Self(value) = self;
        ReturnsResult(Ok(value))
    }
}
