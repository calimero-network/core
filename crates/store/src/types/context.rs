use borsh::{BorshDeserialize, BorshSerialize};

use crate::entry::{Borsh, Identity};
use crate::key::{
    ApplicationMeta as ApplicationMetaKey, ContextConfig as ContextConfigKey,
    ContextIdentity as ContextIdentityKey, ContextMeta as ContextMetaKey,
    ContextState as ContextStateKey,
};
use crate::slice::Slice;
use crate::types::PredefinedEntry;

pub type Hash = [u8; 32];

#[derive(BorshDeserialize, BorshSerialize, Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct ContextMeta {
    pub application: ApplicationMetaKey,
    pub root_hash: Hash,
    // pub wire_version: usize,
}

impl ContextMeta {
    #[must_use]
    pub const fn new(
        application: ApplicationMetaKey,
        root_hash: Hash,
        // wire_version: usize,
    ) -> Self {
        Self {
            application,
            root_hash,
            // wire_version,
        }
    }
}

impl PredefinedEntry for ContextMetaKey {
    type Codec = Borsh;
    type DataType<'a> = ContextMeta;
}

#[derive(BorshDeserialize, BorshSerialize, Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct ContextConfig {
    pub network: Box<str>,
    pub contract: Box<str>,
}

impl ContextConfig {
    #[must_use]
    pub const fn new(network: Box<str>, contract: Box<str>) -> Self {
        Self { network, contract }
    }
}

impl PredefinedEntry for ContextConfigKey {
    type Codec = Borsh;
    type DataType<'a> = ContextConfig;
}

#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct ContextState<'a> {
    pub value: Slice<'a>,
}

impl PredefinedEntry for ContextStateKey {
    type Codec = Identity;
    type DataType<'a> = ContextState<'a>;
}

impl<'a> From<Slice<'a>> for ContextState<'a> {
    fn from(value: Slice<'a>) -> Self {
        Self { value }
    }
}

impl AsRef<[u8]> for ContextState<'_> {
    fn as_ref(&self) -> &[u8] {
        self.value.as_ref()
    }
}

#[derive(BorshDeserialize, BorshSerialize, Clone, Copy, Debug, Eq, PartialEq)]
#[expect(
    clippy::exhaustive_structs,
    reason = "This is not expected to have additional fields"
)]
pub struct ContextIdentity {
    pub private_key: Option<[u8; 32]>,
}

impl PredefinedEntry for ContextIdentityKey {
    type Codec = Borsh;
    type DataType<'a> = ContextIdentity;
}
