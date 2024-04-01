use serde::Serialize;

mod private {
    pub trait Sealed {}
}

pub trait IntoResult: private::Sealed {
    type Ok;
    type Err;

    fn into_result(self) -> ReturnsResult<Self::Ok, Self::Err>;
}

pub struct ReturnsResult<T, E>(Result<T, E>);

impl<T, E> ReturnsResult<T, E>
where
    T: Serialize,
    E: Serialize,
{
    pub fn to_json(self) -> serde_json::Result<Result<Vec<u8>, Vec<u8>>> {
        Ok(match self {
            ReturnsResult(Ok(ok)) => Ok(serde_json::to_vec(&ok)?),
            ReturnsResult(Err(err)) => Err(serde_json::to_vec(&err)?),
        })
    }
}

pub struct WrappedReturn<T>(T);

impl<T> WrappedReturn<T> {
    pub fn new(value: T) -> Self {
        WrappedReturn(value)
    }
}

impl<T, E> WrappedReturn<Result<T, E>> {
    pub fn into_result(self) -> ReturnsResult<T, E> {
        let WrappedReturn(value) = self;
        ReturnsResult(value)
    }
}

#[derive(Serialize)]
pub enum Infallible {}

impl<T> private::Sealed for WrappedReturn<T> {}
impl<T> IntoResult for WrappedReturn<T>
where
    T: Serialize,
{
    type Ok = T;
    type Err = Infallible;

    fn into_result(self) -> ReturnsResult<Self::Ok, Self::Err> {
        let WrappedReturn(value) = self;
        ReturnsResult(Ok(value))
    }
}
