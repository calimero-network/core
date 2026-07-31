//! Encryption-key resolution and delta decryption for state-delta handling.
//!
//! Leaf helpers extracted from the state-delta handler: resolving a
//! delta's group encryption key (with a bounded wait for a late
//! `KeyDelivery`) and decrypting the borsh-encoded storage delta.

use calimero_crypto::Nonce;
use calimero_governance_store::{GroupKeyring, NamespaceRepository};
use calimero_node_primitives::sync::{SealedDeltaPayload, MAX_STATE_DELTA_PLAINTEXT_BYTES};
use calimero_primitives::hash::Hash;
use calimero_primitives::identity::PrivateKey;
use calimero_storage::action::Action;
use eyre::{bail, OptionExt, Result};
use tracing::debug;

/// Bounded wait for a `KeyDelivery` (carried by `NamespaceGovernanceDelta`)
/// to land before we give up on decrypting an inbound state delta.
/// Mirrors `KEY_DELIVERY_FALLBACK_WAIT` in
/// `crates/context/src/handlers/join_group.rs`.
///
/// Why: once StateDelta processing runs on its own Arbiter (issue
/// #2299), the race window where a delta wakes before its associated
/// `KeyDelivery` has been applied to the group store widens. Without
/// this short wait, the failure mode is "rely on the next 30s
/// heartbeat to trigger sync rebroadcast" — exactly the lull pattern
/// the actor isolation was meant to remove. Bounded at 3s so it can't
/// itself starve the actor's mailbox.
pub(super) const STATE_DELTA_KEY_LOOKUP_WAIT: std::time::Duration =
    std::time::Duration::from_secs(3);
const STATE_DELTA_KEY_LOOKUP_POLL: std::time::Duration = std::time::Duration::from_millis(100);

/// Resolve a state delta's encryption key for a given group, polling
/// the group store up to `max_wait` if the key hasn't landed yet.
/// Tries the direct group-id keyring first, then the namespace-id
/// keyring on `Open` subgroups (issue #2256).
///
/// Pass `Duration::ZERO` for a single-shot lookup (no polling). The
/// `replay_buffered_delta` path uses this — by the time replay runs,
/// snapshot sync has settled and any late `KeyDelivery` is already
/// applied; a stall there would multiply per-delta into multi-second
/// sync recovery delays.
///
/// Returns `Ok(Some(_))` on success, `Ok(None)` when the wait expires
/// without the key arriving, `Err(_)` on store errors.
pub(super) async fn lookup_group_key_with_wait(
    context_client: &calimero_context_client::client::ContextClient,
    group_id: &calimero_context_config::types::ContextGroupId,
    key_id: &[u8; 32],
    max_wait: std::time::Duration,
) -> Result<Option<calimero_primitives::identity::PrivateKey>> {
    use tokio::time::{sleep, Instant};

    // Explicit single-shot path: when max_wait is zero we want exactly
    // one lookup with no polling, regardless of the relationship
    // between max_wait and STATE_DELTA_KEY_LOOKUP_POLL. Without this,
    // single-shot semantics depend on POLL > 0, which is fragile.
    let single_shot = max_wait.is_zero();
    let deadline = Instant::now() + max_wait;
    let mut logged_wait = false;
    loop {
        // Scope the &Store borrow to a sub-block so it cannot be
        // mistaken for being held across the sleep below.
        let resolved = {
            let store = context_client.datastore();
            let direct = GroupKeyring::new(store, *group_id).load_key_by_id(key_id)?;
            match direct {
                Some(k) => Some(k),
                None => {
                    let ns_id = NamespaceRepository::new(store).resolve(group_id)?;
                    if &ns_id != group_id {
                        GroupKeyring::new(store, ns_id).load_key_by_id(key_id)?
                    } else {
                        None
                    }
                }
            }
        };

        if let Some(k) = resolved {
            return Ok(Some(calimero_primitives::identity::PrivateKey::from(k)));
        }

        if single_shot {
            return Ok(None);
        }

        // Stop before sleeping if the next poll wouldn't fit inside
        // the deadline — bounds wall-time at exactly `max_wait`
        // instead of `max_wait + STATE_DELTA_KEY_LOOKUP_POLL`.
        if Instant::now() + STATE_DELTA_KEY_LOOKUP_POLL > deadline {
            return Ok(None);
        }

        // Log on the first miss only — keeps the happy path silent
        // but makes a slow KeyDelivery race visible to operators.
        if !logged_wait {
            debug!(
                ?group_id,
                key_id = %hex::encode(key_id),
                wait_ms = max_wait.as_millis(),
                "Group key not yet available — polling for KeyDelivery"
            );
            logged_wait = true;
        }

        sleep(STATE_DELTA_KEY_LOOKUP_POLL).await;
    }
}

/// A decrypted state delta: the storage actions to apply, plus the expected
/// post-apply `root_hash` and the execution `events` that were sealed
/// alongside them.
#[derive(Debug)]
pub(super) struct DecryptedDelta {
    /// Expected state root after applying `actions`. Sealed inside the
    /// ciphertext (never on the wire in cleartext) and recovered here.
    pub(super) root_hash: Hash,
    /// The storage mutations carried by the delta.
    pub(super) actions: Vec<Action>,
    /// Serialized execution events for handler replay, sealed inside the
    /// ciphertext. `None` when the delta emitted no events.
    pub(super) events: Option<Vec<u8>>,
}

/// Decrypt a state delta's encrypted payload, returning the sealed
/// `root_hash` and the storage actions. The plaintext is a borsh-encoded
/// [`SealedDeltaPayload`] wrapping the root hash and the borsh-encoded
/// `StorageDelta` bytes.
pub(super) fn decrypt_delta_actions(
    artifact: Vec<u8>,
    nonce: Nonce,
    sender_key: PrivateKey,
) -> Result<DecryptedDelta> {
    let shared_key = calimero_crypto::SharedKey::from_sk(&sender_key);
    let decrypted = shared_key
        .decrypt(artifact, nonce)
        .ok_or_eyre("failed to decrypt delta payload")?;

    // AEAD proves the payload came from a group-key holder, but a *malicious
    // member* still holds the key and can seal an arbitrarily large artifact.
    // Bound the inner artifact via the type's own `is_valid()` contract before
    // deserializing the storage delta, so a crafted payload can't drive
    // unbounded borsh allocation. (Deserializing the outer `SealedDeltaPayload`
    // is itself bounded: `decrypted` came from an inbound gossip message capped
    // at gossipsub's transmit size, or from a previously network-bounded delta
    // on the buffered-replay path.)
    let sealed: SealedDeltaPayload = borsh::from_slice(&decrypted)?;
    if !sealed.is_valid() {
        bail!(
            "state-delta artifact too large: {} bytes (max {})",
            sealed.artifact.len(),
            MAX_STATE_DELTA_PLAINTEXT_BYTES
        );
    }

    let storage_delta: calimero_storage::delta::StorageDelta = borsh::from_slice(&sealed.artifact)?;

    match storage_delta {
        calimero_storage::delta::StorageDelta::Actions(actions) => Ok(DecryptedDelta {
            root_hash: sealed.root_hash,
            actions,
            events: sealed.events,
        }),
        // A `CausalActions` artifact carries the writer set that the Shared
        // verifier will authorize against. Accepting one off the wire would
        // hand that decision to whoever sealed it: a group-key holder names
        // itself a writer of any Shared object and its own signature then
        // verifies against its own set. This node resolves the writer set for
        // itself, from its own rotation log at the delta's causal cut, when it
        // re-wraps these actions for the guest — so the only variant that may
        // arrive here is the bare action list.
        calimero_storage::delta::StorageDelta::CausalActions { .. } => bail!(
            "state delta carried the CausalActions variant; a peer-supplied \
             writer set is never trusted"
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use calimero_crypto::{SharedKey, NONCE_LEN};
    use calimero_storage::delta::StorageDelta;
    use rand::thread_rng;

    #[test]
    fn decrypt_delta_actions_roundtrip() -> Result<()> {
        let mut rng = thread_rng();
        let sender_key = PrivateKey::random(&mut rng);
        let shared_key = SharedKey::from_sk(&sender_key);

        let storage_delta = StorageDelta::Actions(Vec::new());
        let sealed = SealedDeltaPayload {
            root_hash: Hash::from([9u8; 32]),
            artifact: borsh::to_vec(&storage_delta)?,
            events: Some(b"events-blob".to_vec()),
        };
        let plaintext = borsh::to_vec(&sealed)?;
        let (nonce, cipher) = shared_key
            .encrypt(plaintext)
            .ok_or_eyre("encryption failed")?;

        // Encrypted payload should decrypt back to empty actions AND the
        // sealed root hash AND the sealed events — proving all three survive
        // the round-trip inside the ciphertext rather than on the cleartext
        // wire.
        let decrypted = decrypt_delta_actions(cipher, nonce, sender_key)?;
        assert!(decrypted.actions.is_empty());
        assert_eq!(decrypted.root_hash, Hash::from([9u8; 32]));
        assert_eq!(decrypted.events.as_deref(), Some(b"events-blob".as_ref()));

        Ok(())
    }

    #[test]
    fn decrypt_delta_actions_rejects_bad_cipher() {
        let mut rng = thread_rng();
        let sender_key = PrivateKey::random(&mut rng);
        let nonce = [9u8; NONCE_LEN];

        // Garbage ciphertext should fail to decrypt/deserialize
        let result = decrypt_delta_actions(vec![1, 2, 3, 4], nonce, sender_key);
        assert!(result.is_err());
    }

    /// A group-key holder seals a `CausalActions` artifact whose writer set
    /// names only itself, for an entity it is not a writer of. Nothing
    /// downstream re-derives that set — `Root::sync` feeds it straight to the
    /// Shared verifier as the authoritative answer — so the wire boundary is
    /// where it has to die. If this ever returns `Ok`, any group member can
    /// write to any Shared object.
    #[test]
    fn decrypt_delta_actions_rejects_peer_supplied_writer_set() {
        use std::collections::{BTreeMap, BTreeSet};

        use calimero_storage::address::Id;
        use calimero_storage::entities::OpMask;
        use calimero_storage::logical_clock::HybridTimestamp;
        use calimero_storage::tests::common::{build_signed_shared_action, pubkey_of};
        use ed25519_dalek::SigningKey;

        let mut rng = thread_rng();
        let sender_key = PrivateKey::random(&mut rng);
        let shared_key = SharedKey::from_sk(&sender_key);

        // The attacker holds the group key (so AEAD proves nothing about
        // authorization here) and signs a write to someone else's entity.
        let attacker = SigningKey::from_bytes(&[7u8; 32]);
        let attacker_pk = pubkey_of(&attacker);
        let victim_entity = Id::new([42u8; 32]);
        let action = build_signed_shared_action(
            false,
            victim_entity,
            b"attacker-write".to_vec(),
            BTreeSet::from([attacker_pk]),
            1_000,
            &attacker,
            vec![],
        );

        let mut effective_writers = BTreeMap::new();
        let _ =
            effective_writers.insert(victim_entity, BTreeMap::from([(attacker_pk, OpMask::FULL)]));

        let storage_delta = StorageDelta::CausalActions {
            actions: vec![action],
            delta_id: [1u8; 32],
            delta_hlc: HybridTimestamp::default(),
            effective_writers,
        };
        let sealed = SealedDeltaPayload {
            root_hash: Hash::from([0u8; 32]),
            artifact: borsh::to_vec(&storage_delta).expect("serialize"),
            events: None,
        };
        let plaintext = borsh::to_vec(&sealed).expect("serialize");
        let (nonce, cipher) = shared_key.encrypt(plaintext).expect("encryption failed");

        let err = decrypt_delta_actions(cipher, nonce, sender_key)
            .expect_err("a wire-supplied writer set must be refused");
        assert!(
            err.to_string().contains("never trusted"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn decrypt_delta_actions_rejects_oversized_payload() {
        let mut rng = thread_rng();
        let sender_key = PrivateKey::random(&mut rng);
        let shared_key = SharedKey::from_sk(&sender_key);

        // A group-key holder seals a well-formed SealedDeltaPayload whose inner
        // artifact is just past the cap. The `is_valid()` guard must reject it
        // before the inner storage-delta deserialization is attempted.
        let sealed = SealedDeltaPayload {
            root_hash: Hash::from([0u8; 32]),
            artifact: vec![0u8; MAX_STATE_DELTA_PLAINTEXT_BYTES + 1],
            events: None,
        };
        let plaintext = borsh::to_vec(&sealed).expect("serialize");
        let (nonce, cipher) = shared_key.encrypt(plaintext).expect("encryption failed");

        let err = decrypt_delta_actions(cipher, nonce, sender_key)
            .expect_err("oversized payload must be rejected");
        assert!(
            err.to_string().contains("too large"),
            "unexpected error: {err}"
        );
    }
}
