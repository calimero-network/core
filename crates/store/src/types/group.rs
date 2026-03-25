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

impl PredefinedEntry for key::GroupSigningKey {
    type Codec = Borsh;
    type DataType<'a> = key::GroupSigningKeyValue;
}

impl PredefinedEntry for key::GroupMemberCapability {
    type Codec = Borsh;
    type DataType<'a> = key::GroupMemberCapabilityValue;
}

impl PredefinedEntry for key::GroupContextVisibility {
    type Codec = Borsh;
    type DataType<'a> = key::GroupContextVisibilityValue;
}

impl PredefinedEntry for key::GroupContextAllowlist {
    type Codec = Borsh;
    type DataType<'a> = ();
}

impl PredefinedEntry for key::GroupDefaultCaps {
    type Codec = Borsh;
    type DataType<'a> = key::GroupDefaultCapsValue;
}

impl PredefinedEntry for key::GroupDefaultVis {
    type Codec = Borsh;
    type DataType<'a> = key::GroupDefaultVisValue;
}

impl PredefinedEntry for key::GroupContextLastMigration {
    type Codec = Borsh;
    type DataType<'a> = key::GroupContextLastMigrationValue;
}

impl PredefinedEntry for key::GroupLocalGovNonce {
    type Codec = Borsh;
    type DataType<'a> = u64;
}

impl PredefinedEntry for key::GroupContextAlias {
    type Codec = Borsh;
    type DataType<'a> = String;
}

impl PredefinedEntry for key::GroupMemberAlias {
    type Codec = Borsh;
    type DataType<'a> = String;
}

impl PredefinedEntry for key::GroupAlias {
    type Codec = Borsh;
    type DataType<'a> = String;
}

impl PredefinedEntry for key::GroupOpLog {
    type Codec = Borsh;
    type DataType<'a> = Vec<u8>;
}

impl PredefinedEntry for key::GroupMemberContext {
    type Codec = Borsh;
    type DataType<'a> = [u8; 32];
}

impl PredefinedEntry for key::GroupOpHead {
    type Codec = Borsh;
    type DataType<'a> = key::GroupOpHeadValue;
}
