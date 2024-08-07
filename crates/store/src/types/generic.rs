use crate::entry::Identity;
use crate::key;
use crate::slice::Slice;
use crate::types::PredefinedEntry;

#[derive(Eq, Clone, Debug, PartialEq)]
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

impl<'a> AsRef<[u8]> for GenericData<'a> {
    fn as_ref(&self) -> &[u8] {
        self.value.as_ref()
    }
}
