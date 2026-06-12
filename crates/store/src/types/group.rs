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

impl PredefinedEntry for key::GroupDeniedMember {
    type Codec = Borsh;
    type DataType<'a> = ();
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

impl PredefinedEntry for key::GroupSubgroupVis {
    type Codec = Borsh;
    type DataType<'a> = key::GroupSubgroupVisValue;
}

impl PredefinedEntry for key::GroupLocalGovNonce {
    type Codec = Borsh;
    type DataType<'a> = u64;
}

impl PredefinedEntry for key::GroupLocalGovNonceWindow {
    type Codec = Borsh;
    type DataType<'a> = key::GroupLocalGovNonceWindowValue;
}

impl PredefinedEntry for key::GroupContextMetadata {
    type Codec = Borsh;
    type DataType<'a> = calimero_primitives::metadata::MetadataRecord;
}

impl PredefinedEntry for key::GroupMemberMetadata {
    type Codec = Borsh;
    type DataType<'a> = calimero_primitives::metadata::MetadataRecord;
}

impl PredefinedEntry for key::GroupMetadata {
    type Codec = Borsh;
    type DataType<'a> = calimero_primitives::metadata::MetadataRecord;
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

impl PredefinedEntry for key::PendingSelfPurge {
    type Codec = Borsh;
    type DataType<'a> = ();
}

impl PredefinedEntry for key::ContextServiceName {
    type Codec = Borsh;
    type DataType<'a> = key::ContextServiceNameValue;
}

impl PredefinedEntry for key::NamespaceGovOp {
    type Codec = Borsh;
    type DataType<'a> = key::NamespaceGovOpValue;
}

impl PredefinedEntry for key::NamespaceGovHead {
    type Codec = Borsh;
    type DataType<'a> = key::NamespaceGovHeadValue;
}

impl PredefinedEntry for key::GroupKeyEntry {
    type Codec = Borsh;
    type DataType<'a> = key::GroupKeyValue;
}

// The buffered absorb record (`AbsorbRecord`, PR-6b) is defined in
// `calimero-governance-store`, which depends on this crate — so the value type
// cannot be named here without a dependency cycle. It is stored as an opaque
// borsh byte blob; the repository in `calimero-governance-store` owns the
// `AbsorbRecord` <-> bytes encode/decode.
impl PredefinedEntry for key::AbsorbBufferKey {
    type Codec = Borsh;
    type DataType<'a> = Vec<u8>;
}
