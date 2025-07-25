use crate::entry::Identity;
use crate::key;
use crate::slice::Slice;
use crate::types::PredefinedEntry;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GenericData<'a> {
    value: Slice<'a>,
}

impl PredefinedEntry for key::Generic {
    type Codec = Identity;
    type DataType<'a> = GenericData<'a>;
}

impl<'a> From<Slice<'a>> for GenericData<'a> {
    fn from(value: Slice<'a>) -> Self {
        Self { value }
    }
}

impl AsRef<[u8]> for GenericData<'_> {
    fn as_ref(&self) -> &[u8] {
        self.value.as_ref()
    }
}
