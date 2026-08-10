use crate::{
    AccountBindingRepository, DeviceSecret, KeyringError, MembershipRepository, NamespaceRepository,
};
use calimero_account::{AccountId, DeviceId, KemPublicKey};
use calimero_context_client::local_governance::{
    EncryptedGroupOp, EnvelopeRecipient, GroupOp, KeyEnvelope, KeyRotation,
};
use calimero_context_config::types::ContextGroupId;
use calimero_crypto::{X25519PublicKey, X25519SecretKey};
use calimero_primitives::identity::{PrivateKey, PublicKey};
use calimero_store::key::{GroupKeyEntry, GroupKeyValue, GROUP_KEY_PREFIX};
use calimero_store::Store;
use eyre::{bail, Result as EyreResult};
use sha2::{Digest, Sha256};

use super::collect_keys_with_prefix;

/// One addressee of a scope-key delivery.
///
/// Mirrors [`EnvelopeRecipient`] but carries what the *sender* needs rather than
/// what travels on the wire: the device variant holds the recipient's KEM public
/// key, which is read from the folded binding and deliberately never put on an
/// envelope.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum KeyRecipient {
    /// A member addressed by namespace identity — the bootstrap form.
    Member(PublicKey),
    /// One device of one account.
    Device {
        /// The device to address.
        device: DeviceId,
        /// Its certified X25519 key, from the binding row.
        kem_pk: X25519PublicKey,
    },
}

impl KeyRecipient {
    /// The device this delivery is for, if it is device-addressed.
    #[must_use]
    pub const fn device(&self) -> Option<DeviceId> {
        match *self {
            Self::Device { device, .. } => Some(device),
            Self::Member(_) => None,
        }
    }
}

/// Who is asking for a scope key on the pull path.
///
/// The two halves travel together and must be interpreted together — device
/// first, identity only while the group knows no account for it — so they are one
/// value. Passing them separately invites a caller to use the identity alone,
/// which is precisely the bug that let a revoked device pull its key back.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct KeyRequester {
    /// The requester's namespace identity, used for the membership check.
    pub identity: PublicKey,
    /// The device it claims to be, when it has enrolled one.
    ///
    /// Unauthenticated by design: the reply is sealed to that device's certified
    /// X25519 key, so a false claim yields an envelope the caller cannot open.
    pub device: Option<DeviceId>,
}

/// A [`KeyRecipient`] together with the member whose entitlement it rests on.
///
/// The pairing is what lets a caller exclude a member and have every one of that
/// member's devices go with them. Excluding by recipient alone could only drop
/// the identity-addressed entry, silently leaving the removed member's devices
/// holding the fresh key — the exact failure the exclusion exists to prevent.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EntitledRecipient {
    /// The group member this delivery is on behalf of, named by account —
    /// the same principal the membership row is keyed by, so a caller
    /// excluding someone (a rotation dropping the departed) excludes every
    /// device they hold rather than only the one that happened to be listed.
    pub member: AccountId,
    /// Where the key actually goes.
    pub recipient: KeyRecipient,
}

/// Serializes the read-check-write in [`GroupKeyring::store_key_with_epoch`]
/// across all callers (governance apply and the sync-task pull/join paths),
/// making the epoch monotonicity atomic without a store-level compare-and-swap.
static GROUP_KEY_EPOCH_WRITE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

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
    /// the epoch. A lower/equal write (e.g. an epoch-`0` pull for a key a
    /// rotation already stored) is a no-op.
    ///
    /// The read-check-write is serialized by a process-global lock. The store
    /// layer has no compare-and-swap, and `store_key` (epoch `0`) is called from
    /// the direct-pull path (`apply_received_group_key`) and join handlers, which
    /// run on sync tasks *outside* the per-namespace governance-apply actor lock.
    /// Without serialization an epoch-`0` write and a rotation's epoch-`N` write
    /// for the same fresh `key_id` could both observe an absent entry and the
    /// epoch-`0` put could land last, regressing the stored epoch to `0` and
    /// making the node pick a stale "current" key (the very divergence the epoch
    /// exists to prevent). The lock makes the check-then-write atomic.
    ///
    /// The lock is sufficient without store-level snapshot isolation because a
    /// base-`Store` `handle.put` is **write-through** to the shared DB (it calls
    /// `db.put` directly; the layer's `commit` is a no-op, so there is no
    /// per-handle write buffer). The write is therefore durable and visible the
    /// instant `put` returns — before the lock is released — so the next lock
    /// holder's freshly-opened `handle.get` always observes it. Correctness does
    /// **not** depend on when `handle` is dropped. A single process-global lock
    /// (rather than a per-group lock) is deliberate: group-key writes are rare
    /// (rotations / deliveries), never on a hot path, so cross-group contention
    /// is negligible and not worth a per-key lock map.
    pub fn store_key_with_epoch(&self, group_key: &[u8; 32], epoch: u64) -> EyreResult<[u8; 32]> {
        let key_id = Self::key_id_for(group_key);
        let entry = GroupKeyEntry::new(self.group_id.to_bytes(), key_id);
        // Serialize the read-modify-write; recover a poisoned lock (the guarded
        // state lives in the store, not the guard, so a prior panic is benign).
        let _guard = GROUP_KEY_EPOCH_WRITE_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut handle = self.store.handle();
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
    ///
    /// The `key_id` tie-break is what makes two *concurrent* rotations (e.g. two
    /// admins removing members on causally-unordered ops that land at the same
    /// epoch) **converge** rather than diverge: once both keys are present, every
    /// node picks the same one (larger `key_id` wins — a total order over a
    /// sha256 hash). And the choice is safety-neutral either way, because both
    /// concurrent rotations exclude the removed member(s) from their envelopes,
    /// so whichever key wins, a removed member holds neither.
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

        let (nonce, ciphertext) = shared_key
            .encrypt(plaintext)
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

    /// Wrap `group_key` for `recipient_pk`'s namespace identity, authenticated by
    /// `sender_sk` and bound to `group_id`.
    ///
    /// This is the **bootstrap** wrap. It stays member-addressed because a node
    /// with no scope key cannot yet publish the encrypted `GroupOp` that would
    /// enroll a device, so nothing device-addressed could ever reach it first.
    /// Once a member holds a key and has enrolled a device,
    /// [`wrap_for_device`](Self::wrap_for_device) is what delivers every
    /// subsequent one.
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
        let recipient = EnvelopeRecipient::Member {
            identity: *recipient_pk,
            ephemeral_pk: ephemeral_sk.public_key(),
        };

        let shared = SharedKey::new(&ephemeral_sk, recipient_pk).map_err(|e| {
            KeyringError::KeyAgreementFailed {
                details: format!("{e:?}"),
            }
        })?;

        Self::seal(sender_sk, recipient, group_id, group_key, &shared)
    }

    /// Wrap `group_key` for one device, under the X25519 key its certificate
    /// published.
    ///
    /// `device_kem_pk` must come from the folded device binding, never from the
    /// wire. That is the whole security argument for this function: the row that
    /// supplies the key is the same row that says the device is still
    /// authorized, so a revoked device's key is simply not available to wrap
    /// with — the exclusion cannot be forgotten separately from the lookup.
    ///
    /// Uses native X25519 rather than the Ed25519-identity agreement
    /// [`wrap_for_member`](Self::wrap_for_member) performs: a device's signing
    /// key and its KEM key are separate keys with separate lifetimes, which is
    /// what lets a device rotate one without invalidating deliveries under the
    /// other.
    pub fn wrap_for_device(
        sender_sk: &PrivateKey,
        device: DeviceId,
        device_kem_pk: &X25519PublicKey,
        group_id: &[u8; 32],
        group_key: &[u8; 32],
    ) -> EyreResult<KeyEnvelope> {
        use calimero_crypto::SharedKey;

        let ephemeral_sk = X25519SecretKey::random(&mut rand::thread_rng());
        let recipient = EnvelopeRecipient::Device {
            device,
            ephemeral_pk: KemPublicKey::from(*ephemeral_sk.public_key().as_bytes()),
        };

        let shared = SharedKey::from_x25519(&ephemeral_sk, device_kem_pk).map_err(|e| {
            KeyringError::KeyAgreementFailed {
                details: format!("{e:?}"),
            }
        })?;

        Self::seal(sender_sk, recipient, group_id, group_key, &shared)
    }

    /// Seal and sign, shared by both wrap modes.
    ///
    /// Only the agreement differs between them; everything after it — the AEAD
    /// seal, the canonical payload, the sender signature — must be byte-identical
    /// or the two would drift into subtly different authentication guarantees.
    fn seal(
        sender_sk: &PrivateKey,
        recipient: EnvelopeRecipient,
        group_id: &[u8; 32],
        group_key: &[u8; 32],
        shared: &calimero_crypto::SharedKey,
    ) -> EyreResult<KeyEnvelope> {
        let (nonce, ciphertext) = shared
            .encrypt(group_key.to_vec())
            .ok_or(KeyringError::EncryptionFailed)?;

        let sender = sender_sk.public_key();
        let payload =
            KeyEnvelope::signing_payload(group_id, &recipient, &sender, &nonce, &ciphertext);
        let signature = sender_sk
            .sign(&payload)
            .map_err(|e| KeyringError::EnvelopeAuthFailed(format!("sign: {e}")))?
            .to_bytes();

        Ok(KeyEnvelope {
            recipient,
            sender,
            nonce,
            ciphertext,
            signature,
        })
    }

    /// Unwrap a member-addressed [`KeyEnvelope`] with `recipient_sk`, verifying
    /// the sender's authenticating signature (bound to `group_id`) before
    /// decrypting.
    ///
    /// When `expected_sender` is `Some`, the envelope's `sender` must equal it —
    /// callers that know who is authorized to wrap (e.g. the admin who authored
    /// a rotation) pass it to reject an otherwise-valid envelope minted by the
    /// wrong identity.
    ///
    /// A device-addressed envelope is refused here rather than silently ignored:
    /// the two agreements are not interchangeable, so reaching this function with
    /// one is a caller bug, and [`unwrap_for_device`](Self::unwrap_for_device)
    /// (or [`unwrap_any`](Self::unwrap_any)) is what handles it.
    pub fn unwrap_for_recipient(
        recipient_sk: &PrivateKey,
        group_id: &[u8; 32],
        expected_sender: Option<&PublicKey>,
        envelope: &KeyEnvelope,
    ) -> EyreResult<[u8; 32]> {
        use calimero_crypto::SharedKey;

        Self::check_sender(expected_sender, envelope)?;

        // Cheap identity gate first: the envelope must be addressed to us. This
        // is checked against our own key (not a value we have to trust from the
        // envelope), so it is safe to do before signature verification and gives
        // a clear "wrong recipient" error instead of a downstream
        // `DecryptionFailed` from ECDH-ing with the wrong key. Callers already
        // filter by `recipient`; this is defense in depth.
        let EnvelopeRecipient::Member {
            identity,
            ephemeral_pk,
        } = envelope.recipient
        else {
            bail!(KeyringError::EnvelopeAuthFailed(
                "envelope is device-addressed; it cannot be opened with an identity key".to_owned()
            ));
        };
        if identity != recipient_sk.public_key() {
            bail!(KeyringError::EnvelopeAuthFailed(
                "envelope is not addressed to this recipient".to_owned()
            ));
        }

        Self::verify_and_open(group_id, envelope, || {
            SharedKey::new(recipient_sk, &ephemeral_pk)
        })
    }

    /// Unwrap a device-addressed [`KeyEnvelope`] with this node's device secret.
    ///
    /// `device` is checked against the envelope's address for the same
    /// defense-in-depth reason the member path checks the identity key, and a
    /// member-addressed envelope is refused for the same reason as above.
    pub fn unwrap_for_device(
        device: DeviceId,
        kem_secret: &X25519SecretKey,
        group_id: &[u8; 32],
        expected_sender: Option<&PublicKey>,
        envelope: &KeyEnvelope,
    ) -> EyreResult<[u8; 32]> {
        use calimero_crypto::SharedKey;

        Self::check_sender(expected_sender, envelope)?;

        let EnvelopeRecipient::Device {
            device: addressed,
            ephemeral_pk,
        } = envelope.recipient
        else {
            bail!(KeyringError::EnvelopeAuthFailed(
                "envelope is member-addressed; it cannot be opened with a device secret".to_owned()
            ));
        };
        if addressed != device {
            bail!(KeyringError::EnvelopeAuthFailed(
                "envelope is not addressed to this device".to_owned()
            ));
        }

        Self::verify_and_open(group_id, envelope, || {
            SharedKey::from_x25519(kem_secret, &X25519PublicKey::from(*ephemeral_pk.as_bytes()))
        })
    }

    /// Open an envelope with whichever credential it is addressed to.
    ///
    /// The receive paths need this because a single rotation bundle can carry
    /// both kinds at once: members who have enrolled a device get device
    /// envelopes, and members who have not are still addressed by identity. A
    /// receiver cannot know in advance which one is for it, so it tries the
    /// credential the envelope names.
    ///
    /// `device` is `None` on a node that has not enrolled one, in which case a
    /// device-addressed envelope simply is not for us.
    pub fn unwrap_any(
        identity_sk: &PrivateKey,
        device: Option<&DeviceSecret>,
        group_id: &[u8; 32],
        expected_sender: Option<&PublicKey>,
        envelope: &KeyEnvelope,
    ) -> EyreResult<[u8; 32]> {
        match envelope.recipient {
            EnvelopeRecipient::Member { .. } => {
                Self::unwrap_for_recipient(identity_sk, group_id, expected_sender, envelope)
            }
            EnvelopeRecipient::Device { .. } => {
                let Some(node_device) = device else {
                    bail!(KeyringError::EnvelopeAuthFailed(
                        "envelope is device-addressed and this node has enrolled no device"
                            .to_owned()
                    ));
                };
                Self::unwrap_for_device(
                    node_device.device,
                    &node_device.kem_secret,
                    group_id,
                    expected_sender,
                    envelope,
                )
            }
        }
    }

    /// Enforce `expected_sender` when the caller supplied one.
    fn check_sender(expected_sender: Option<&PublicKey>, envelope: &KeyEnvelope) -> EyreResult<()> {
        if let Some(expected) = expected_sender {
            if envelope.sender != *expected {
                bail!(KeyringError::EnvelopeAuthFailed(format!(
                    "sender {} is not the required {expected}",
                    envelope.sender
                )));
            }
        }
        Ok(())
    }

    /// Authenticate the sender, then agree and decrypt.
    ///
    /// The agreement is a closure so signature verification always happens
    /// **first**, for both wrap modes: a forged or cross-group-replayed envelope
    /// must fail before any key material is derived from bytes an attacker
    /// chose.
    fn verify_and_open<F>(
        group_id: &[u8; 32],
        envelope: &KeyEnvelope,
        agree: F,
    ) -> EyreResult<[u8; 32]>
    where
        F: FnOnce() -> Result<calimero_crypto::SharedKey, calimero_crypto::SharedKeyError>,
    {
        let payload = KeyEnvelope::signing_payload(
            group_id,
            &envelope.recipient,
            &envelope.sender,
            &envelope.nonce,
            &envelope.ciphertext,
        );
        envelope
            .sender
            .verify_raw_signature(&payload, &envelope.signature)
            .map_err(|e| KeyringError::EnvelopeAuthFailed(format!("verify: {e}")))?;

        let shared = agree().map_err(|e| KeyringError::KeyAgreementFailed {
            details: format!("{e:?}"),
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

    /// Who is entitled to this group's key right now.
    ///
    /// Split out of [`build_rotation`](Self::build_rotation) so the *policy*
    /// question — who may hold the key — is asked explicitly by the caller,
    /// rather than decided inside a wrapping helper. That matters beyond
    /// tidiness: entitlement is an authorization decision, and every other one
    /// in this system is answered from folded state, not from a side effect of
    /// wrapping.
    ///
    /// Resolution is **per member, device-first**: a member row names an
    /// account, and that account is addressed only through its live devices.
    ///
    /// A member whose every device has been revoked or superseded receives
    /// **nothing** until they enroll a new one. That is the correct outcome and
    /// not an oversight — the revoked device is still running on a node that
    /// holds the member key, so any fallback to identity addressing would hand
    /// the fresh key straight back to it.
    ///
    /// **Devices are resolved at the NAMESPACE, not at `self.group_id`.**
    /// Binding rows are written where the credential arrived, which is the
    /// namespace a member joined; a subgroup holds none of its own. Scanning
    /// the subgroup would find no devices for anyone and deliver nothing to a
    /// group whose members are all perfectly entitled. This is the same
    /// namespace-vs-decision-group distinction
    /// [`member_account_in_namespace`](crate::member_account_in_namespace)
    /// exists for.
    ///
    /// # Errors
    /// Propagates the membership or account-row scan failure.
    pub fn current_key_recipients(&self) -> EyreResult<Vec<EntitledRecipient>> {
        let members = MembershipRepository::new(self.store).list(&self.group_id, 0, usize::MAX)?;
        let namespace = NamespaceRepository::new(self.store).resolve(&self.group_id)?;
        // One scan for the whole fan-out. Asking per account inside the member
        // loop rescanned the binding column once per member.
        let devices =
            AccountBindingRepository::new(self.store).live_devices_by_account(&namespace)?;

        let mut out = Vec::with_capacity(members.len());
        for (member, _) in members {
            for binding in devices.get(&member).map(Vec::as_slice).unwrap_or(&[]) {
                out.push(EntitledRecipient {
                    member,
                    recipient: KeyRecipient::Device {
                        device: binding.device,
                        kem_pk: X25519PublicKey::from(binding.kem_pk),
                    },
                });
            }
        }
        Ok(out)
    }

    /// How to address a scope key to one requester who is asking for it, rather
    /// than to everyone entitled.
    ///
    /// Same rule as [`current_key_recipients`](Self::current_key_recipients),
    /// narrowed to a single member — and it has to be the same rule, or the pull
    /// path would route around the exclusion the fan-out enforces. That is exactly
    /// what it did before this existed: a request names the requester's *identity*
    /// key, so a node whose device had been revoked was still that member and was
    /// served the current key on its next sync round.
    ///
    /// [`KeyRequester::device`] is what the requester says it is. It is **not**
    /// authenticated, and does not need to be: the reply is sealed to that
    /// device's certified X25519 key, so naming someone else's device yields an
    /// envelope the caller cannot open. The wrap is the authentication.
    ///
    /// Returns `None` when nothing may be delivered:
    ///
    /// - the member has an account here and named no device, or named one that is
    ///   not a live device of theirs — the revoked case, and the one this exists
    ///   to close;
    /// - the member has an account here whose devices are all revoked or
    ///   superseded. Recovery is deliberately **not** self-service: re-enrolling
    ///   needs an encrypted `GroupOp`, which needs the key, so a member in this
    ///   state needs an admin to re-deliver it (`RootOp::KeyDelivery`) or to
    ///   publish the link on their behalf. If they could re-key themselves,
    ///   revocation would mean nothing.
    ///
    /// A requester whose key names no account here is served nothing. Since
    /// every member is bound by the op that admits it — a join for a joiner, the
    /// genesis for a founder — an unresolvable requester is not a member, and
    /// the caller has already refused it on that basis.
    ///
    /// # Errors
    /// Propagates the account-row scan failure.
    pub fn key_recipient_for_requester(
        &self,
        requester: &KeyRequester,
    ) -> EyreResult<Option<KeyRecipient>> {
        // Resolved at the NAMESPACE, for the same reason
        // [`current_key_recipients`](Self::current_key_recipients) resolves
        // devices there: bindings are written where the credential arrived, and
        // a subgroup holds none of its own.
        let namespace = NamespaceRepository::new(self.store).resolve(&self.group_id)?;
        let Some(account) =
            crate::member_account_in_namespace(self.store, &self.group_id, &requester.identity)?
        else {
            return Ok(None);
        };

        let Some(claimed) = requester.device else {
            return Ok(None);
        };
        let bindings = AccountBindingRepository::new(self.store);
        for binding in bindings.live_bindings(&namespace)? {
            if binding.account == account && binding.device == claimed {
                return Ok(Some(KeyRecipient::Device {
                    device: binding.device,
                    kem_pk: X25519PublicKey::from(binding.kem_pk),
                }));
            }
        }
        Ok(None)
    }

    /// Wrap `group_key` for one [`KeyRecipient`], whichever kind it is.
    ///
    /// # Errors
    /// Propagates the wrap failure.
    pub fn wrap_for_recipient(
        sender_sk: &PrivateKey,
        recipient: &KeyRecipient,
        group_id: &[u8; 32],
        group_key: &[u8; 32],
    ) -> EyreResult<KeyEnvelope> {
        match recipient {
            KeyRecipient::Member(identity) => {
                Self::wrap_for_member(sender_sk, identity, group_id, group_key)
            }
            KeyRecipient::Device { device, kem_pk } => {
                Self::wrap_for_device(sender_sk, *device, kem_pk, group_id, group_key)
            }
        }
    }

    /// Wrap `new_group_key` once per recipient.
    ///
    /// `recipients` is an input rather than something this function discovers,
    /// so the function does only what its name says: wrap. Deciding who belongs
    /// in the list is [`current_key_recipients`](Self::current_key_recipients).
    ///
    /// This is also why there is no `excluded_member` parameter. Exclusion only
    /// existed because the caller had no other way to influence a list the
    /// function built for itself; now removing someone is simply leaving them
    /// out — which cannot be forgotten silently the way an unpassed exclusion
    /// could.
    ///
    /// # Errors
    /// Propagates a wrap failure for any recipient.
    pub fn build_rotation(
        &self,
        new_group_key: &[u8; 32],
        sender_sk: &PrivateKey,
        recipients: &[KeyRecipient],
    ) -> EyreResult<KeyRotation> {
        let group_id = self.group_id.to_bytes();
        let new_key_id = Self::key_id_for(new_group_key);
        let mut envelopes = Vec::with_capacity(recipients.len());

        for recipient in recipients {
            envelopes.push(Self::wrap_for_recipient(
                sender_sk,
                recipient,
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
mod recipient_tests {
    use super::*;
    use crate::test_fixtures::{test_group_id, test_store};
    use calimero_primitives::context::GroupMemberRole;

    use crate::AccountBindingRepository;
    use calimero_account::{sign_device_cert, AccountGenesis, KemPublicKey};
    use calimero_store::Store;

    fn member(seed: u8) -> PublicKey {
        PrivateKey::from([seed; 32]).public_key()
    }

    /// Enroll a device for an account rooted at `member_sk`, the shape the
    /// membership gate requires: the account's epoch-0 root key IS a member key.
    fn link_device(
        store: &Store,
        gid: ContextGroupId,
        member_sk: &PrivateKey,
        device_seed: u8,
    ) -> DeviceId {
        let genesis = AccountGenesis::new(member_sk.public_key(), [device_seed; 16]);
        let account = genesis.account_id();
        let cert = sign_device_cert(
            member_sk,
            account,
            DeviceId::mint(account, [device_seed; 16]),
            &PrivateKey::from([device_seed; 32]).public_key(),
            &KemPublicKey::from(
                *X25519SecretKey::from([device_seed; 32])
                    .public_key()
                    .as_bytes(),
            ),
            0,
            0,
        )
        .expect("sign cert");
        let repo = AccountBindingRepository::new(store);
        // The apply handler records the vouch alongside the link; the fixture has
        // to as well, or the member→account direction is empty and every lookup
        // here would be testing a state production never produces.
        repo.record_endorser(&gid, account, &member_sk.public_key())
            .expect("endorse");
        repo.apply_link(&gid, &genesis, &[], &cert)
            .expect("store")
            .expect("admitted")
            .device
    }

    #[test]
    fn build_rotation_wraps_exactly_the_recipients_it_is_given() {
        // The point of taking a list rather than discovering one: what goes out
        // is what the caller asked for, with no hidden policy in between.
        let store = test_store();
        let gid = test_group_id();
        let sender = PrivateKey::from([1u8; 32]);

        let recipients = vec![
            KeyRecipient::Member(member(2)),
            KeyRecipient::Member(member(3)),
        ];
        let rotation = GroupKeyring::new(&store, gid)
            .build_rotation(&[9u8; 32], &sender, &recipients)
            .expect("build rotation");

        let wrapped: Vec<Option<PublicKey>> = rotation
            .envelopes
            .iter()
            .map(|e| e.recipient.member_identity())
            .collect();
        assert_eq!(wrapped, vec![Some(member(2)), Some(member(3))]);
    }

    #[test]
    fn build_rotation_ignores_membership_rows_entirely() {
        // Regression guard for the split. Membership in the store must NOT leak
        // into the fan-out — otherwise a caller that deliberately narrows the
        // recipient list (a removal, or a per-device list) would silently have
        // members added back behind its back.
        let store = test_store();
        let gid = test_group_id();
        let sender = PrivateKey::from([1u8; 32]);

        let stored = member(7);
        MembershipRepository::new(&store)
            .add_member(&gid, &stored, GroupMemberRole::Member)
            .expect("add member");

        let asked_for = vec![KeyRecipient::Member(member(2))];
        let rotation = GroupKeyring::new(&store, gid)
            .build_rotation(&[9u8; 32], &sender, &asked_for)
            .expect("build rotation");

        assert_eq!(rotation.envelopes.len(), 1);
        assert_eq!(
            rotation.envelopes[0].recipient.member_identity(),
            Some(member(2))
        );
        assert!(
            rotation
                .envelopes
                .iter()
                .all(|e| e.recipient.member_identity() != Some(stored)),
            "a member row must not add itself to a caller-supplied list"
        );
    }

    #[test]
    fn a_member_with_no_account_is_addressed_by_identity() {
        // The bootstrap case, and the reason the identity form cannot be
        // retired: a member who has never held the key long enough to publish a
        // device link has nothing else to be addressed by.
        let store = test_store();
        let gid = test_group_id();
        let repo = MembershipRepository::new(&store);
        repo.add_member(&gid, &member(2), GroupMemberRole::Member)
            .expect("add");
        repo.add_member(&gid, &member(3), GroupMemberRole::Admin)
            .expect("add");

        let mut got: Vec<PublicKey> = GroupKeyring::new(&store, gid)
            .current_key_recipients()
            .expect("list")
            .into_iter()
            .map(|entitled| match entitled.recipient {
                KeyRecipient::Member(identity) => identity,
                KeyRecipient::Device { .. } => panic!("no devices are enrolled"),
            })
            .collect();
        got.sort_unstable();
        let mut want = vec![member(2), member(3)];
        want.sort_unstable();
        assert_eq!(got, want);
    }

    /// Enroll a device for an account rooted at a **dedicated account root** —
    /// the shape production actually produces since the root became an offline
    /// key of its own. The member tie is the endorsement, not the genesis key.
    fn link_device_under_dedicated_root(
        store: &Store,
        gid: ContextGroupId,
        account_root_sk: &PrivateKey,
        endorser: &PublicKey,
        device_seed: u8,
    ) -> DeviceId {
        let genesis = AccountGenesis::new(account_root_sk.public_key(), [device_seed; 16]);
        let account = genesis.account_id();
        let cert = sign_device_cert(
            account_root_sk,
            account,
            DeviceId::mint(account, [device_seed; 16]),
            &PrivateKey::from([device_seed; 32]).public_key(),
            &KemPublicKey::from(
                *X25519SecretKey::from([device_seed; 32])
                    .public_key()
                    .as_bytes(),
            ),
            0,
            0,
        )
        .expect("sign cert");
        let repo = AccountBindingRepository::new(store);
        repo.record_endorser(&gid, account, endorser)
            .expect("endorse");
        repo.apply_link(&gid, &genesis, &[], &cert)
            .expect("store")
            .expect("admitted")
            .device
    }

    #[test]
    fn a_member_whose_account_uses_a_dedicated_root_is_still_addressed_by_device() {
        // The rooting production uses: the account root is an offline key that is
        // a member NOWHERE, and the tie to the member is the endorsement carried
        // on the link. Matching members against the account's genesis key — as
        // this did — matches nothing once the root is dedicated, so every member
        // silently falls back to identity addressing, which hands the scope key
        // straight to the node running a revoked device: the exact leak
        // device-first delivery exists to close.
        let store = test_store();
        let gid = test_group_id();
        let member_sk = PrivateKey::from([2u8; 32]);
        MembershipRepository::new(&store)
            .add_member(&gid, &member_sk.public_key(), GroupMemberRole::Member)
            .expect("add");

        let account_root = PrivateKey::from([42u8; 32]);
        let laptop = link_device_under_dedicated_root(
            &store,
            gid,
            &account_root,
            &member_sk.public_key(),
            5,
        );

        let recipients = GroupKeyring::new(&store, gid)
            .current_key_recipients()
            .expect("list");

        let devices: Vec<DeviceId> = recipients
            .iter()
            .filter_map(|e| match e.recipient {
                KeyRecipient::Device { device, .. } => Some(device),
                KeyRecipient::Member(_) => None,
            })
            .collect();
        assert_eq!(
            devices,
            vec![laptop],
            "the account's device must be addressed; got {recipients:?}"
        );
        assert!(
            !recipients
                .iter()
                .any(|e| matches!(e.recipient, KeyRecipient::Member(_))),
            "a member whose account has a live device must NOT also be addressed \
             by identity: {recipients:?}"
        );
    }

    #[test]
    fn a_member_with_devices_is_addressed_only_through_them() {
        // Device-first. Keeping the identity entry alongside would hand the key
        // to the member's node directly, which is exactly how a revoked device
        // would get it back.
        let store = test_store();
        let gid = test_group_id();
        let member_sk = PrivateKey::from([2u8; 32]);
        MembershipRepository::new(&store)
            .add_member(&gid, &member_sk.public_key(), GroupMemberRole::Member)
            .expect("add");

        let laptop = link_device(&store, gid, &member_sk, 5);
        let phone = link_device(&store, gid, &member_sk, 6);

        let got = GroupKeyring::new(&store, gid)
            .current_key_recipients()
            .expect("list");
        assert_eq!(got.len(), 2, "one entry per device, none for the identity");
        assert!(
            got.iter()
                .all(|e| e.member == member_sk.public_key() && e.recipient.device().is_some()),
            "every entry must be a device speaking for the member"
        );

        let mut devices: Vec<DeviceId> = got.iter().filter_map(|e| e.recipient.device()).collect();
        devices.sort_unstable();
        let mut want = vec![laptop, phone];
        want.sort_unstable();
        assert_eq!(devices, want);
    }

    #[test]
    fn a_root_key_rotation_does_not_cut_the_account_off_from_delivery() {
        // The link gate reads the immutable genesis key, so an account keeps
        // passing it across rotations. The fan-out used to key on the account's
        // CURRENT root, so rotating onto a key that is not a group member made the
        // account vanish from delivery while still authorized to write — devices
        // that may author but can never read what they wrote.
        let store = test_store();
        let gid = test_group_id();
        let member_sk = PrivateKey::from([2u8; 32]);
        MembershipRepository::new(&store)
            .add_member(&gid, &member_sk.public_key(), GroupMemberRole::Member)
            .expect("add");
        let device = link_device(&store, gid, &member_sk, 5);
        assert_eq!(
            GroupKeyring::new(&store, gid)
                .current_key_recipients()
                .expect("list")
                .len(),
            1
        );

        // Rotate the account root onto a key that is NOT a member of the group.
        let genesis = AccountGenesis::new(member_sk.public_key(), [5u8; 16]);
        let offline_root = PrivateKey::from([0x77u8; 32]);
        let handoff = calimero_account::sign_root_key_handoff(
            &member_sk,
            genesis.account_id(),
            0,
            &offline_root.public_key(),
        )
        .expect("sign handoff");
        AccountBindingRepository::new(&store)
            .apply_rotation(&gid, &handoff)
            .expect("store")
            .expect("rotated");

        // The account is still the member's, so it is still addressed — through
        // whichever devices remain in force under the new epoch.
        let got = GroupKeyring::new(&store, gid)
            .current_key_recipients()
            .expect("list");
        assert!(
            got.iter().all(|e| e.member == member_sk.public_key()),
            "the account must still resolve to the member its genesis names, not \
             disappear because its current root key is not a member"
        );
        // The epoch-0 device is superseded by the rotation, so it drops out — that
        // is the supersession rule, not the keying bug.
        assert!(got.iter().all(|e| e.recipient.device() != Some(device)));
    }

    #[test]
    fn a_member_whose_only_device_was_revoked_is_addressed_by_nothing() {
        // Not an oversight: falling back to the identity key here would deliver
        // the fresh key to the very node running the revoked device. They get
        // nothing until they enroll again.
        let store = test_store();
        let gid = test_group_id();
        let member_sk = PrivateKey::from([2u8; 32]);
        MembershipRepository::new(&store)
            .add_member(&gid, &member_sk.public_key(), GroupMemberRole::Member)
            .expect("add");

        let laptop = link_device(&store, gid, &member_sk, 5);
        AccountBindingRepository::new(&store)
            .apply_revocation(&gid, laptop)
            .expect("revoke");

        assert!(GroupKeyring::new(&store, gid)
            .current_key_recipients()
            .expect("list")
            .is_empty());
    }

    #[test]
    fn excluding_a_member_drops_every_device_of_theirs() {
        // The property the member/recipient pairing exists for. Filtering by
        // recipient alone could only drop an identity-addressed entry, leaving a
        // removed member's devices holding the fresh key.
        let store = test_store();
        let gid = test_group_id();
        let leaving = PrivateKey::from([2u8; 32]);
        let staying = PrivateKey::from([3u8; 32]);
        let repo = MembershipRepository::new(&store);
        repo.add_member(&gid, &leaving.public_key(), GroupMemberRole::Member)
            .expect("add");
        repo.add_member(&gid, &staying.public_key(), GroupMemberRole::Admin)
            .expect("add");

        let doomed = [
            link_device(&store, gid, &leaving, 5),
            link_device(&store, gid, &leaving, 6),
        ];
        let survivor = link_device(&store, gid, &staying, 7);

        let kept: Vec<KeyRecipient> = GroupKeyring::new(&store, gid)
            .current_key_recipients()
            .expect("list")
            .into_iter()
            .filter(|entitled| entitled.member != leaving.public_key())
            .map(|entitled| entitled.recipient)
            .collect();

        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].device(), Some(survivor));
        for device in doomed {
            assert!(
                !kept.iter().any(|r| r.device() == Some(device)),
                "a removed member's device must not survive the exclusion"
            );
        }
    }

    #[test]
    fn a_rotation_can_carry_both_addressing_modes_at_once() {
        // The mixed group is the normal case during rollout, and the receive
        // path depends on a bundle being allowed to hold both.
        let store = test_store();
        let gid = test_group_id();
        let enrolled = PrivateKey::from([2u8; 32]);
        let bare = PrivateKey::from([3u8; 32]);
        let repo = MembershipRepository::new(&store);
        repo.add_member(&gid, &enrolled.public_key(), GroupMemberRole::Member)
            .expect("add");
        repo.add_member(&gid, &bare.public_key(), GroupMemberRole::Member)
            .expect("add");
        let device = link_device(&store, gid, &enrolled, 5);

        let keyring = GroupKeyring::new(&store, gid);
        let recipients: Vec<KeyRecipient> = keyring
            .current_key_recipients()
            .expect("list")
            .into_iter()
            .map(|entitled| entitled.recipient)
            .collect();
        let rotation = keyring
            .build_rotation(&[9u8; 32], &PrivateKey::from([1u8; 32]), &recipients)
            .expect("build rotation");

        assert_eq!(rotation.envelopes.len(), 2);
        assert!(rotation
            .envelopes
            .iter()
            .any(|e| e.recipient.device() == Some(device)));
        assert!(rotation
            .envelopes
            .iter()
            .any(|e| e.recipient.member_identity() == Some(bare.public_key())));
    }

    #[test]
    fn an_empty_recipient_list_produces_no_envelopes() {
        // Degenerate but reachable: the last member leaves. A rotation with no
        // recipients must be an empty envelope set, not an error and not a
        // silent fall-back to "everyone".
        let store = test_store();
        let gid = test_group_id();
        MembershipRepository::new(&store)
            .add_member(&gid, &member(7), GroupMemberRole::Member)
            .expect("add member");

        let rotation = GroupKeyring::new(&store, gid)
            .build_rotation(&[9u8; 32], &PrivateKey::from([1u8; 32]), &[])
            .expect("build rotation");
        assert!(rotation.envelopes.is_empty());
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
        let EnvelopeRecipient::Member { ephemeral_pk, .. } = env.recipient else {
            panic!("wrap_for_member must produce a member-addressed envelope");
        };
        assert_ne!(ephemeral_pk, sender.public_key());

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

    fn device_fixture(seed: u8) -> (DeviceId, X25519SecretKey) {
        let account = calimero_account::AccountGenesis::new(
            PrivateKey::from([seed; 32]).public_key(),
            [seed; 16],
        )
        .account_id();
        (
            DeviceId::mint(account, [seed; 16]),
            X25519SecretKey::from([seed; 32]),
        )
    }

    #[test]
    fn device_envelope_roundtrips_under_native_x25519() {
        let group_id = [0x11u8; 32];
        let group_key = [0x22u8; 32];
        let sender = PrivateKey::from([0x01u8; 32]);
        let (device, kem_secret) = device_fixture(0x05);

        let env = GroupKeyring::wrap_for_device(
            &sender,
            device,
            &kem_secret.public_key(),
            &group_id,
            &group_key,
        )
        .unwrap();

        assert_eq!(env.recipient.device(), Some(device));
        assert_eq!(
            GroupKeyring::unwrap_for_device(device, &kem_secret, &group_id, None, &env).unwrap(),
            group_key
        );

        // Same authentication guarantees as the member form: bound to the group,
        // bound to the sender, tamper-evident.
        assert!(
            GroupKeyring::unwrap_for_device(device, &kem_secret, &[0x33u8; 32], None, &env)
                .is_err()
        );
        let wrong_sender = PrivateKey::from([0x09u8; 32]).public_key();
        assert!(GroupKeyring::unwrap_for_device(
            device,
            &kem_secret,
            &group_id,
            Some(&wrong_sender),
            &env
        )
        .is_err());
        let mut tampered = env.clone();
        tampered.signature[0] ^= 0xFF;
        assert!(
            GroupKeyring::unwrap_for_device(device, &kem_secret, &group_id, None, &tampered)
                .is_err()
        );
    }

    #[test]
    fn a_device_envelope_is_not_for_another_device() {
        let group_id = [0x11u8; 32];
        let sender = PrivateKey::from([0x01u8; 32]);
        let (device, kem_secret) = device_fixture(0x05);
        let (other_device, other_secret) = device_fixture(0x06);

        let env = GroupKeyring::wrap_for_device(
            &sender,
            device,
            &kem_secret.public_key(),
            &group_id,
            &[0x22u8; 32],
        )
        .unwrap();

        // Wrong device id, wrong secret, and the combination — all refused.
        assert!(GroupKeyring::unwrap_for_device(
            other_device,
            &other_secret,
            &group_id,
            None,
            &env
        )
        .is_err());
        assert!(
            GroupKeyring::unwrap_for_device(device, &other_secret, &group_id, None, &env).is_err()
        );
    }

    #[test]
    fn the_two_addressing_modes_cannot_be_confused() {
        // The tag inside the signed payload is what enforces this. Without it,
        // rewriting the borsh discriminant would reinterpret a member envelope's
        // identity as a device id while the signature still verified.
        let group_id = [0x11u8; 32];
        let group_key = [0x22u8; 32];
        let sender = PrivateKey::from([0x01u8; 32]);
        let identity = PrivateKey::from([0x02u8; 32]);
        let (device, kem_secret) = device_fixture(0x05);

        let member_env =
            GroupKeyring::wrap_for_member(&sender, &identity.public_key(), &group_id, &group_key)
                .unwrap();
        let device_env = GroupKeyring::wrap_for_device(
            &sender,
            device,
            &kem_secret.public_key(),
            &group_id,
            &group_key,
        )
        .unwrap();

        // Neither unwrap accepts the other's envelope, and each says so rather
        // than failing later inside the AEAD.
        assert!(
            GroupKeyring::unwrap_for_recipient(&identity, &group_id, None, &device_env).is_err()
        );
        assert!(
            GroupKeyring::unwrap_for_device(device, &kem_secret, &group_id, None, &member_env)
                .is_err()
        );
    }

    #[test]
    fn unwrap_any_routes_each_envelope_to_the_credential_that_opens_it() {
        // What the rotation receive path relies on: one bundle, both modes, and
        // a receiver that does not know in advance which entry is its own.
        let group_id = [0x11u8; 32];
        let group_key = [0x22u8; 32];
        let sender = PrivateKey::from([0x01u8; 32]);
        let identity = PrivateKey::from([0x02u8; 32]);
        let (device, kem_secret) = device_fixture(0x05);
        let node_device = DeviceSecret {
            device,
            kem_secret: kem_secret.clone(),
        };

        let member_env =
            GroupKeyring::wrap_for_member(&sender, &identity.public_key(), &group_id, &group_key)
                .unwrap();
        let device_env = GroupKeyring::wrap_for_device(
            &sender,
            device,
            &kem_secret.public_key(),
            &group_id,
            &group_key,
        )
        .unwrap();

        for env in [&member_env, &device_env] {
            assert_eq!(
                GroupKeyring::unwrap_any(&identity, Some(&node_device), &group_id, None, env)
                    .unwrap(),
                group_key
            );
        }

        // A node that has enrolled no device is simply not the addressee of a
        // device envelope, and must not fall back to its identity key.
        assert!(GroupKeyring::unwrap_any(&identity, None, &group_id, None, &device_env).is_err());
        assert_eq!(
            GroupKeyring::unwrap_any(&identity, None, &group_id, None, &member_env).unwrap(),
            group_key
        );
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
