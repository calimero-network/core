#![expect(single_use_lifetimes, reason = "borsh shenanigans")]

use std::borrow::Cow;
use std::num::NonZeroUsize;

use borsh::{BorshDeserialize, BorshSerialize};

use crate::entry::{Borsh, Identity};
use crate::key;
use crate::slice::Slice;
use crate::types::PredefinedEntry;

pub type Hash = [u8; 32];

#[derive(BorshDeserialize, BorshSerialize, Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct ContextMeta {
    pub application: key::ApplicationMeta,
    pub root_hash: Hash,
}

impl ContextMeta {
    #[must_use]
    pub const fn new(application: key::ApplicationMeta, root_hash: Hash) -> Self {
        Self {
            application,
            root_hash,
        }
    }
}

impl PredefinedEntry for key::ContextMeta {
    type Codec = Borsh;
    type DataType<'a> = ContextMeta;
}

#[derive(BorshDeserialize, BorshSerialize, Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct ContextConfig {
    pub protocol: Box<str>,
    pub network: Box<str>,
    pub contract: Box<str>,
    pub proxy_contract: Box<str>,
    pub application_revision: u64,
    pub members_revision: u64,
}

impl ContextConfig {
    #[must_use]
    pub const fn new(
        protocol: Box<str>,
        network: Box<str>,
        contract: Box<str>,
        proxy_contract: Box<str>,
        application_revision: u64,
        members_revision: u64,
    ) -> Self {
        Self {
            protocol,
            network,
            contract,
            proxy_contract,
            application_revision,
            members_revision,
        }
    }
}

impl PredefinedEntry for key::ContextConfig {
    type Codec = Borsh;
    type DataType<'a> = ContextConfig;
}

#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct ContextState<'a> {
    pub value: Slice<'a>,
}

impl PredefinedEntry for key::ContextState {
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
    pub sender_key: Option<[u8; 32]>,
}

impl PredefinedEntry for key::ContextIdentity {
    type Codec = Borsh;
    type DataType<'a> = ContextIdentity;
}

#[derive(BorshDeserialize, BorshSerialize, Clone, Debug, Eq, PartialEq)]
pub enum ContextDelta<'a> {
    Head(NonZeroUsize),
    Data(Cow<'a, [u8]>),
}

impl PredefinedEntry for key::ContextDelta {
    type Codec = Borsh;
    type DataType<'a> = ContextDelta<'a>;
}
