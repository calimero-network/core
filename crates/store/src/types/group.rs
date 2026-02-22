#![allow(single_use_lifetimes, reason = "borsh shenanigans")]

use calimero_primitives::context::GroupMemberRole;

use crate::entry::Borsh;
use crate::key;
use crate::types::PredefinedEntry;

impl PredefinedEntry for key::GroupMeta {
    type Codec = Borsh;
    type DataType<'a> = key::GroupMetaValue;
}

impl PredefinedEntry for key::GroupMember {
    type Codec = Borsh;
    type DataType<'a> = GroupMemberRole;
}

impl PredefinedEntry for key::GroupContextIndex {
    type Codec = Borsh;
    type DataType<'a> = ();
}

impl PredefinedEntry for key::ContextGroupRef {
    type Codec = Borsh;
    type DataType<'a> = [u8; 32];
}

impl PredefinedEntry for key::GroupUpgradeKey {
    type Codec = Borsh;
    type DataType<'a> = key::GroupUpgradeValue;
}
