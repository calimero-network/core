//! Values some members replicate but cannot read: [`Sealed<T>`] and
//! [`TeeSecret<T>`].
//!
//! A card game needs two kinds of hidden value. A player's hand is dealt by the
//! TEE, sealed to that player's device key: every member stores the envelope,
//! and only the player's node opens it ([`Sealed::to_key`]). The rest of the
//! deck is sealed to every TEE authority's attested key, so only a TEE-triggered
//! run can read it ([`Sealed::to_tee`], or [`TeeSecret`] for a whole cell).
//!
//! Keep either in `TeeOnly` state. Sealing proves confidentiality, never
//! authorship: anyone who knows a key can seal to it, so a sealed card is
//! genuine because only the TEE authority can write where it is stored.
//!
//! A TEE authority admitted after a secret was sealed cannot open it until a
//! TEE run seals it again, which [`TeeSecret::set`] does on every write.

use core::marker::PhantomData;

use borsh::{BorshDeserialize, BorshSerialize};
use calimero_sdk::env;

use super::permissioned::{PermissionedStorage, TeeAuthorityAcl};
use super::{LwwRegister, StoreError};

/// A `T` sealed to one or more Ed25519 keys; each opens it on its own.
///
/// Stored as a list of `recipient key ‖ envelope` byte strings, so a reader
/// opens only the envelope made for it.
pub struct Sealed<T> {
    envelopes: Vec<Vec<u8>>,
    _value: PhantomData<fn() -> T>,
}

// By hand, because a derive would demand `T: Borsh*` for a `T` that is never
// stored as itself: only its envelopes are.
impl<T> BorshSerialize for Sealed<T> {
    fn serialize<W: borsh::io::Write>(&self, writer: &mut W) -> borsh::io::Result<()> {
        self.envelopes.serialize(writer)
    }
}

impl<T> BorshDeserialize for Sealed<T> {
    fn deserialize_reader<R: borsh::io::Read>(reader: &mut R) -> borsh::io::Result<Self> {
        Ok(Self {
            envelopes: Vec::deserialize_reader(reader)?,
            _value: PhantomData,
        })
    }
}

impl<T> Clone for Sealed<T> {
    fn clone(&self) -> Self {
        Self {
            envelopes: self.envelopes.clone(),
            _value: PhantomData,
        }
    }
}

impl<T> Default for Sealed<T> {
    fn default() -> Self {
        Self {
            envelopes: Vec::new(),
            _value: PhantomData,
        }
    }
}

impl<T> core::fmt::Debug for Sealed<T> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Sealed")
            .field("recipients", &self.envelopes.len())
            .finish()
    }
}

impl<T> Sealed<T>
where
    T: BorshSerialize + BorshDeserialize,
{
    /// Seal `value` to each of `keys`.
    ///
    /// # Errors
    /// [`StoreError::SealFailed`] if `value` does not serialize or a key is not a
    /// usable Ed25519 point.
    pub fn to_keys(keys: &[[u8; 32]], value: &T) -> Result<Self, StoreError> {
        let plaintext = borsh::to_vec(value).map_err(|_| StoreError::SealFailed)?;
        let envelopes = keys
            .iter()
            .map(|key| {
                env::seal_to(key, &plaintext)
                    .map(|envelope| [key.as_slice(), &envelope].concat())
                    .ok_or(StoreError::SealFailed)
            })
            .collect::<Result<_, _>>()?;
        Ok(Self {
            envelopes,
            _value: PhantomData,
        })
    }

    /// Seal `value` to one device key, typically a player's `env::device_id()`
    /// from a run they made.
    ///
    /// # Errors
    /// As [`to_keys`](Self::to_keys).
    pub fn to_key(key: &[u8; 32], value: &T) -> Result<Self, StoreError> {
        Self::to_keys(core::slice::from_ref(key), value)
    }

    /// Seal `value` to every TEE authority of the context, so that only a
    /// TEE-triggered run can read it. Only callable in such a run.
    ///
    /// # Errors
    /// As [`to_keys`](Self::to_keys), and [`StoreError::SealFailed`] when the
    /// context has no TEE authority to seal to.
    pub fn to_tee(value: &T) -> Result<Self, StoreError> {
        let keys = env::tee_authority_keys();
        if keys.is_empty() {
            return Err(StoreError::SealFailed);
        }
        Self::to_keys(&keys, value)
    }

    /// The value, if one of the envelopes was made for this run's device and
    /// opens. `None` for every other reader.
    #[must_use]
    pub fn open(&self) -> Option<T> {
        let me = env::device_id();
        let envelope = self
            .envelopes
            .iter()
            .find_map(|entry| entry.strip_prefix(me.as_slice()))?;
        let plaintext = env::open_sealed(envelope)?;
        T::try_from_slice(&plaintext).ok()
    }

    /// Whether anything is sealed here.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.envelopes.is_empty()
    }
}

/// A `TeeOnly` cell whose value only the TEE authorities can read.
///
/// Members replicate it and see that it is set; only a TEE-triggered run on a
/// TEE authority opens it. The deck of a card game is the motivating case.
pub type TeeSecret<T> = PermissionedStorage<LwwRegister<Sealed<T>>, TeeAuthorityAcl>;

impl<T> PermissionedStorage<LwwRegister<Sealed<T>>, TeeAuthorityAcl>
where
    T: BorshSerialize + BorshDeserialize + 'static,
{
    /// A new secret. Like every `TeeOnly` cell it stores nothing until the TEE
    /// authority's first write.
    #[must_use]
    pub fn new_tee_secret() -> Self {
        Self::new_tee_only()
    }

    /// The value, when this is a TEE-triggered run on a TEE authority that it
    /// was sealed to; `None` when nothing is set yet.
    ///
    /// # Errors
    /// [`StoreError::SealFailed`] if a value is set but does not open for this
    /// run; any error [`try_get`](Self::try_get) reports.
    pub fn reveal(&self) -> Result<Option<T>, StoreError> {
        match self.try_get()? {
            Some(register) if !register.get().is_empty() => register
                .get()
                .open()
                .map(Some)
                .ok_or(StoreError::SealFailed),
            _ => Ok(None),
        }
    }

    /// Seal `value` to every current TEE authority and store it. Only a
    /// TEE-triggered run may call this.
    ///
    /// # Errors
    /// Any error [`Sealed::to_tee`] or [`insert`](Self::insert) reports.
    pub fn set(&mut self, value: &T) -> Result<(), StoreError> {
        let sealed = Sealed::to_tee(value)?;
        let _previous = self.insert(LwwRegister::new(sealed))?;
        Ok(())
    }
}

/// Borsh-identical to its envelope list, so it describes as one.
#[cfg(not(target_arch = "wasm32"))]
impl<T> calimero_wasm_abi::abi_type::AbiType for Sealed<T> {
    fn type_ref(
        reg: &mut calimero_wasm_abi::abi_type::TypeRegistry,
    ) -> calimero_wasm_abi::schema::TypeRef {
        <Vec<Vec<u8>> as calimero_wasm_abi::abi_type::AbiType>::type_ref(reg)
    }
}
