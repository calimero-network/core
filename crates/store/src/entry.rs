use crate::key::AsKeyParts;
use crate::slice::Slice;

pub trait DataType: Sized {
    type Error;

    fn from_slice(slice: Slice) -> Result<Self, Self::Error>;
    fn as_slice(&self) -> Result<Slice, Self::Error>;
}

pub trait Entry {
    type Key: AsKeyParts;
    type DataType: DataType;

    fn key(&self) -> &Self::Key;
}
