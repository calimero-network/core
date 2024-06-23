use generic_array::sequence::Concat;
use generic_array::typenum::U32;
use generic_array::GenericArray;

use crate::db::Column;
use crate::key::component::KeyComponent;
use crate::key::{AsKeyParts, Key};

pub struct ContextId;

impl KeyComponent for ContextId {
    type LEN = U32;
}

#[derive(Copy, Clone)]
pub struct ContextMeta(Key<ContextId>);

impl ContextMeta {
    pub fn new(context_id: [u8; 32]) -> Self {
        Self(Key(context_id.into()))
    }
}

impl AsKeyParts for ContextMeta {
    type Components = (ContextId,);

    fn parts(&self) -> (Column, &Key<Self::Components>) {
        (Column::Identity, (&self.0).into())
    }
}

pub struct PublicKey;

impl KeyComponent for PublicKey {
    type LEN = U32;
}

#[derive(Copy, Clone)]
pub struct ContextIdentity(Key<(ContextId, PublicKey)>);

impl ContextIdentity {
    pub fn new(context_id: [u8; 32], context_pk: [u8; 32]) -> Self {
        Self(Key(GenericArray::from(context_id).concat(context_pk.into())))
    }
}

impl AsKeyParts for ContextIdentity {
    type Components = (ContextId, PublicKey);

    fn parts(&self) -> (Column, &Key<Self::Components>) {
        (Column::Identity, (&self.0).into())
    }
}

pub struct StateKey;

impl KeyComponent for StateKey {
    type LEN = U32;
}

#[derive(Copy, Clone)]
pub struct ContextState(Key<(ContextId, StateKey)>);

impl ContextState {
    pub fn new(context_id: [u8; 32], state_key: [u8; 32]) -> Self {
        Self(Key(GenericArray::from(context_id).concat(state_key.into())))
    }
}

impl AsKeyParts for ContextState {
    type Components = (ContextId, StateKey);

    fn parts(&self) -> (Column, &Key<Self::Components>) {
        (Column::State, (&self.0).into())
    }
}

pub struct TransactionId;

impl KeyComponent for TransactionId {
    type LEN = U32;
}

#[derive(Copy, Clone)]
pub struct ContextTransaction(Key<(ContextId, TransactionId)>);

impl ContextTransaction {
    pub fn new(context_id: [u8; 32], transaction_id: [u8; 32]) -> Self {
        Self(Key(
            GenericArray::from(context_id).concat(transaction_id.into())
        ))
    }
}

impl AsKeyParts for ContextTransaction {
    type Components = (ContextId, TransactionId);

    fn parts(&self) -> (Column, &Key<Self::Components>) {
        (Column::Transaction, &self.0)
    }
}
