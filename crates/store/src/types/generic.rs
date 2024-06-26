use std::convert::Infallible;

use crate::entry::DataType;
use crate::key;
use crate::slice::Slice;
use crate::types::PredefinedEntry;

#[derive(Eq, Clone, Debug, PartialEq)]
pub struct GenericData(Box<[u8]>);

impl DataType for GenericData {
    type Error = Infallible;

    fn from_slice(slice: Slice) -> Result<Self, Self::Error> {
        Ok(Self(slice.into_boxed()))
    }

    fn as_slice(&self) -> Result<Slice, Self::Error> {
        Ok(self.0.as_ref().into())
    }
}

impl PredefinedEntry for key::Generic {
    type DataType = GenericData;
}
