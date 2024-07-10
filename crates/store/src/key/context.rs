use std::convert::Infallible;
use std::fmt;

use generic_array::sequence::Concat;
use generic_array::typenum::U32;
use generic_array::GenericArray;

use crate::db::Column;
use crate::key::component::KeyComponent;
use crate::key::{AsKeyParts, FromKeyParts, Key};

pub struct ContextId;

impl KeyComponent for ContextId {
    type LEN = U32;
}

#[derive(Eq, Ord, Copy, Clone, PartialEq, PartialOrd)]
pub struct ContextMeta(Key<ContextId>);

impl ContextMeta {
    pub fn new(context_id: calimero_primitives::context::ContextId) -> Self {
        Self(Key((*context_id).into()))
    }

    pub fn context_id(&self) -> calimero_primitives::context::ContextId {
        (*AsRef::<[_; 32]>::as_ref(&self.0)).into()
    }
}

impl AsKeyParts for ContextMeta {
    type Components = (ContextId,);

    fn parts(&self) -> (Column, &Key<Self::Components>) {
        (Column::Identity, (&self.0).into())
    }
}

impl FromKeyParts for ContextMeta {
    type Error = Infallible;

    fn try_from_parts(parts: Key<Self::Components>) -> Result<Self, Self::Error> {
        Ok(Self(*<&_>::from(&parts)))
    }
}

impl fmt::Debug for ContextMeta {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ContextMeta")
            .field("id", &self.context_id())
            .finish()
    }
}

pub struct PublicKey;

impl KeyComponent for PublicKey {
    type LEN = U32;
}

#[derive(Eq, Ord, Copy, Clone, PartialEq, PartialOrd)]
pub struct ContextIdentity(Key<(ContextId, PublicKey)>);

impl ContextIdentity {
    pub fn new(context_id: calimero_primitives::context::ContextId, context_pk: [u8; 32]) -> Self {
        Self(Key(
            GenericArray::from(*context_id).concat(context_pk.into())
        ))
    }

    pub fn context_id(&self) -> calimero_primitives::context::ContextId {
        let mut context_id = [0; 32];

        context_id.copy_from_slice(&AsRef::<[_; 64]>::as_ref(&self.0)[..32]);

        context_id.into()
    }

    pub fn public_key(&self) -> [u8; 32] {
        let mut public_key = [0; 32];

        public_key.copy_from_slice(&AsRef::<[_; 64]>::as_ref(&self.0)[32..]);

        public_key
    }
}

impl AsKeyParts for ContextIdentity {
    type Components = (ContextId, PublicKey);

    fn parts(&self) -> (Column, &Key<Self::Components>) {
        (Column::Identity, (&self.0).into())
    }
}

impl FromKeyParts for ContextIdentity {
    type Error = Infallible;

    fn try_from_parts(parts: Key<Self::Components>) -> Result<Self, Self::Error> {
        Ok(Self(parts))
    }
}

impl fmt::Debug for ContextIdentity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ContextIdentity")
            .field("context_id", &self.context_id())
            .field("public_key", &self.public_key())
            .finish()
    }
}

pub struct StateKey;

impl KeyComponent for StateKey {
    type LEN = U32;
}

#[derive(Eq, Ord, Copy, Clone, PartialEq, PartialOrd)]
pub struct ContextState(Key<(ContextId, StateKey)>);

impl ContextState {
    pub fn new(context_id: calimero_primitives::context::ContextId, state_key: [u8; 32]) -> Self {
        Self(Key(GenericArray::from(*context_id).concat(state_key.into())))
    }

    pub fn context_id(&self) -> calimero_primitives::context::ContextId {
        let mut context_id = [0; 32];

        context_id.copy_from_slice(&AsRef::<[_; 64]>::as_ref(&self.0)[..32]);

        context_id.into()
    }

    pub fn state_key(&self) -> [u8; 32] {
        let mut state_key = [0; 32];

        state_key.copy_from_slice(&AsRef::<[_; 64]>::as_ref(&self.0)[32..]);

        state_key
    }
}

impl AsKeyParts for ContextState {
    type Components = (ContextId, StateKey);

    fn parts(&self) -> (Column, &Key<Self::Components>) {
        (Column::State, (&self.0).into())
    }
}

impl FromKeyParts for ContextState {
    type Error = Infallible;

    fn try_from_parts(parts: Key<Self::Components>) -> Result<Self, Self::Error> {
        Ok(Self(parts))
    }
}

impl fmt::Debug for ContextState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ContextState")
            .field("context_id", &self.context_id())
            .field("state_key", &self.state_key())
            .finish()
    }
}

pub struct TransactionId;

impl KeyComponent for TransactionId {
    type LEN = U32;
}

#[derive(Eq, Ord, Copy, Clone, PartialEq, PartialOrd)]
pub struct ContextTransaction(Key<(ContextId, TransactionId)>);

impl ContextTransaction {
    pub fn new(
        context_id: calimero_primitives::context::ContextId,
        transaction_id: [u8; 32],
    ) -> Self {
        Self(Key(
            GenericArray::from(*context_id).concat(transaction_id.into())
        ))
    }

    pub fn context_id(&self) -> calimero_primitives::context::ContextId {
        let mut context_id = [0; 32];

        context_id.copy_from_slice(&AsRef::<[_; 64]>::as_ref(&self.0)[..32]);

        context_id.into()
    }

    pub fn transaction_id(&self) -> [u8; 32] {
        let mut transaction_id = [0; 32];

        transaction_id.copy_from_slice(&AsRef::<[_; 64]>::as_ref(&self.0)[32..]);

        transaction_id
    }
}

impl AsKeyParts for ContextTransaction {
    type Components = (ContextId, TransactionId);

    fn parts(&self) -> (Column, &Key<Self::Components>) {
        (Column::Transaction, &self.0)
    }
}

impl FromKeyParts for ContextTransaction {
    type Error = Infallible;

    fn try_from_parts(parts: Key<Self::Components>) -> Result<Self, Self::Error> {
        Ok(Self(parts))
    }
}

impl fmt::Debug for ContextTransaction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ContextTransaction")
            .field("context_id", &self.context_id())
            .field("transaction_id", &self.transaction_id())
            .finish()
    }
}
