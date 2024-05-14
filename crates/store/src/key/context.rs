use generic_array::sequence::Concat;
use generic_array::GenericArray;

use crate::db::Column;
use crate::key::component::{ContextId, TransactionId};
use crate::key::{AsKeyParts, Key};

#[derive(Copy, Clone)]
pub struct ContextIdentity(Key<ContextId>);

impl ContextIdentity {
    pub fn new(context_id: [u8; 32]) -> Self {
        Self(Key(context_id.into()))
    }
}

impl AsKeyParts for ContextIdentity {
    type Components = (ContextId,);

    fn parts(&self) -> (Column, &Key<Self::Components>) {
        (Column::Identity, (&self.0).into())
    }
}

#[derive(Copy, Clone)]
pub struct ContextState(Key<ContextId>);

impl ContextState {
    pub fn new(context_id: [u8; 32]) -> Self {
        Self(Key(context_id.into()))
    }
}

impl AsKeyParts for ContextState {
    type Components = (ContextId,);

    fn parts(&self) -> (Column, &Key<Self::Components>) {
        (Column::State, (&self.0).into())
    }
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

#[derive(Copy, Clone)]
pub struct ContextMembers(Key<ContextId>);

impl ContextMembers {
    pub fn new(context_id: [u8; 32]) -> Self {
        Self(Key(context_id.into()))
    }
}

impl AsKeyParts for ContextMembers {
    type Components = (ContextId,);

    fn parts(&self) -> (Column, &Key<Self::Components>) {
        (Column::Membership, (&self.0).into())
    }
}
