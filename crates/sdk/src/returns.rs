use serde::Serialize;

mod private {
    pub trait Sealed {}
}

pub trait IntoResult: private::Sealed {
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
    pub fn to_json(&self) -> serde_json::Result<Result<Vec<u8>, Vec<u8>>> {
        Ok(match self {
            Self(Ok(ok)) => Ok(serde_json::to_vec(&ok)?),
            Self(Err(err)) => Err(serde_json::to_vec(&err)?),
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

impl<T> private::Sealed for WrappedReturn<T> {}
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
