use core::convert::Infallible;
use core::fmt::{self, Debug, Formatter};

#[cfg(feature = "borsh")]
use borsh::{BorshDeserialize, BorshSerialize};
use calimero_primitives::context::ContextId as PrimitiveContextId;
use calimero_primitives::identity::PublicKey as PrimitivePublicKey;
use generic_array::sequence::Concat;
use generic_array::typenum::U32;
use generic_array::GenericArray;

use crate::db::Column;
use crate::key::component::KeyComponent;
use crate::key::{AsKeyParts, FromKeyParts, Key};

#[derive(Clone, Copy, Debug)]
pub struct ContextId;

impl KeyComponent for ContextId {
    type LEN = U32;
}

#[derive(Clone, Copy, Eq, Ord, PartialEq, PartialOrd)]
#[cfg_attr(feature = "borsh", derive(BorshSerialize, BorshDeserialize))]
pub struct ContextMeta(Key<ContextId>);

impl ContextMeta {
    #[must_use]
    pub fn new(context_id: PrimitiveContextId) -> Self {
        Self(Key((*context_id).into()))
    }

    #[must_use]
    pub fn context_id(&self) -> PrimitiveContextId {
        (*AsRef::<[_; 32]>::as_ref(&self.0)).into()
    }
}

impl AsKeyParts for ContextMeta {
    type Components = (ContextId,);

    fn column() -> Column {
        Column::Meta
    }

    fn as_key(&self) -> &Key<Self::Components> {
        (&self.0).into()
    }
}

impl FromKeyParts for ContextMeta {
    type Error = Infallible;

    fn try_from_parts(parts: Key<Self::Components>) -> Result<Self, Self::Error> {
        Ok(Self(*<&_>::from(&parts)))
    }
}

impl Debug for ContextMeta {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("ContextMeta")
            .field("id", &self.context_id())
            .finish()
    }
}

#[derive(Clone, Copy, Debug)]
pub struct PublicKey;

impl KeyComponent for PublicKey {
    type LEN = U32;
}

#[derive(Clone, Copy, Eq, Ord, PartialEq, PartialOrd)]
#[cfg_attr(feature = "borsh", derive(BorshSerialize, BorshDeserialize))]
pub struct ContextIdentity(Key<(ContextId, PublicKey)>);

impl ContextIdentity {
    #[must_use]
    pub fn new(context_id: PrimitiveContextId, context_pk: PrimitivePublicKey) -> Self {
        Self(Key(
            GenericArray::from(*context_id).concat(GenericArray::from(context_pk.0))
        ))
    }

    #[must_use]
    pub fn context_id(&self) -> PrimitiveContextId {
        let mut context_id = [0; 32];

        context_id.copy_from_slice(&AsRef::<[_; 64]>::as_ref(&self.0)[..32]);

        context_id.into()
    }

    #[must_use]
    pub fn public_key(&self) -> [u8; 32] {
        let mut public_key = [0; 32];

        public_key.copy_from_slice(&AsRef::<[_; 64]>::as_ref(&self.0)[32..]);

        public_key
    }
}

impl AsKeyParts for ContextIdentity {
    type Components = (ContextId, PublicKey);

    fn column() -> Column {
        Column::Identity
    }

    fn as_key(&self) -> &Key<Self::Components> {
        &self.0
    }
}

impl FromKeyParts for ContextIdentity {
    type Error = Infallible;

    fn try_from_parts(parts: Key<Self::Components>) -> Result<Self, Self::Error> {
        Ok(Self(parts))
    }
}

impl Debug for ContextIdentity {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("ContextIdentity")
            .field("context_id", &self.context_id())
            .field("public_key", &self.public_key())
            .finish()
    }
}

#[derive(Clone, Copy, Debug)]
pub struct StateKey;

impl KeyComponent for StateKey {
    type LEN = U32;
}

#[derive(Clone, Copy, Eq, Ord, PartialEq, PartialOrd)]
#[cfg_attr(feature = "borsh", derive(BorshSerialize, BorshDeserialize))]
pub struct ContextState(Key<(ContextId, StateKey)>);

impl ContextState {
    #[must_use]
    pub fn new(context_id: PrimitiveContextId, state_key: [u8; 32]) -> Self {
        Self(Key(GenericArray::from(*context_id).concat(state_key.into())))
    }

    #[must_use]
    pub fn context_id(&self) -> PrimitiveContextId {
        let mut context_id = [0; 32];

        context_id.copy_from_slice(&AsRef::<[_; 64]>::as_ref(&self.0)[..32]);

        context_id.into()
    }

    #[must_use]
    pub fn state_key(&self) -> [u8; 32] {
        let mut state_key = [0; 32];

        state_key.copy_from_slice(&AsRef::<[_; 64]>::as_ref(&self.0)[32..]);

        state_key
    }
}

impl AsKeyParts for ContextState {
    type Components = (ContextId, StateKey);

    fn column() -> Column {
        Column::State
    }

    fn as_key(&self) -> &Key<Self::Components> {
        &self.0
    }
}

impl FromKeyParts for ContextState {
    type Error = Infallible;

    fn try_from_parts(parts: Key<Self::Components>) -> Result<Self, Self::Error> {
        Ok(Self(parts))
    }
}

impl Debug for ContextState {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("ContextState")
            .field("context_id", &self.context_id())
            .field("state_key", &self.state_key())
            .finish()
    }
}

#[derive(Clone, Copy, Debug)]
pub struct TransactionId;

impl KeyComponent for TransactionId {
    type LEN = U32;
}

#[derive(Clone, Copy, Eq, Ord, PartialEq, PartialOrd)]
#[cfg_attr(feature = "borsh", derive(BorshSerialize, BorshDeserialize))]
pub struct ContextTransaction(Key<(ContextId, TransactionId)>);

impl ContextTransaction {
    #[must_use]
    pub fn new(context_id: PrimitiveContextId, transaction_id: [u8; 32]) -> Self {
        Self(Key(
            GenericArray::from(*context_id).concat(transaction_id.into())
        ))
    }

    #[must_use]
    pub fn context_id(&self) -> PrimitiveContextId {
        let mut context_id = [0; 32];

        context_id.copy_from_slice(&AsRef::<[_; 64]>::as_ref(&self.0)[..32]);

        context_id.into()
    }

    #[must_use]
    pub fn transaction_id(&self) -> [u8; 32] {
        let mut transaction_id = [0; 32];

        transaction_id.copy_from_slice(&AsRef::<[_; 64]>::as_ref(&self.0)[32..]);

        transaction_id
    }
}

impl AsKeyParts for ContextTransaction {
    type Components = (ContextId, TransactionId);

    fn column() -> Column {
        Column::Transaction
    }

    fn as_key(&self) -> &Key<Self::Components> {
        &self.0
    }
}

impl FromKeyParts for ContextTransaction {
    type Error = Infallible;

    fn try_from_parts(parts: Key<Self::Components>) -> Result<Self, Self::Error> {
        Ok(Self(parts))
    }
}

impl Debug for ContextTransaction {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("ContextTransaction")
            .field("context_id", &self.context_id())
            .field("transaction_id", &self.transaction_id())
            .finish()
    }
}
