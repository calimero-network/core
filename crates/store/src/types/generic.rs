use std::convert::Infallible;

use crate::entry::DataType;
use crate::key;
use crate::slice::Slice;
use crate::types::PredefinedEntry;

#[derive(Eq, Clone, Debug, PartialEq)]
pub struct GenericData<'a> {
    value: Slice<'a>,
}

impl<'a> DataType<'a> for GenericData<'a> {
    type Error = Infallible;

    fn from_slice(slice: Slice<'a>) -> Result<Self, Self::Error> {
        Ok(Self { value: slice })
    }

    fn as_slice(&'a self) -> Result<Slice<'a>, Self::Error> {
        Ok(self.value.as_ref().into())
    }
}

impl PredefinedEntry for key::Generic {
    type DataType<'a> = GenericData<'a>;
}
