use crate::key::AsKeyParts;
use crate::slice::Slice;

pub trait DataType<'a>: Sized {
    type Error;

    // todo! change to &'a [u8]
    fn from_slice(slice: Slice<'a>) -> Result<Self, Self::Error>;
    fn as_slice(&'a self) -> Result<Slice<'a>, Self::Error>;
}

pub trait Entry {
    type Key: AsKeyParts;
    type DataType<'a>: DataType<'a>;

    fn key(&self) -> &Self::Key;

    // each entry should be able to define what
    // happens when it's operated on wrt storage
    // for example: to ref dec one of it's fields
    // when it's changed, for example
    // read old state, check if it's changed, decrement
    // the referent entry
}

#[cfg(feature = "serde")]
pub struct Json<T>(T);

#[cfg(feature = "serde")]
impl<T> Json<T> {
    pub fn new(value: T) -> Self {
        Self(value)
    }

    pub fn value(self) -> T {
        self.0
    }
}

#[cfg(feature = "serde")]
impl<'a, T> DataType<'a> for Json<T>
where
    T: serde::Serialize + serde::de::DeserializeOwned,
{
    type Error = serde_json::Error;

    fn from_slice(slice: Slice<'a>) -> Result<Self, Self::Error> {
        serde_json::from_slice(&slice).map(Json)
    }

    fn as_slice(&self) -> Result<Slice, Self::Error> {
        serde_json::to_vec(&self.0).map(Into::into)
    }
}
