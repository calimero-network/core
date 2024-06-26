use crate::key::AsKeyParts;
use crate::slice::Slice;

pub trait DataType<'a>: Sized {
    type Error;

    fn from_slice(slice: Slice<'a>) -> Result<Self, Self::Error>;
    fn as_slice(&'a self) -> Result<Slice<'a>, Self::Error>;
}

pub trait Entry {
    type Key: AsKeyParts;
    type DataType<'a>: DataType<'a>;

    fn key(&self) -> &Self::Key;
}
