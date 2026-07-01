use crate::{KeyringError, MembershipRepository};
use calimero_context_client::local_governance::{
    EncryptedGroupOp, GroupOp, KeyEnvelope, KeyRotation,
};
use calimero_context_config::types::ContextGroupId;
use calimero_primitives::identity::{PrivateKey, PublicKey};
use calimero_store::key::{GroupKeyEntry, GroupKeyValue, GROUP_KEY_PREFIX};
use calimero_store::Store;
use eyre::{bail, Result as EyreResult};
use sha2::{Digest, Sha256};

use super::collect_keys_with_prefix;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StoredGroupKey {
    pub key_id: [u8; 32],
    pub group_key: [u8; 32],
}

impl StoredGroupKey {
    pub fn into_tuple(self) -> ([u8; 32], [u8; 32]) {
        (self.key_id, self.group_key)
    }
}

/// Domain API for managing encryption keys used by group governance ops.
pub struct GroupKeyring<'a> {
    store: &'a Store,
    group_id: ContextGroupId,
}

impl<'a> GroupKeyring<'a> {
    pub fn new(store: &'a Store, group_id: ContextGroupId) -> Self {
        Self { store, group_id }
    }

    pub fn key_id_for(group_key: &[u8; 32]) -> [u8; 32] {
        let mut hasher = Sha256::new();
        hasher.update(group_key);
        hasher.finalize().into()
    }

    /// Store a genesis / bootstrap group key at `epoch = 0`.
    ///
    /// Use [`store_key_with_epoch`](Self::store_key_with_epoch) for a key
    /// introduced by a governance op (a rotation or an on-DAG delivery), whose
    /// deterministic DAG sequence is the epoch that decides which key is
    /// "current". A bare genesis key is always the oldest (`epoch 0`), so any
    /// later rotation deterministically supersedes it.
    pub fn store_key(&self, group_key: &[u8; 32]) -> EyreResult<[u8; 32]> {
        self.store_key_with_epoch(group_key, 0)
    }

    /// Store a group key stamped with an explicit deterministic `epoch` (the DAG
    /// sequence of the op that introduced it).
    ///
    /// The epoch is stored **monotonically and never lowered**: a write only
    /// touches the store when the entry is absent or when it strictly *raises*
    /// the epoch. This matters because `store_key` (epoch `0`) is also called
    /// outside the per-namespace governance-apply lock — from the direct-pull
    /// path (`apply_received_group_key`) and the join handlers — so an epoch-`0`
    /// write could otherwise race a rotation's epoch-`N` write and clobber it.
    /// Since a lowering (or equal) write is a no-op here, an epoch-`0` write can
    /// never overwrite a higher stored epoch regardless of interleaving; the
    /// only writes that hit the store are the absent-entry seed and genuine
    /// epoch increases (a rotation), and those never contend for the same
    /// `key_id` (`key_id = sha256(group_key)`, and each rotation mints a fresh
    /// key, so a given key_id has exactly one "real" epoch — two writers raising
    /// it to the same value are idempotent). Result: nodes converge on the same
    /// "current" key without needing an atomic read-modify-write.
    pub fn store_key_with_epoch(&self, group_key: &[u8; 32], epoch: u64) -> EyreResult<[u8; 32]> {
        let key_id = Self::key_id_for(group_key);
        let entry = GroupKeyEntry::new(self.group_id.to_bytes(), key_id);
        let mut handle = self.store.handle();
        // Only write when absent or strictly raising the epoch — a lower/equal
        // epoch (e.g. an epoch-0 pull for a key a rotation already stored) is a
        // no-op, so it can never regress a higher stored epoch even under a
        // racing interleave with a concurrent writer.
        if let Some(existing) = handle.get(&entry)? {
            let existing: GroupKeyValue = existing;
            if epoch <= existing.epoch {
                return Ok(key_id);
            }
        }
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let value = GroupKeyValue {
            group_key: *group_key,
            created_at: now,
            epoch,
        };
        handle.put(&entry, &value)?;
        Ok(key_id)
    }

    pub fn load_key_by_id(&self, key_id: &[u8; 32]) -> EyreResult<Option<[u8; 32]>> {
        let entry = GroupKeyEntry::new(self.group_id.to_bytes(), *key_id);
        let handle = self.store.handle();
        Ok(handle.get(&entry)?.map(|v: GroupKeyValue| v.group_key))
    }

    /// Delete a single stored group key by its `key_id`. Idempotent (a missing
    /// entry is a no-op). Unlike [`Self::delete_all_for_group`] this does NOT
    /// require the membership-removed purge precondition, because it targets one
    /// caller-named key — its sole use is the create-group rollback path
    /// (#2474), which deletes the exact key it just stored when a namespace-root
    /// genesis apply fails, so the partially-written root is cleanly absent and
    /// a retry with the same group id succeeds.
    pub fn delete_key_by_id(&self, key_id: &[u8; 32]) -> EyreResult<()> {
        let entry = GroupKeyEntry::new(self.group_id.to_bytes(), *key_id);
        let mut handle = self.store.handle();
        handle.delete(&entry)?;
        Ok(())
    }

    /// Returns the "current" key: the one with the highest deterministic
    /// `epoch` (the DAG sequence of the op that introduced it), breaking ties by
    /// the larger `key_id`. This is fully deterministic across nodes — unlike
    /// the old wall-clock `created_at` ordering, two rotations within the same
    /// second or a skewed clock can no longer make two nodes pick different
    /// "current" keys (which caused decrypt divergence).
    pub fn load_current_key_record(&self) -> EyreResult<Option<StoredGroupKey>> {
        let gid = self.group_id.to_bytes();
        let keys = collect_keys_with_prefix(
            self.store,
            GroupKeyEntry::new(gid, [0u8; 32]),
            GROUP_KEY_PREFIX,
            |k| k.group_id() == gid,
        )?;
        let handle = self.store.handle();
        let mut best: Option<(StoredGroupKey, u64)> = None;

        for key in keys {
            let Some(val): Option<GroupKeyValue> = handle.get(&key)? else {
                continue;
            };
            let current = StoredGroupKey {
                key_id: key.key_id(),
                group_key: val.group_key,
            };
            let better = match best.as_ref() {
                None => true,
                Some((best_rec, best_epoch)) => {
                    val.epoch > *best_epoch
                        || (val.epoch == *best_epoch && current.key_id > best_rec.key_id)
                }
            };
            if better {
                best = Some((current, val.epoch));
            }
        }

        Ok(best.map(|(record, _)| record))
    }

    /// Delete every stored group encryption key (`GroupKeyEntry`) for this
    /// group. Used by the purge/leave cascade for forward-secrecy hygiene —
    /// mirrors `SigningKeysRepository::delete_all_for_group` (the group id is
    /// taken from `self` rather than a parameter, since the keyring is already
    /// scoped to one group). Idempotent.
    ///
    /// Correctness relies on `GroupKeyEntry` keys being ordered by
    /// `(group_id, key_id)`, so all of this group's keys are contiguous and the
    /// prefix scan collects them in a single pass — the same ordering
    /// assumption as [`load_current_key_record`](Self::load_current_key_record).
    ///
    /// The scan and the deletes use separate store handles, so this is **not**
    /// atomic. Two windows follow from that, both benign here:
    ///
    /// 1. *Concurrent writer.* A `store_key` racing between the scan and the
    ///    delete loop would be missed. This cannot happen on the purge path:
    ///    the only writer of `GroupKeyEntry` is the governance key-delivery /
    ///    rotation pipeline, which only writes for groups the node is a member
    ///    of, and `delete_group_local_rows` removes the membership rows *before*
    ///    calling this — and the cascade itself runs single-threaded. So no
    ///    `store_key` for this group can be issued once we reach here. The
    ///    method is `pub(crate)` precisely so this precondition is enforced
    ///    structurally: the only caller is `delete_group_local_rows` (and the
    ///    in-crate tests), never an external code path that might skip the
    ///    membership removal.
    /// 2. *Partial delete on error.* If a `handle.delete` fails mid-loop, the
    ///    already-deleted keys stay deleted and the rest remain; the error
    ///    propagates via `?`. The caller (`delete_group_local_rows`) propagates
    ///    it too, keeping the purge retry anchor alive, and the next reconcile
    ///    invocation re-scans and deletes only the survivors — idempotent across
    ///    retries even after a partial delete.
    pub(crate) fn delete_all_for_group(&self) -> EyreResult<()> {
        let gid = self.group_id.to_bytes();
        let keys = collect_keys_with_prefix(
            self.store,
            GroupKeyEntry::new(gid, [0u8; 32]),
            GROUP_KEY_PREFIX,
            |k| k.group_id() == gid,
        )?;
        let mut handle = self.store.handle();
        for key in keys {
            handle.delete(&key)?;
        }
        Ok(())
    }

    /// Backward-compatible tuple view of [`StoredGroupKey`].
    pub fn load_current_key(&self) -> EyreResult<Option<([u8; 32], [u8; 32])>> {
        Ok(self
            .load_current_key_record()?
            .map(StoredGroupKey::into_tuple))
    }

    /// Cheap existence check: does this group's keyring hold **any**
    /// [`GroupKeyEntry`] at all (current or rotated-out)?
    ///
    /// Unlike [`load_current_key_record`](Self::load_current_key_record), which
    /// scans every key for this group to pick the newest by `created_at`, this
    /// stops at the first matching key — it mirrors that method's
    /// prefix-ordering assumption (all of this group's keys are contiguous after
    /// the seek) but returns `true` on the first hit and never reads a value.
    ///
    /// Used to gate the `GroupCreated` re-drive (#2848): a retry resolves each
    /// buffered op by its `key_id` via [`load_key_by_id`](Self::load_key_by_id),
    /// so the correct gate is "the keyring is non-empty" (if the matching key is
    /// held, the keyring is necessarily non-empty), **not** "the *current* key
    /// is held" — after a rotation a node may hold only the OLD key that a
    /// buffered op was encrypted under, which `load_current_key` would miss.
    pub fn holds_any_key(&self) -> EyreResult<bool> {
        let gid = self.group_id.to_bytes();
        let handle = self.store.handle();
        let mut iter = handle.iter::<GroupKeyEntry>()?;
        let start = GroupKeyEntry::new(gid, [0u8; 32]);
        if let Some(key) = iter.seek(start).transpose() {
            let key = key?;
            // `GroupKeyEntry` keys are ordered `(prefix, group_id, key_id)`, so
            // the first key at/after the seek that still belongs to this group
            // means the keyring is non-empty.
            if key.group_id() == gid {
                return Ok(true);
            }
        }
        Ok(false)
    }

    pub fn encrypt_op(group_key: &[u8; 32], op: &GroupOp) -> EyreResult<EncryptedGroupOp> {
        use calimero_crypto::SharedKey;

        let plaintext = borsh::to_vec(op).map_err(|e| eyre::eyre!("borsh encode GroupOp: {e}"))?;
        let sk = PrivateKey::from(*group_key);
        let shared_key = SharedKey::from_sk(&sk);

        let nonce: [u8; 12] = {
            use rand::Rng;
            rand::thread_rng().gen()
        };

        let ciphertext = shared_key
            .encrypt(plaintext, nonce)
            .ok_or(KeyringError::EncryptionFailed)?;

        Ok(EncryptedGroupOp { nonce, ciphertext })
    }

    pub fn decrypt_op(group_key: &[u8; 32], encrypted: &EncryptedGroupOp) -> EyreResult<GroupOp> {
        use calimero_crypto::SharedKey;

        let sk = PrivateKey::from(*group_key);
        let shared_key = SharedKey::from_sk(&sk);
        let plaintext = shared_key
            .decrypt(encrypted.ciphertext.clone(), encrypted.nonce)
            .ok_or(KeyringError::DecryptionFailed)?;
        borsh::from_slice(&plaintext).map_err(|e| {
            // "Unexpected length of input" on this path means the decrypted
            // plaintext length does not match the current `GroupOp` borsh
            // schema — almost always a cross-version schema drift where an
            // older node wrote an op shape the current node can't decode.
            // Log the plaintext length + prefix so the failing op type can
            // be identified and either forward-migrated or skipped.
            tracing::warn!(
                plaintext_len = plaintext.len(),
                plaintext_prefix = %hex::encode(&plaintext[..plaintext.len().min(32)]),
                error = %e,
                "borsh decode inner GroupOp failed (codec/schema mismatch)"
            );
            KeyringError::InnerOpDecodeFailed(format!("{e}")).into()
        })
    }

    /// Wrap `group_key` for `recipient_pk`, authenticated by `sender_sk` and
    /// bound to `group_id`.
    ///
    /// Forward secrecy: a fresh ephemeral keypair is generated per call and the
    /// ECDH secret is derived from `SharedKey::new(ephemeral_sk, recipient_pk)`,
    /// so a later compromise of `sender_sk` does not decrypt this envelope.
    /// Authentication: `sender_sk` signs the canonical envelope bytes (see
    /// [`KeyEnvelope::signing_payload`]) so a recipient can verify who wrapped
    /// the key and reject forged / cross-group-replayed envelopes.
    pub fn wrap_for_member(
        sender_sk: &PrivateKey,
        recipient_pk: &PublicKey,
        group_id: &[u8; 32],
        group_key: &[u8; 32],
    ) -> EyreResult<KeyEnvelope> {
        use calimero_crypto::SharedKey;

        // Per-envelope ephemeral keypair — the source of forward secrecy.
        let ephemeral_sk = PrivateKey::random(&mut rand::thread_rng());
        let ephemeral_pk = ephemeral_sk.public_key();

        let shared = SharedKey::new(&ephemeral_sk, recipient_pk).map_err(|e| {
            KeyringError::KeyAgreementFailed {
                details: format!("{e:?}"),
            }
        })?;

        let nonce: [u8; 12] = {
            use rand::Rng;
            rand::thread_rng().gen()
        };

        let ciphertext = shared
            .encrypt(group_key.to_vec(), nonce)
            .ok_or(KeyringError::EncryptionFailed)?;

        let sender = sender_sk.public_key();
        let payload = KeyEnvelope::signing_payload(
            group_id,
            recipient_pk,
            &sender,
            &ephemeral_pk,
            &nonce,
            &ciphertext,
        );
        let signature = sender_sk
            .sign(&payload)
            .map_err(|e| KeyringError::EnvelopeAuthFailed(format!("sign: {e}")))?
            .to_bytes();

        Ok(KeyEnvelope {
            recipient: *recipient_pk,
            sender,
            ephemeral_pk,
            nonce,
            ciphertext,
            signature,
        })
    }

    /// Unwrap a [`KeyEnvelope`] addressed to `recipient_sk`, verifying the
    /// sender's authenticating signature (bound to `group_id`) before
    /// decrypting.
    ///
    /// When `expected_sender` is `Some`, the envelope's `sender` must equal it —
    /// callers that know who is authorized to wrap (e.g. the admin who authored
    /// a rotation) pass it to reject an otherwise-valid envelope minted by the
    /// wrong identity. On success the (verified) sender is available on the
    /// returned key's envelope; the raw group key is returned.
    pub fn unwrap_for_recipient(
        recipient_sk: &PrivateKey,
        group_id: &[u8; 32],
        expected_sender: Option<&PublicKey>,
        envelope: &KeyEnvelope,
    ) -> EyreResult<[u8; 32]> {
        use calimero_crypto::SharedKey;

        if let Some(expected) = expected_sender {
            if envelope.sender != *expected {
                bail!(KeyringError::EnvelopeAuthFailed(format!(
                    "sender {} is not the required {expected}",
                    envelope.sender
                )));
            }
        }

        // Authenticate the sender before doing any ECDH/decrypt work: a forged
        // or cross-group-replayed envelope fails here.
        let payload = KeyEnvelope::signing_payload(
            group_id,
            &envelope.recipient,
            &envelope.sender,
            &envelope.ephemeral_pk,
            &envelope.nonce,
            &envelope.ciphertext,
        );
        envelope
            .sender
            .verify_raw_signature(&payload, &envelope.signature)
            .map_err(|e| KeyringError::EnvelopeAuthFailed(format!("verify: {e}")))?;

        let shared = SharedKey::new(recipient_sk, &envelope.ephemeral_pk).map_err(|e| {
            KeyringError::KeyAgreementFailed {
                details: format!("{e:?}"),
            }
        })?;

        let plaintext = shared
            .decrypt(envelope.ciphertext.clone(), envelope.nonce)
            .ok_or(KeyringError::DecryptionFailed)?;

        if plaintext.len() != 32 {
            bail!(KeyringError::BadKeyLength(plaintext.len()));
        }

        let mut key = [0u8; 32];
        key.copy_from_slice(&plaintext);
        Ok(key)
    }

    pub fn build_rotation(
        &self,
        new_group_key: &[u8; 32],
        sender_sk: &PrivateKey,
        excluded_member: Option<&PublicKey>,
    ) -> EyreResult<KeyRotation> {
        let members = MembershipRepository::new(self.store).list(&self.group_id, 0, usize::MAX)?;
        let group_id = self.group_id.to_bytes();
        let new_key_id = Self::key_id_for(new_group_key);
        let mut envelopes = Vec::new();

        for (member_pk, _) in &members {
            if excluded_member == Some(member_pk) {
                continue;
            }
            envelopes.push(Self::wrap_for_member(
                sender_sk,
                member_pk,
                &group_id,
                new_group_key,
            )?);
        }

        Ok(KeyRotation {
            new_key_id: new_key_id.into(),
            envelopes,
        })
    }
}

#[cfg(test)]
mod delete_tests {
    use std::sync::Arc;

    use calimero_store::db::InMemoryDB;

    use super::*;

    fn test_store() -> Store {
        Store::new(Arc::new(InMemoryDB::owned()))
    }

    #[test]
    fn delete_all_for_group_removes_all_keys_and_is_scoped() {
        let store = test_store();
        let gid = ContextGroupId::from([0x42u8; 32]);
        let ring = GroupKeyring::new(&store, gid);

        let id1 = ring.store_key(&[0x01u8; 32]).unwrap();
        let _id2 = ring.store_key(&[0x02u8; 32]).unwrap();
        assert!(ring.load_current_key().unwrap().is_some());
        assert!(ring.load_key_by_id(&id1).unwrap().is_some());

        // Seed a different group; it must survive the targeted delete.
        let other = ContextGroupId::from([0x99u8; 32]);
        let other_ring = GroupKeyring::new(&store, other);
        let _ = other_ring.store_key(&[0x03u8; 32]).unwrap();

        ring.delete_all_for_group().unwrap();

        assert!(
            ring.load_current_key().unwrap().is_none(),
            "all group encryption keys for the target group must be gone"
        );
        assert!(ring.load_key_by_id(&id1).unwrap().is_none());
        assert!(
            other_ring.load_current_key().unwrap().is_some(),
            "another group's keys must NOT be deleted"
        );

        // Idempotent: deleting again is a no-op.
        ring.delete_all_for_group().unwrap();
    }

    #[test]
    fn holds_any_key_detects_presence_emptiness_and_old_rotated_key() {
        let store = test_store();
        let gid = ContextGroupId::from([0x42u8; 32]);
        let ring = GroupKeyring::new(&store, gid);

        // Empty keyring.
        assert!(!ring.holds_any_key().unwrap());
        assert!(ring.load_current_key().unwrap().is_none());

        // Store an OLD key, then a NEW key (later `created_at`). After a
        // rotation a node may hold both; `load_current_key` resolves to the
        // newest, but `holds_any_key` only cares that the ring is non-empty —
        // which is exactly the property the GroupCreated re-drive gate (W3/S1)
        // needs, since the retry resolves a buffered op by its `key_id`
        // (possibly the OLD key) and not by "is current".
        let old_id = ring.store_key(&[0x01u8; 32]).unwrap();
        let _new_id = ring.store_key(&[0x02u8; 32]).unwrap();
        assert!(ring.holds_any_key().unwrap());
        assert!(
            ring.load_key_by_id(&old_id).unwrap().is_some(),
            "old rotated-out key is still resolvable by its key_id"
        );

        // Scoping: another group's key must not make this ring report present.
        let empty_gid = ContextGroupId::from([0x77u8; 32]);
        let empty_ring = GroupKeyring::new(&store, empty_gid);
        assert!(
            !empty_ring.holds_any_key().unwrap(),
            "holds_any_key must be scoped to its own group_id"
        );

        // After clearing, the ring is empty again.
        ring.delete_all_for_group().unwrap();
        assert!(!ring.holds_any_key().unwrap());
    }

    #[test]
    fn envelope_roundtrips_and_authenticates_sender() {
        let group_id = [0x11u8; 32];
        let group_key = [0x22u8; 32];
        let sender = PrivateKey::from([0x01u8; 32]);
        let recipient = PrivateKey::from([0x02u8; 32]);

        let env =
            GroupKeyring::wrap_for_member(&sender, &recipient.public_key(), &group_id, &group_key)
                .unwrap();

        // Sender is authenticated, and the ephemeral key is NOT the sender's
        // long-term key (forward secrecy).
        assert_eq!(env.sender, sender.public_key());
        assert_ne!(env.ephemeral_pk, sender.public_key());

        // Round-trips for the addressed recipient.
        assert_eq!(
            GroupKeyring::unwrap_for_recipient(&recipient, &group_id, None, &env).unwrap(),
            group_key
        );

        // `expected_sender` is enforced.
        assert!(GroupKeyring::unwrap_for_recipient(
            &recipient,
            &group_id,
            Some(&sender.public_key()),
            &env
        )
        .is_ok());
        let wrong = PrivateKey::from([0x09u8; 32]).public_key();
        assert!(
            GroupKeyring::unwrap_for_recipient(&recipient, &group_id, Some(&wrong), &env).is_err()
        );
    }

    #[test]
    fn envelope_rejects_tamper_forgery_and_cross_group_replay() {
        let group_id = [0x11u8; 32];
        let group_key = [0x22u8; 32];
        let sender = PrivateKey::from([0x01u8; 32]);
        let recipient = PrivateKey::from([0x02u8; 32]);
        let env =
            GroupKeyring::wrap_for_member(&sender, &recipient.public_key(), &group_id, &group_key)
                .unwrap();

        // Replaying the envelope under a different group_id fails: the
        // signature is bound to the group.
        let other_group = [0x33u8; 32];
        assert!(GroupKeyring::unwrap_for_recipient(&recipient, &other_group, None, &env).is_err());

        // A flipped signature byte fails verification.
        let mut tampered = env.clone();
        tampered.signature[0] ^= 0xFF;
        assert!(
            GroupKeyring::unwrap_for_recipient(&recipient, &group_id, None, &tampered).is_err()
        );

        // Claiming a different sender (without a matching signature) fails.
        let mut spoofed = env.clone();
        spoofed.sender = PrivateKey::from([0x07u8; 32]).public_key();
        assert!(GroupKeyring::unwrap_for_recipient(&recipient, &group_id, None, &spoofed).is_err());
    }

    #[test]
    fn current_key_selected_by_epoch_then_key_id() {
        let store = test_store();
        let gid = ContextGroupId::from([0x42u8; 32]);
        let ring = GroupKeyring::new(&store, gid);

        // Higher epoch wins regardless of key bytes.
        let old = [0x01u8; 32];
        let new = [0x02u8; 32];
        ring.store_key_with_epoch(&old, 5).unwrap();
        ring.store_key_with_epoch(&new, 9).unwrap();
        assert_eq!(
            ring.load_current_key_record().unwrap().unwrap().group_key,
            new
        );

        // Epoch is monotonic: re-storing `new` at a LOWER epoch keeps epoch 9,
        // so `new` is still current.
        ring.store_key_with_epoch(&new, 0).unwrap();
        assert_eq!(
            ring.load_current_key_record().unwrap().unwrap().group_key,
            new
        );
    }

    #[test]
    fn current_key_breaks_equal_epoch_tie_by_key_id_deterministically() {
        let store = test_store();
        let gid = ContextGroupId::from([0x43u8; 32]);
        let ring = GroupKeyring::new(&store, gid);

        let a = [0x01u8; 32];
        let b = [0x02u8; 32];
        ring.store_key_with_epoch(&a, 7).unwrap();
        ring.store_key_with_epoch(&b, 7).unwrap();

        // Deterministic tie-break: the larger key_id wins on every node.
        let expected = if GroupKeyring::key_id_for(&a) > GroupKeyring::key_id_for(&b) {
            a
        } else {
            b
        };
        assert_eq!(
            ring.load_current_key_record().unwrap().unwrap().group_key,
            expected
        );
    }
}
