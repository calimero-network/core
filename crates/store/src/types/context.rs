use borsh::{BorshDeserialize, BorshSerialize};

use crate::entry::{Borsh, Identity};
use crate::key::{
    ApplicationMeta as ApplicationMetaKey, ContextConfig as ContextConfigKey,
    ContextIdentity as ContextIdentityKey, ContextMeta as ContextMetaKey,
    ContextState as ContextStateKey, ContextTransaction as ContextTransactionKey,
};
use crate::slice::Slice;
use crate::types::PredefinedEntry;

pub type Hash = [u8; 32];

#[derive(BorshDeserialize, BorshSerialize, Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct ContextMeta {
    pub application: ApplicationMetaKey,
    pub root_hash: Hash,
}

impl ContextMeta {
    #[must_use]
    pub const fn new(application: ApplicationMetaKey, root_hash: Hash) -> Self {
        Self {
            application,
            root_hash,
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

#[derive(BorshDeserialize, BorshSerialize, Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct ContextTransaction {
    pub method: Box<str>,
    pub payload: Box<[u8]>,
    pub prior_hash: Hash,
    pub executor_public_key: [u8; 32],
}

impl ContextTransaction {
    #[must_use]
    pub const fn new(
        method: Box<str>,
        payload: Box<[u8]>,
        prior_hash: Hash,
        executor_public_key: [u8; 32],
    ) -> Self {
        Self {
            method,
            payload,
            prior_hash,
            executor_public_key,
        }
    }
}

impl PredefinedEntry for ContextTransactionKey {
    type Codec = Borsh;
    type DataType<'a> = ContextTransaction;
}
