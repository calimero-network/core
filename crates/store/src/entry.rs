use crate::slice::Slice;

pub trait DataType: Sized {
    type Error;

    fn from_slice(slice: Slice) -> Result<Self, Self::Error>;
    fn as_slice(&self) -> Result<Slice, Self::Error>;
}
