use borsh::{BorshDeserialize, BorshSerialize};
use calimero_primitives::identity::{KeyPair, PeerId, PublicKey};

use crate::entry::{Borsh, Identity};
use crate::key::{
    ApplicationMeta as ApplicationMetaKey, ContextIdentity as ContextIdentityKey,
    ContextMeta as ContextMetaKey, ContextState as ContextStateKey,
    ContextTransaction as ContextTransactionKey,
};
use crate::slice::Slice;
use crate::types::PredefinedEntry;

pub type TransactionHash = [u8; 32];

#[derive(BorshDeserialize, BorshSerialize, Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct ContextMeta {
    pub application: ApplicationMetaKey,
    pub last_transaction_hash: TransactionHash,
    pub coordinator_peer: Option<PeerId>,
}

impl ContextMeta {
    #[must_use]
    pub const fn new(
        application: ApplicationMetaKey,
        last_transaction_hash: TransactionHash,
        coordinator_peer: Option<PeerId>,
    ) -> Self {
        Self {
            application,
            last_transaction_hash,
            coordinator_peer,
        }
    }
}

impl PredefinedEntry for ContextMetaKey {
    type Codec = Borsh;
    type DataType<'a> = ContextMeta;
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
#[allow(clippy::exhaustive_structs)]
pub struct ContextIdentity {
    pub public_key: PublicKey,
    pub private_key: Option<[u8; 32]>,
}

impl From<KeyPair> for ContextIdentity {
    fn from(id: KeyPair) -> Self {
        Self {
            public_key: id.public_key,
            private_key: id.private_key,
        }
    }
}

impl From<ContextIdentity> for KeyPair {
    fn from(id: ContextIdentity) -> Self {
        Self {
            public_key: id.public_key,
            private_key: id.private_key,
        }
    }
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
    pub prior_hash: TransactionHash,
    pub executor_public_key: [u8; 32],
}

impl ContextTransaction {
    #[must_use]
    pub fn new(
        method: Box<str>,
        payload: Box<[u8]>,
        prior_hash: TransactionHash,
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
