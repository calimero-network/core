#![allow(single_use_lifetimes, reason = "borsh shenanigans")]

use borsh::{BorshDeserialize, BorshSerialize};

use crate::entry::{Borsh, Identity};
use crate::key;
use crate::slice::Slice;
use crate::types::PredefinedEntry;

pub type Hash = [u8; 32];

#[derive(BorshDeserialize, BorshSerialize, Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct ContextMeta {
    pub application: key::ApplicationMeta,
    pub root_hash: Hash,
    pub dag_heads: Vec<[u8; 32]>,
    pub service_name: Option<Box<str>>,
}

impl ContextMeta {
    #[must_use]
    pub const fn new(
        application: key::ApplicationMeta,
        root_hash: Hash,
        dag_heads: Vec<[u8; 32]>,
        service_name: Option<Box<str>>,
    ) -> Self {
        Self {
            application,
            root_hash,
            dag_heads,
            service_name,
        }
    }
}

impl PredefinedEntry for key::ContextMeta {
    type Codec = Borsh;
    type DataType<'a> = ContextMeta;
}

/// Value for [`key::ContextAuthoredRemaining`]: this node's owner's count of
/// identity-gated entries still below the target schema (the heartbeat's
/// `authored_remaining`; 6f). Node-local + advisory, written only by the
/// post-migrate / `migrate_my_entries` persist and read by the heartbeat —
/// kept off the hot `ContextMeta` write path so a per-write rewrite can't
/// clobber it. A brand-new key, so a missing row reads as `None` (treated as
/// 0); no on-disk back-compat shim needed.
#[derive(BorshDeserialize, BorshSerialize, Clone, Copy, Debug, Eq, PartialEq)]
#[expect(
    clippy::exhaustive_structs,
    reason = "single advisory counter; additions would need a migration"
)]
pub struct ContextAuthoredRemaining {
    pub count: u32,
}

impl PredefinedEntry for key::ContextAuthoredRemaining {
    type Codec = Borsh;
    type DataType<'a> = ContextAuthoredRemaining;
}

/// Value for [`key::ContextMigrationFailed`]: the categorized reason this
/// context's last migration attempt did not complete, as a stable discriminant
/// (`1` = migration-check aborted, `2` = migrate apply errored). Node-local +
/// advisory; the key's presence is the signal, the byte carries the reason. A
/// brand-new key, so a missing row reads as `None` (no failure on record).
#[derive(BorshDeserialize, BorshSerialize, Clone, Copy, Debug, Eq, PartialEq)]
#[expect(
    clippy::exhaustive_structs,
    reason = "single advisory discriminant; additions would need a migration"
)]
pub struct ContextMigrationFailed {
    pub kind: u8,
}

impl PredefinedEntry for key::ContextMigrationFailed {
    type Codec = Borsh;
    type DataType<'a> = ContextMigrationFailed;
}

#[derive(BorshDeserialize, BorshSerialize, Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct ContextConfig {
    pub application_revision: u64,
    pub members_revision: u64,
}

impl ContextConfig {
    #[must_use]
    pub const fn new(application_revision: u64, members_revision: u64) -> Self {
        Self {
            application_revision,
            members_revision,
        }
    }
}

impl PredefinedEntry for key::ContextConfig {
    type Codec = Borsh;
    type DataType<'a> = ContextConfig;
}

#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct ContextState<'a> {
    pub value: Slice<'a>,
}

impl PredefinedEntry for key::ContextState {
    type Codec = Identity;
    type DataType<'a> = ContextState<'a>;
}

impl<'a> From<Slice<'a>> for ContextState<'a> {
    fn from(value: Slice<'a>) -> Self {
        Self { value }
    }
}

impl AsRef<[u8]> for ContextState<'_> {
    fn as_ref(&self) -> &[u8] {
        self.value.as_ref()
    }
}

/// Node-local private storage that is NOT synchronized across nodes
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct ContextPrivateState<'a> {
    pub value: Slice<'a>,
}

impl PredefinedEntry for key::ContextPrivateState {
    type Codec = Identity;
    type DataType<'a> = ContextPrivateState<'a>;
}

impl<'a> From<Slice<'a>> for ContextPrivateState<'a> {
    fn from(value: Slice<'a>) -> Self {
        Self { value }
    }
}

impl AsRef<[u8]> for ContextPrivateState<'_> {
    fn as_ref(&self) -> &[u8] {
        self.value.as_ref()
    }
}

#[derive(BorshDeserialize, BorshSerialize, Clone, Copy, Debug, Eq, PartialEq)]
#[expect(
    clippy::exhaustive_structs,
    reason = "This is not expected to have additional fields"
)]
pub struct ContextIdentity {
    pub private_key: Option<[u8; 32]>,
    pub sender_key: Option<[u8; 32]>,
}

impl PredefinedEntry for key::ContextIdentity {
    type Codec = Borsh;
    type DataType<'a> = ContextIdentity;
}

/// Tombstone value for `key::ContextLeftMarker`. Stores when the user explicitly
/// left this context on this node (millis since epoch). Presence of the row is
/// what matters for the auto-follow gate; the timestamp is for diagnostics.
#[derive(BorshDeserialize, BorshSerialize, Clone, Copy, Debug, Eq, PartialEq)]
#[expect(
    clippy::exhaustive_structs,
    reason = "Tombstone value — additions would need a migration"
)]
pub struct ContextLeftMarker {
    pub left_at_ms: u64,
}

impl PredefinedEntry for key::ContextLeftMarker {
    type Codec = Borsh;
    type DataType<'a> = ContextLeftMarker;
}

/// DAG delta data (persisted)
#[derive(BorshDeserialize, BorshSerialize, Clone, Debug)]
pub struct ContextDagDelta {
    pub delta_id: [u8; 32],
    pub parents: Vec<[u8; 32]>,
    pub actions: Vec<u8>, // Serialized actions
    pub hlc: calimero_storage::logical_clock::HybridTimestamp,
    pub applied: bool,
    pub expected_root_hash: [u8; 32],
    pub events: Option<Vec<u8>>,
    /// Signing identity of the node that authored this delta. Populated
    /// from the gossip envelope on receive; populated from the local
    /// node identity on local apply. Used by the DAG-catchup responder
    /// to advertise the author on the wire so initiator-side membership
    /// checks can reject revoked-author deltas at apply time (parity
    /// with the gossip-receive cross-DAG check).
    pub author_id: Option<calimero_primitives::identity::PublicKey>,
    /// Serialized `calimero_context_config::types::GovernanceParentEdge`
    /// (borsh bytes) at sign time. Stored as a blob to avoid pulling
    /// `calimero-context-config` into `calimero-store` — matches the
    /// existing pattern for `actions` / `events`. Initiator-side
    /// DAG-catchup deserializes this and runs `membership_status_at`
    /// against it. `None` for legacy deltas authored before this
    /// field was added.
    pub governance_position_blob: Option<Vec<u8>>,
    /// Ed25519 signature by `author_id`'s identity key over the
    /// canonical `DeltaSignaturePayload`. Closes the anti-impersonation
    /// gap on the delta envelope: a current group-key holder can't
    /// relabel a foreign delta as their own (or vice versa). Served
    /// alongside `author_id` on the wire; verified by every receive
    /// path before applying. `None` for snapshot checkpoints / genesis
    /// rows that have no author signature to record.
    pub delta_signature: Option<[u8; 64]>,
}

impl ContextDagDelta {
    /// Deserialize actions from the serialized byte array
    ///
    /// # Errors
    ///
    /// Returns an error if the actions cannot be deserialized
    pub fn deserialize_actions(
        &self,
    ) -> Result<Vec<calimero_storage::action::Action>, borsh::io::Error> {
        borsh::from_slice(&self.actions)
    }

    /// Deserialize events from the serialized byte array (if present)
    ///
    /// # Errors
    ///
    /// Returns an error if the events cannot be deserialized
    #[cfg(feature = "serde")]
    pub fn deserialize_events(&self) -> Result<Option<Vec<serde_json::Value>>, eyre::Report> {
        if let Some(ref events_bytes) = self.events {
            let events: Vec<serde_json::Value> = serde_json::from_slice(events_bytes)
                .map_err(|e| eyre::eyre!("Failed to deserialize events: {}", e))?;
            Ok(Some(events))
        } else {
            Ok(None)
        }
    }
}

impl PredefinedEntry for key::ContextDagDelta {
    type Codec = Borsh;
    type DataType<'a> = ContextDagDelta;
}

#[cfg(test)]
mod context_authored_remaining_tests {
    use borsh::BorshDeserialize;

    use super::ContextAuthoredRemaining;

    // The dedicated counter value round-trips through borsh.
    #[test]
    fn authored_remaining_roundtrips() {
        let v = ContextAuthoredRemaining { count: 5 };
        let bytes = borsh::to_vec(&v).expect("serialize");
        let back = ContextAuthoredRemaining::try_from_slice(&bytes).expect("deserialize");
        assert_eq!(back.count, 5);
    }
}

#[cfg(test)]
mod context_local_key_isolation_tests {
    use std::sync::Arc;

    use calimero_primitives::context::ContextId;

    use crate::db::InMemoryDB;
    use crate::key;
    use crate::types::{ContextAuthoredRemaining, ContextMigrationFailed};
    use crate::Store;

    // ContextMigrationFailed lives in its own column, so its context_id-only key
    // must not share a KV row with the same-shaped ContextAuthoredRemaining key.
    // (Sharing ContextLocal collided: a failure write clobbered the count, and a
    // count of 1 misdecoded as `check_aborted`.) Writing/clearing one must leave
    // the other untouched.
    #[test]
    fn migration_failed_does_not_collide_with_authored_remaining() {
        let store = Store::new(Arc::new(InMemoryDB::owned()));
        let ctx = ContextId::from([7u8; 32]);
        let ar = key::ContextAuthoredRemaining::new(ctx);
        let mf = key::ContextMigrationFailed::new(ctx);

        let mut h = store.handle();
        h.put(&ar, &ContextAuthoredRemaining { count: 5 }).unwrap();
        h.put(&mf, &ContextMigrationFailed { kind: 2 }).unwrap();

        // Independent rows — neither write clobbered the other.
        assert_eq!(h.get(&ar).unwrap().unwrap().count, 5);
        assert_eq!(h.get(&mf).unwrap().unwrap().kind, 2);

        // Clearing the failure marker must NOT delete the authored-remaining row.
        h.delete(&mf).unwrap();
        assert_eq!(h.get(&ar).unwrap().unwrap().count, 5);
        assert!(h.get(&mf).unwrap().is_none());
    }
}

/// Value for [`key::ContextExecutingBlob`]: the bytecode blob this context's
/// committed state executes under, when it differs from the application
/// row's (version-stable bundle id, row already overwritten in place by a
/// newer version). Written on logical migration abort; deleted when a
/// migrate succeeds. Node-local; a missing row means "execute the row's
/// bytecode" (today's behavior).
#[derive(BorshDeserialize, BorshSerialize, Clone, Copy, Debug, Eq, PartialEq)]
#[expect(
    clippy::exhaustive_structs,
    reason = "single pin value; additions would need a migration"
)]
pub struct ContextExecutingBlob {
    pub blob: [u8; 32],
}

impl PredefinedEntry for key::ContextExecutingBlob {
    type Codec = Borsh;
    type DataType<'a> = ContextExecutingBlob;
}
