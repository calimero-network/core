use borsh::{BorshDeserialize, BorshSerialize};
use calimero_primitives::alias::Alias;

use crate::entry::Borsh;
use crate::key::{IdentityAlias as IdentityAliasKey, Kind};
use crate::types::PredefinedEntry;

#[derive(BorshDeserialize, BorshSerialize, Clone, Debug, Eq, PartialEq)]
pub struct IdentityAlias {
    kind: Kind,
    scope: [u8; 32],
    alias: Alias,
}

impl PredefinedEntry for IdentityAliasKey {
    type Codec = Borsh;

    type DataType<'a> = IdentityAlias;
}
