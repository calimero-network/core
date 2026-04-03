use calimero_context_client::local_governance::{
    EncryptedGroupOp, GroupOp, KeyEnvelope, KeyRotation,
};
use calimero_context_config::types::ContextGroupId;
use calimero_primitives::identity::{PrivateKey, PublicKey};
use calimero_store::key::{GroupKeyEntry, GroupKeyValue, GROUP_KEY_PREFIX};
use calimero_store::Store;
use eyre::{bail, Result as EyreResult};
use sha2::{Digest, Sha256};

use super::{collect_keys_with_prefix, list_group_members};

/// Domain API for managing encryption keys used by group governance ops.
pub struct GroupKeyring<'a> {
    store: &'a Store,
    group_id: ContextGroupId,
}

impl<'a> GroupKeyring<'a> {
    pub fn new(store: &'a Store, group_id: ContextGroupId) -> Self {
        Self { store, group_id }
    }

    pub fn group_id(&self) -> &ContextGroupId {
        &self.group_id
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

    /// Returns the latest key by `created_at` as `(key_id, group_key)`.
    pub fn load_current_key(&self) -> EyreResult<Option<([u8; 32], [u8; 32])>> {
        let gid = self.group_id.to_bytes();
        let keys = collect_keys_with_prefix(
            self.store,
            GroupKeyEntry::new(gid, [0u8; 32]),
            GROUP_KEY_PREFIX,
            |k| k.group_id() == gid,
        )?;
        let handle = self.store.handle();
        let mut best: Option<([u8; 32], [u8; 32], u64)> = None;

        for key in keys {
            let Some(val): Option<GroupKeyValue> = handle.get(&key)? else {
                continue;
            };
            let kid = key.key_id();
            if best.as_ref().is_none_or(|(_, _, ts)| val.created_at > *ts) {
                best = Some((kid, val.group_key, val.created_at));
            }
        }

        Ok(best.map(|(kid, gk, _)| (kid, gk)))
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
            .ok_or_else(|| eyre::eyre!("AES-GCM encryption failed"))?;

        Ok(EncryptedGroupOp { nonce, ciphertext })
    }

    pub fn decrypt_op(group_key: &[u8; 32], encrypted: &EncryptedGroupOp) -> EyreResult<GroupOp> {
        use calimero_crypto::SharedKey;

        let sk = PrivateKey::from(*group_key);
        let shared_key = SharedKey::from_sk(&sk);
        let plaintext = shared_key
            .decrypt(encrypted.ciphertext.clone(), encrypted.nonce)
            .ok_or_else(|| eyre::eyre!("failed to decrypt group op (bad sender_key or corrupt)"))?;
        borsh::from_slice(&plaintext).map_err(|e| eyre::eyre!("borsh decode inner GroupOp: {e}"))
    }

    pub fn wrap_for_member(
        sender_sk: &PrivateKey,
        recipient_pk: &PublicKey,
        group_key: &[u8; 32],
    ) -> EyreResult<KeyEnvelope> {
        use calimero_crypto::SharedKey;

        let shared = SharedKey::new(sender_sk, recipient_pk)
            .map_err(|e| eyre::eyre!("ECDH key agreement failed: {e:?}"))?;

        let nonce: [u8; 12] = {
            use rand::Rng;
            rand::thread_rng().gen()
        };

        let ciphertext = shared
            .encrypt(group_key.to_vec(), nonce)
            .ok_or_else(|| eyre::eyre!("AES-GCM encryption of group key failed"))?;

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

        let shared = SharedKey::new(recipient_sk, &envelope.ephemeral_pk)
            .map_err(|e| eyre::eyre!("ECDH key agreement failed: {e:?}"))?;

        let plaintext = shared
            .decrypt(envelope.ciphertext.clone(), envelope.nonce)
            .ok_or_else(|| eyre::eyre!("failed to decrypt key envelope (wrong recipient?)"))?;

        if plaintext.len() != 32 {
            bail!(
                "decrypted key envelope has wrong length: {}",
                plaintext.len()
            );
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
        let members = list_group_members(self.store, &self.group_id, 0, usize::MAX)?;
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

// Backward-compatible free-function facade
pub fn compute_key_id(group_key: &[u8; 32]) -> [u8; 32] {
    GroupKeyring::key_id_for(group_key)
}

pub fn store_group_key(
    store: &Store,
    group_id: &ContextGroupId,
    group_key: &[u8; 32],
) -> EyreResult<[u8; 32]> {
    GroupKeyring::new(store, *group_id).store_key(group_key)
}

pub fn load_group_key_by_id(
    store: &Store,
    group_id: &ContextGroupId,
    key_id: &[u8; 32],
) -> EyreResult<Option<[u8; 32]>> {
    GroupKeyring::new(store, *group_id).load_key_by_id(key_id)
}

pub fn load_current_group_key(
    store: &Store,
    group_id: &ContextGroupId,
) -> EyreResult<Option<([u8; 32], [u8; 32])>> {
    GroupKeyring::new(store, *group_id).load_current_key()
}

pub fn wrap_group_key_for_member(
    sender_sk: &PrivateKey,
    recipient_pk: &PublicKey,
    group_key: &[u8; 32],
) -> EyreResult<KeyEnvelope> {
    GroupKeyring::wrap_for_member(sender_sk, recipient_pk, group_key)
}

pub fn unwrap_group_key(recipient_sk: &PrivateKey, envelope: &KeyEnvelope) -> EyreResult<[u8; 32]> {
    GroupKeyring::unwrap_for_recipient(recipient_sk, envelope)
}

pub fn build_key_rotation(
    store: &Store,
    group_id: &ContextGroupId,
    new_group_key: &[u8; 32],
    sender_sk: &PrivateKey,
    excluded_member: Option<&PublicKey>,
) -> EyreResult<KeyRotation> {
    GroupKeyring::new(store, *group_id).build_rotation(new_group_key, sender_sk, excluded_member)
}

pub fn encrypt_group_op(group_key: &[u8; 32], op: &GroupOp) -> EyreResult<EncryptedGroupOp> {
    GroupKeyring::encrypt_op(group_key, op)
}
