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

    pub fn store_key(&self, group_key: &[u8; 32]) -> EyreResult<[u8; 32]> {
        let key_id = Self::key_id_for(group_key);
        let entry = GroupKeyEntry::new(self.group_id.to_bytes(), key_id);
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let value = GroupKeyValue {
            group_key: *group_key,
            created_at: now,
        };
        let mut handle = self.store.handle();
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

    /// Returns the latest key by `created_at`.
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
            if best.as_ref().is_none_or(|(_, ts)| val.created_at > *ts) {
                best = Some((current, val.created_at));
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

    pub fn wrap_for_member(
        sender_sk: &PrivateKey,
        recipient_pk: &PublicKey,
        group_key: &[u8; 32],
    ) -> EyreResult<KeyEnvelope> {
        use calimero_crypto::SharedKey;

        let shared = SharedKey::new(sender_sk, recipient_pk).map_err(|e| {
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

        Ok(KeyEnvelope {
            recipient: *recipient_pk,
            ephemeral_pk: sender_sk.public_key(),
            nonce,
            ciphertext,
        })
    }

    pub fn unwrap_for_recipient(
        recipient_sk: &PrivateKey,
        envelope: &KeyEnvelope,
    ) -> EyreResult<[u8; 32]> {
        use calimero_crypto::SharedKey;

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
        let new_key_id = Self::key_id_for(new_group_key);
        let mut envelopes = Vec::new();

        for (member_pk, _) in &members {
            if excluded_member == Some(member_pk) {
                continue;
            }
            envelopes.push(Self::wrap_for_member(sender_sk, member_pk, new_group_key)?);
        }

        Ok(KeyRotation {
            new_key_id,
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
}
