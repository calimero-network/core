#![allow(single_use_lifetimes, reason = "borsh shenanigans")]

use crate::entry::Borsh;
use crate::key;
use crate::types::PredefinedEntry;

impl PredefinedEntry for key::GroupMeta {
    type Codec = Borsh;
    type DataType<'a> = key::GroupMetaValue;
}

impl PredefinedEntry for key::GroupMember {
    type Codec = Borsh;
    type DataType<'a> = key::GroupMemberValue;
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

impl PredefinedEntry for key::GroupContextMemberCap {
    type Codec = Borsh;
    type DataType<'a> = u8;
}

impl PredefinedEntry for key::GroupOpHead {
    type Codec = Borsh;
    type DataType<'a> = key::GroupOpHeadValue;
}

impl PredefinedEntry for key::GroupParentRef {
    type Codec = Borsh;
    type DataType<'a> = [u8; 32];
}

impl PredefinedEntry for key::GroupChildIndex {
    type Codec = Borsh;
    type DataType<'a> = ();
}

impl PredefinedEntry for key::NamespaceIdentity {
    type Codec = Borsh;
    type DataType<'a> = key::NamespaceIdentityValue;
}
