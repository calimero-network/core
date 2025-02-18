use calimero_primitives::hash::Hash;

use crate::entry::Borsh;
use crate::key::Alias as AliasKey;
use crate::types::PredefinedEntry;

impl PredefinedEntry for AliasKey {
    type Codec = Borsh;

    type DataType<'a> = Hash;
}
