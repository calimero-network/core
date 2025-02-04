use calimero_primitives::hash::Hash;

use crate::entry::Borsh;
use crate::key::IdentityAlias as IdentityAliasKey;
use crate::types::PredefinedEntry;

impl PredefinedEntry for IdentityAliasKey {
    type Codec = Borsh;

    type DataType<'a> = Hash;
}
