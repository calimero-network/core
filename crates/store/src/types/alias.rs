use calimero_primitives::hash::Hash;

use crate::entry::Borsh;
use crate::key;
use crate::types::PredefinedEntry;

impl PredefinedEntry for key::Alias {
    type Codec = Borsh;
    type DataType<'a> = Hash;
}
