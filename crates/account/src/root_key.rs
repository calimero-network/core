//! Root-key rotation: the [`RootKeyHandoff`] chain, and the walk that resolves
//! the account's root key at a given epoch.
//!
//! Everything else in the crate that checks a signature ultimately asks
//! [`root_key_at_epoch`] which key was in force, so this module is the one place
//! that decides what a valid chain is.
//!
//! # Why it is shaped this way
//!
//! **Each key authorizes its own successor.** A handoff is signed by the
//! *outgoing* key, so the chain from the genesis forward is a standard forward
//! key-rollover. A verifier that trusts the genesis — and it can, because the
//! genesis *is* the account id — can therefore verify any later key with no
//! external input and no ordering dependency.
//!
//! Note what that does not give you: an attacker holding a stolen root key can
//! sign a handoff of their own. Recovering from root-key compromise needs a
//! separate recovery authority and is deliberately out of scope here — but
//! because `AccountId` is not the key, adding one later is a key-set change
//! rather than a new identity.
//!
//! **The walk stops at the epoch asked for.** Entries beyond it are neither read
//! nor verified. They are not part of the authorization the credential rests on,
//! so letting one refuse the whole credential would invalidate a certificate that
//! verifies perfectly against a key the chain genuinely established — over an
//! entry that decides nothing. It is also the difference between one Ed25519
//! verification and up to [`MAX_ROOT_KEY_HANDOFFS`] of them on a path any member
//! can drive; the cap bounds that work but does not avoid doing it.
//!
//! A handoff beyond the requested epoch is still worthless to whoever appended
//! it: the epoch it claims to establish is only reachable by asking for it, which
//! walks — and verifies — every entry up to it.
//!
//! **Minting goes through the same `signing_payload` the verifier uses.** A
//! hand-rolled payload that omits a field still produces a signature that
//! *verifies*, while silently leaving that field unauthenticated — and the
//! omission is invisible at the call site. Routing every signer through one
//! preimage builder makes that class of bug unexpressible.

use borsh::{BorshDeserialize, BorshSerialize};

use calimero_primitives::identity::{domain_hash, AccountId, PrivateKey, PublicKey};

use crate::account::{AccountGenesis, ACCOUNT_GENESIS_VERSION};
use crate::domain::HANDOFF_SIGN_DOMAIN;
use crate::error::AccountError;
use crate::signed::sign_payload;

/// Max root-key handoffs in one credential chain.
///
/// Each entry costs an Ed25519 verification in [`root_key_at_epoch`], on a path
/// reachable from untrusted bytes, so an uncapped chain is verification
/// amplification. Generous against real use: an account rotating its root key
/// daily would take over two years to reach it.
pub const MAX_ROOT_KEY_HANDOFFS: usize = 1_024;

/// Rolls an account's root key from epoch `from_epoch` to `from_epoch + 1`.
#[derive(Clone, Copy, Debug, Eq, PartialEq, BorshSerialize, BorshDeserialize)]
pub struct RootKeyHandoff {
    /// The account whose key is rolling. Bound into the signature so a handoff
    /// cannot be replayed onto a different account.
    pub account: AccountId,
    /// Epoch of the key that signs this handoff.
    pub from_epoch: u32,
    /// The incoming key, which becomes epoch `from_epoch + 1`.
    pub new_root_sign_pk: PublicKey,
    /// Signature by the epoch-`from_epoch` root key over
    /// [`RootKeyHandoff::signing_payload`].
    pub signature: [u8; 64],
}

impl RootKeyHandoff {
    /// Canonical bytes the outgoing root key signs. Covers every field except
    /// the signature itself.
    #[must_use]
    pub fn signing_payload(
        account: AccountId,
        from_epoch: u32,
        new_root_sign_pk: &PublicKey,
    ) -> [u8; 32] {
        domain_hash(
            HANDOFF_SIGN_DOMAIN,
            &[
                account.as_bytes(),
                &from_epoch.to_le_bytes(),
                new_root_sign_pk.as_ref(),
            ],
        )
    }

    /// The bytes this handoff's signature covers.
    #[must_use]
    pub fn payload(&self) -> [u8; 32] {
        Self::signing_payload(self.account, self.from_epoch, &self.new_root_sign_pk)
    }

    /// Mint a handoff, signed by the **outgoing** key.
    ///
    /// `from_epoch` must be the epoch of `current_root_sk`; the resulting handoff
    /// establishes `from_epoch + 1`.
    ///
    /// # Errors
    /// [`AccountError::SigningFailed`] if the key refuses to sign.
    pub fn sign(
        current_root_sk: &PrivateKey,
        account: AccountId,
        from_epoch: u32,
        new_root_sign_pk: &PublicKey,
    ) -> Result<Self, AccountError> {
        let payload = Self::signing_payload(account, from_epoch, new_root_sign_pk);
        Ok(Self {
            account,
            from_epoch,
            new_root_sign_pk: *new_root_sign_pk,
            signature: sign_payload(current_root_sk, &payload)?,
        })
    }
}

/// Walk a handoff chain from `genesis` and return the root key at `epoch`.
///
/// Epoch 0 is the genesis key, needing no chain at all; epoch `n` needs the first
/// `n` handoffs. The chain must start at epoch 0 and step by exactly one — a gap
/// would mean accepting a key whose authorization was never demonstrated, and a
/// repeat would make "the key at epoch n" ambiguous. See the module docs for why
/// entries past `epoch` are ignored rather than rejected.
///
/// # Errors
/// [`AccountError::UnsupportedVersion`] for an unknown genesis version,
/// [`AccountError::ChainTooLong`] past [`MAX_ROOT_KEY_HANDOFFS`],
/// [`AccountError::EpochOutOfRange`] when the chain is too short to reach
/// `epoch`, and the `Chain*` / `Handoff*` variants for a chain that is
/// discontinuous, addressed to another account, or not properly signed up to
/// `epoch`.
pub fn root_key_at_epoch(
    genesis: &AccountGenesis,
    chain: &[RootKeyHandoff],
    epoch: u32,
) -> Result<PublicKey, AccountError> {
    if genesis.version != ACCOUNT_GENESIS_VERSION {
        return Err(AccountError::UnsupportedVersion {
            found: genesis.version,
            supported: ACCOUNT_GENESIS_VERSION,
        });
    }

    // Cap before reading or verifying anything. Each entry costs an Ed25519
    // verification, and this function is reachable from the wire — the governance
    // path bounds the field at decode, but `calimero-op` has no bounds layer at
    // all, so the check has to exist here too rather than relying on every caller
    // to have one.
    if chain.len() > MAX_ROOT_KEY_HANDOFFS {
        return Err(AccountError::ChainTooLong {
            found: chain.len(),
            limit: MAX_ROOT_KEY_HANDOFFS,
        });
    }

    let needed = usize::try_from(epoch).unwrap_or(usize::MAX);
    if needed > chain.len() {
        // A chain long enough to overflow u32 cannot be held in memory.
        return Err(AccountError::EpochOutOfRange {
            key_epoch: epoch,
            reachable: u32::try_from(chain.len()).unwrap_or(u32::MAX),
        });
    }

    let account = genesis.account_id();
    let mut current = genesis.root_sign_pk;

    for (index, handoff) in chain.iter().take(needed).enumerate() {
        // `index` is bounded by the chain length, as above.
        let expected = u32::try_from(index).unwrap_or(u32::MAX);
        if handoff.from_epoch != expected {
            return Err(AccountError::ChainNotContiguous {
                expected,
                found: handoff.from_epoch,
            });
        }
        if handoff.account != account {
            return Err(AccountError::HandoffAccountMismatch { epoch: expected });
        }
        // Signed by the OUTGOING key — the one already established at this
        // position — which is what makes the chain an authorization chain
        // rather than a list of assertions.
        if current
            .verify_raw_signature(&handoff.payload(), &handoff.signature)
            .is_err()
        {
            return Err(AccountError::HandoffSignatureInvalid { epoch: expected });
        }
        current = handoff.new_root_sign_pk;
    }

    Ok(current)
}
