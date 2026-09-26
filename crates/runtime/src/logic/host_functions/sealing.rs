//! Sealing: public-key encryption an app can use to keep a value from the
//! members that replicate it.
//!
//! A TEE deals a card by sealing it to the player's device key and writing the
//! envelope into `TeeOnly` state. Every member replicates the envelope; only the
//! player's own node can open it, in a run whose executor is that key. A value
//! only the TEE may read — the rest of the deck — is sealed to every TEE
//! authority's attested key instead (the SDK's `TeeSecret<T>`).
//!
//! An envelope proves confidentiality, never authorship: anyone who knows a key
//! can seal to it. What makes a dealt card genuine is that it sits in `TeeOnly`
//! state, which only the TEE authority can write.

use crate::errors::HostError;
use crate::logic::{sys, VMHostFunctions, VMLogicResult};

impl VMHostFunctions<'_> {
    /// Seals the bytes at `src_plaintext_ptr` to the Ed25519 public key at
    /// `src_key_ptr`, and puts the envelope in register `dest_register_id`.
    ///
    /// Available in every run: sealing reveals nothing.
    ///
    /// # Returns
    ///
    /// * `1` if the envelope is in the register.
    /// * `0` if the key is not a usable Ed25519 point.
    ///
    /// # Errors
    ///
    /// * `HostError::InvalidMemoryAccess` if memory access fails for a descriptor buffer.
    pub fn seal_to(
        &mut self,
        src_key_ptr: u64,
        src_plaintext_ptr: u64,
        dest_register_id: u64,
    ) -> VMLogicResult<u32> {
        // SAFETY: `sys::Buffer<'_>` is a vetted `GuestAbiType` ABI descriptor (a `#[repr(C)]`
        //         layout of `u64`-shaped fields), so reinterpreting the guest bytes as
        //         it is sound; the guest SDK wrote a well-formed instance at this
        //         offset and the read is bounds-checked. See `read_guest_memory_typed`.
        let key_buf = unsafe { self.read_guest_memory_typed::<sys::Buffer<'_>>(src_key_ptr)? };
        // SAFETY: as above.
        let plaintext_buf =
            unsafe { self.read_guest_memory_typed::<sys::Buffer<'_>>(src_plaintext_ptr)? };
        let key = calimero_primitives::identity::PublicKey::from(
            *self.read_guest_memory_sized::<32>(&key_buf)?,
        );
        let plaintext = self.read_guest_memory_slice(&plaintext_buf)?.to_vec();

        let Ok(envelope) = calimero_crypto::seal_to_root(&mut rand::rng(), &key, plaintext) else {
            return Ok(0);
        };
        self.with_logic_mut(|logic| {
            logic
                .registers
                .set(logic.limits, dest_register_id, envelope.to_bytes())
        })?;
        Ok(1)
    }

    /// Opens the envelope at `src_sealed_ptr` with this run's executor key, and
    /// puts the plaintext in register `dest_register_id`.
    ///
    /// # Returns
    ///
    /// * `1` if the envelope opened.
    /// * `0` if it did not: it is malformed, sealed to another key, or tampered
    ///   with. The three are deliberately indistinguishable.
    ///
    /// # Errors
    ///
    /// * `HostError::TeeOnly` when the node gave this run no key to open with.
    ///   That is a run on a TEE node that the TEE scheduler did not fire: what is
    ///   sealed to a TEE is readable only inside a TEE-triggered run.
    /// * `HostError::InvalidMemoryAccess` if memory access fails for a descriptor buffer.
    pub fn open_sealed(
        &mut self,
        src_sealed_ptr: u64,
        dest_register_id: u64,
    ) -> VMLogicResult<u32> {
        let Some(opener) = self.borrow_logic().context.sealing.opener.clone() else {
            return Err(HostError::TeeOnly {
                function: "open_sealed",
            }
            .into());
        };
        // SAFETY: `sys::Buffer<'_>` is a vetted `GuestAbiType` ABI descriptor (a `#[repr(C)]`
        //         layout of `u64`-shaped fields), so reinterpreting the guest bytes as
        //         it is sound; the guest SDK wrote a well-formed instance at this
        //         offset and the read is bounds-checked. See `read_guest_memory_typed`.
        let sealed_buf =
            unsafe { self.read_guest_memory_typed::<sys::Buffer<'_>>(src_sealed_ptr)? };
        let sealed = self.read_guest_memory_slice(&sealed_buf)?;

        let Some(envelope) = calimero_crypto::SealedEnvelope::from_bytes(sealed) else {
            return Ok(0);
        };
        let Ok(plaintext) = calimero_crypto::open_sealed(&opener, &envelope) else {
            return Ok(0);
        };
        self.with_logic_mut(|logic| {
            logic
                .registers
                .set(logic.limits, dest_register_id, plaintext)
        })?;
        Ok(1)
    }

    /// Puts the attested keys of the context's TEE authorities in register
    /// `dest_register_id`, 32 bytes each, in the order the node listed them.
    ///
    /// # Errors
    ///
    /// * `HostError::TeeOnly` outside a TEE-triggered run.
    pub fn tee_authority_keys(&mut self, dest_register_id: u64) -> VMLogicResult<()> {
        let logic = self.borrow_logic();
        if !logic.context.tee_trigger {
            return Err(HostError::TeeOnly {
                function: "tee_authority_keys",
            }
            .into());
        }
        let keys = logic.context.sealing.tee_authority_keys.concat();
        self.with_logic_mut(|logic| logic.registers.set(logic.limits, dest_register_id, keys))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use calimero_primitives::identity::PrivateKey;
    use wasmer::{AsStoreMut, Store};

    use crate::errors::HostError;
    use crate::logic::tests::{prepare_guest_buf_descriptor, SimpleMockStorage};
    use crate::logic::{
        Cow, SealingContext, VMContext, VMLimits, VMLogic, VMLogicError, DIGEST_SIZE,
    };

    const KEY_DESC: u64 = 10;
    const TEXT_DESC: u64 = 30;
    const KEY_AT: u64 = 200;
    const TEXT_AT: u64 = 300;
    const REGISTER: u64 = 1;

    fn context(tee_trigger: bool, sealing: SealingContext) -> VMContext<'static> {
        let mut context = VMContext::new(
            Cow::Owned(vec![]),
            [0u8; DIGEST_SIZE],
            [0u8; DIGEST_SIZE],
            calimero_account::AccountId::from([0u8; DIGEST_SIZE]),
        );
        context.tee_trigger = tee_trigger;
        context.sealing = sealing;
        context
    }

    /// Run `f` against host functions over a fresh VM with `context`.
    fn with_host<R>(
        context: VMContext<'static>,
        f: impl FnOnce(&mut crate::logic::VMHostFunctions<'_>) -> R,
    ) -> R {
        let mut storage = SimpleMockStorage::new();
        let limits = VMLimits::default();
        let mut logic = VMLogic::new(&mut storage, None, context, &limits, None);
        let mut store = Store::default();
        let memory =
            wasmer::Memory::new(&mut store, wasmer::MemoryType::new(1, None, false)).unwrap();
        let _ = logic.with_memory(memory);
        let mut host = logic.host_functions(store.as_store_mut());
        f(&mut host)
    }

    fn put(host: &crate::logic::VMHostFunctions<'_>, desc: u64, at: u64, bytes: &[u8]) {
        host.borrow_memory().write(at, bytes).unwrap();
        prepare_guest_buf_descriptor(host, desc, at, bytes.len() as u64);
    }

    fn register(host: &crate::logic::VMHostFunctions<'_>) -> Vec<u8> {
        host.borrow_logic()
            .registers
            .get(REGISTER)
            .unwrap()
            .to_vec()
    }

    fn seal(recipient: &PrivateKey, plaintext: &[u8]) -> Vec<u8> {
        with_host(context(false, SealingContext::default()), |host| {
            put(host, KEY_DESC, KEY_AT, recipient.public_key().as_ref());
            put(host, TEXT_DESC, TEXT_AT, plaintext);
            assert_eq!(host.seal_to(KEY_DESC, TEXT_DESC, REGISTER).unwrap(), 1);
            register(host)
        })
    }

    fn open(
        opener: Option<&PrivateKey>,
        tee_trigger: bool,
        sealed: &[u8],
    ) -> Result<Option<Vec<u8>>, VMLogicError> {
        let sealing = SealingContext {
            opener: opener.map(|key| Arc::new(PrivateKey::from(*key.as_bytes()))),
            tee_authority_keys: vec![],
        };
        with_host(context(tee_trigger, sealing), |host| {
            put(host, TEXT_DESC, TEXT_AT, sealed);
            Ok((host.open_sealed(TEXT_DESC, REGISTER)? == 1).then(|| register(host)))
        })
    }

    /// A card sealed to a player opens on that player's key, for nobody else,
    /// and not at all in a run the node gave no key to.
    #[test]
    fn an_envelope_opens_only_for_its_recipient() {
        let mut rng = rand::rng();
        let player = PrivateKey::random(&mut rng);
        let other = PrivateKey::random(&mut rng);
        let sealed = seal(&player, b"queen of hearts");
        assert_ne!(
            sealed, b"queen of hearts",
            "the envelope is not the plaintext"
        );

        assert_eq!(
            open(Some(&player), false, &sealed).unwrap().as_deref(),
            Some(b"queen of hearts".as_slice())
        );
        assert_eq!(open(Some(&other), false, &sealed).unwrap(), None);
        assert_eq!(open(Some(&player), false, &sealed[..20]).unwrap(), None);
        assert!(matches!(
            open(None, false, &sealed),
            Err(VMLogicError::HostError(HostError::TeeOnly { .. }))
        ));
    }

    /// The TEE authority keys are only handed to a TEE-triggered run.
    #[test]
    fn tee_authority_keys_are_only_available_to_a_tee_run() {
        let keys = vec![[1u8; 32], [2u8; 32]];
        for tee_trigger in [false, true] {
            let sealing = SealingContext {
                opener: None,
                tee_authority_keys: keys.clone(),
            };
            with_host(context(tee_trigger, sealing), |host| {
                let result = host.tee_authority_keys(REGISTER);
                if tee_trigger {
                    assert!(result.is_ok());
                    assert_eq!(register(host), keys.concat());
                } else {
                    assert!(matches!(
                        result,
                        Err(VMLogicError::HostError(HostError::TeeOnly { .. }))
                    ));
                }
            });
        }
    }
}
