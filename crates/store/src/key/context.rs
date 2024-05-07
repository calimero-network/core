use generic_array::sequence::Concat;
use generic_array::GenericArray;

use crate::db::Column;
use crate::key::component::{ContextId, TransactionId};
use crate::key::{Key, KeyParts};

#[derive(Copy, Clone)]
pub struct ContextIdentity(Key<ContextId>);

impl ContextIdentity {
    pub fn new(context_id: [u8; 32]) -> Self {
        Self(Key(context_id.into()))
    }
}

impl KeyParts for ContextIdentity {
    type Components = (ContextId,);

    fn column(&self) -> Column {
        Column::Identity
    }

    fn key(&self) -> &Key<Self::Components> {
        (&self.0).into()
    }
}

#[derive(Copy, Clone)]
pub struct ContextState(Key<ContextId>);

impl ContextState {
    pub fn new(context_id: [u8; 32]) -> Self {
        Self(Key(context_id.into()))
    }
}

impl KeyParts for ContextState {
    type Components = (ContextId,);

    fn column(&self) -> Column {
        Column::State
    }

    fn key(&self) -> &Key<Self::Components> {
        (&self.0).into()
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

impl KeyParts for ContextTransaction {
    type Components = (ContextId, TransactionId);

    fn column(&self) -> Column {
        Column::Transaction
    }

    fn key(&self) -> &Key<Self::Components> {
        &self.0
    }
}

#[derive(Copy, Clone)]
pub struct ContextMembers(Key<ContextId>);

impl ContextMembers {
    pub fn new(context_id: [u8; 32]) -> Self {
        Self(Key(context_id.into()))
    }
}

impl KeyParts for ContextMembers {
    type Components = (ContextId,);

    fn column(&self) -> Column {
        Column::Membership
    }

    fn key(&self) -> &Key<Self::Components> {
        (&self.0).into()
    }
}
