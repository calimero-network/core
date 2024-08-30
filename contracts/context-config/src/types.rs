use std::fmt;
use std::ops::{Deref, DerefMut};

use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use near_sdk::store::IterableSet;
use near_sdk::{bs58, env, near, serde, serde_json, AccountId};
use thiserror::Error;

use crate::repr::{self, Repr, ReprBytes};
use crate::Prefix;

#[derive(Eq, Ord, Copy, Debug, Clone, PartialEq, PartialOrd)]
#[near(serializers = [borsh])]
pub struct ContextId([u8; 32]);

impl ReprBytes for ContextId {
    type EncodeBytes<'a> = &'a [u8; 32];
    type DecodeBytes = [u8; 32];

    type Error = repr::InsufficientLength;

    fn as_bytes(&self) -> Self::EncodeBytes<'_> {
        &self.0
    }

    fn from_bytes<F>(f: F) -> Result<Self, repr::Error<Self::Error>>
    where
        F: FnOnce(&mut Self::DecodeBytes) -> bs58::decode::Result<usize>,
    {
        Self::DecodeBytes::from_bytes(f).map(Self)
    }
}

#[derive(Eq, Ord, Copy, Debug, Clone, PartialEq, PartialOrd)]
#[near(serializers = [borsh])]
pub struct ContextIdentity([u8; 32]);

impl ReprBytes for ContextIdentity {
    type EncodeBytes<'a> = &'a [u8; 32];
    type DecodeBytes = [u8; 32];

    type Error = repr::InsufficientLength;

    fn as_bytes(&self) -> Self::EncodeBytes<'_> {
        &self.0
    }

    fn from_bytes<F>(f: F) -> Result<Self, repr::Error<Self::Error>>
    where
        F: FnOnce(&mut Self::DecodeBytes) -> bs58::decode::Result<usize>,
    {
        Self::DecodeBytes::from_bytes(f).map(Self)
    }
}

#[derive(Eq, Ord, Copy, Debug, Clone, PartialEq, PartialOrd)]
#[near(serializers = [borsh])]
pub struct BlobId([u8; 32]);

impl ReprBytes for BlobId {
    type EncodeBytes<'a> = &'a [u8; 32];
    type DecodeBytes = [u8; 32];

    type Error = repr::InsufficientLength;

    fn as_bytes(&self) -> Self::EncodeBytes<'_> {
        &self.0
    }

    fn from_bytes<F>(f: F) -> Result<Self, repr::Error<Self::Error>>
    where
        F: FnOnce(&mut Self::DecodeBytes) -> bs58::decode::Result<usize>,
    {
        Self::DecodeBytes::from_bytes(f).map(Self)
    }
}

#[derive(Eq, Ord, Copy, Debug, Clone, PartialEq, PartialOrd)]
#[near(serializers = [borsh])]
pub struct ApplicationId([u8; 32]);

impl ReprBytes for ApplicationId {
    type EncodeBytes<'a> = &'a [u8; 32];
    type DecodeBytes = [u8; 32];

    type Error = repr::InsufficientLength;

    fn as_bytes(&self) -> Self::EncodeBytes<'_> {
        &self.0
    }

    fn from_bytes<F>(f: F) -> Result<Self, repr::Error<Self::Error>>
    where
        F: FnOnce(&mut Self::DecodeBytes) -> bs58::decode::Result<usize>,
    {
        Self::DecodeBytes::from_bytes(f).map(Self)
    }
}

#[derive(Eq, Ord, Debug, Default, Clone, PartialEq, PartialOrd)]
#[near(serializers = [borsh, json])]
pub struct ApplicationSource(pub String);

impl ReprBytes for Signature {
    type EncodeBytes<'a> = [u8; 64];
    type DecodeBytes = [u8; 64];

    type Error = repr::InsufficientLength;

    fn as_bytes(&self) -> Self::EncodeBytes<'_> {
        self.to_bytes()
    }

    fn from_bytes<F>(f: F) -> Result<Self, repr::Error<Self::Error>>
    where
        F: FnOnce(&mut Self::DecodeBytes) -> bs58::decode::Result<usize>,
    {
        Self::DecodeBytes::from_bytes(f).map(|b| Self::from_bytes(&b))
    }
}

#[derive(Eq, Debug, Clone, PartialEq)]
#[near(serializers = [json])]
pub struct SignedPayload<T> {
    payload: Repr<Vec<u8>>,
    signature: Repr<Signature>,
    #[serde(skip)]
    _priv: std::marker::PhantomData<T>,
}

#[derive(Debug, Error)]
pub enum Error<E> {
    #[error("invalid signature")]
    InvalidSignature,
    #[error("failed to parse JSON payload: {0}")]
    ParseError(#[from] serde_json::Error),
    #[error("failed to derive verifing key: {0}")]
    KeyDerivationError(E),
}

impl<'a, T: serde::Serialize + serde::Deserialize<'a>> SignedPayload<T> {
    pub fn new(payload: &T, sign: impl FnOnce(&[u8]) -> Signature) -> serde_json::Result<Self> {
        let payload = serde_json::to_vec(&payload)?;
        let signature = sign(&payload);
        Ok(Self {
            payload: Repr::new(payload),
            signature: Repr::new(signature),
            _priv: Default::default(),
        })
    }

    pub fn parse<E>(
        &'a self,
        f: impl FnOnce(&T) -> Result<VerifyingKey, E>,
    ) -> Result<T, Error<E>> {
        let parsed = serde_json::from_slice(&self.payload)?;
        let key = f(&parsed).map_err(Error::KeyDerivationError)?;
        key.verify(&self.payload, &self.signature)
            .map_or(Err(Error::InvalidSignature), |_| Ok(parsed))
    }
}

#[derive(Debug)]
#[near(serializers = [borsh])]
pub struct Guard<T> {
    inner: T,
    priviledged: IterableSet<AccountId>,
}

#[derive(Copy, Clone, Error)]
#[error("unauthorized access")]
pub struct UnauthorizedAccess {
    _priv: (),
}

impl fmt::Debug for UnauthorizedAccess {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

impl<T> Guard<T> {
    pub fn new(inner: T, prefix: Prefix) -> Self {
        let mut priviledged = IterableSet::new(prefix);

        priviledged.insert(env::signer_account_id());

        Self { inner, priviledged }
    }

    pub fn get_mut(&mut self) -> Result<GuardMut<'_, T>, UnauthorizedAccess> {
        if !self.priviledged.contains(&env::signer_account_id()) {
            return Err(UnauthorizedAccess { _priv: () });
        }

        Ok(GuardMut { inner: self })
    }

    pub fn priviledged(&self) -> &IterableSet<AccountId> {
        &self.priviledged
    }
}

impl<T> Deref for Guard<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

#[derive(Debug)]
pub struct GuardMut<'a, T> {
    inner: &'a mut Guard<T>,
}

impl<T> GuardMut<'_, T> {
    pub fn priviledges(&mut self) -> Priviledges<'_> {
        Priviledges {
            inner: &mut self.inner.priviledged,
        }
    }
}

impl<T> Deref for GuardMut<'_, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl<T> DerefMut for GuardMut<'_, T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.inner.inner
    }
}

#[derive(Debug)]
pub struct Priviledges<'a> {
    inner: &'a mut IterableSet<AccountId>,
}

impl Priviledges<'_> {
    pub fn grant(&mut self, account_id: AccountId) {
        self.inner.insert(account_id);
    }

    pub fn revoke(&mut self, account_id: AccountId) {
        self.inner.remove(&account_id);
    }
}
