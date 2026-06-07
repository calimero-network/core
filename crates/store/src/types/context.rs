#![allow(single_use_lifetimes, reason = "borsh shenanigans")]

use borsh::{BorshDeserialize, BorshSerialize};

use crate::entry::{Borsh, Identity};
use crate::key;
use crate::slice::Slice;
use crate::types::PredefinedEntry;

pub type Hash = [u8; 32];

#[derive(BorshSerialize, Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct ContextMeta {
    pub application: key::ApplicationMeta,
    pub root_hash: Hash,
    pub dag_heads: Vec<[u8; 32]>,
    pub service_name: Option<Box<str>>,
    /// Best-effort count of THIS node's owner's identity-gated entries still
    /// below the target schema (self-reported in the migration heartbeat as
    /// `authored_remaining`). Node-local; recomputed at migrate-apply /
    /// `migrate_my_entries` and preserved across other ContextMeta rewrites.
    pub authored_remaining: u32,
}

// Custom deserialization: `authored_remaining` was appended after the initial
// schema. Old on-disk rows end after `service_name`; tolerate EOF and default
// to 0 so existing contexts deserialize unchanged. (ContextMeta is node-local,
// so only on-disk back-compat matters — no cross-node wire concern.)
impl BorshDeserialize for ContextMeta {
    fn deserialize_reader<R: std::io::Read>(reader: &mut R) -> std::io::Result<Self> {
        let application = key::ApplicationMeta::deserialize_reader(reader)?;
        let root_hash = Hash::deserialize_reader(reader)?;
        let dag_heads = Vec::<[u8; 32]>::deserialize_reader(reader)?;
        let service_name = Option::<Box<str>>::deserialize_reader(reader)?;
        // A short trailing read surfaces as either UnexpectedEof or (in this
        // borsh version) InvalidData "Unexpected length of input" — both mean
        // an old row with no authored_remaining; default to 0. Mirrors the
        // ApplicationMeta `services` back-compat handling.
        let authored_remaining = match u32::deserialize_reader(reader) {
            Ok(v) => v,
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => 0,
            Err(e)
                if e.kind() == std::io::ErrorKind::InvalidData
                    && e.to_string().contains("Unexpected length") =>
            {
                0
            }
            Err(e) => return Err(e),
        };
        Ok(Self {
            application,
            root_hash,
            dag_heads,
            service_name,
            authored_remaining,
        })
    }
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
            authored_remaining: 0,
        }
    }
}

impl PredefinedEntry for key::ContextMeta {
    type Codec = Borsh;
    type DataType<'a> = ContextMeta;
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
    /// Serialized `calimero_context_config::types::GovernancePosition`
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
mod context_meta_backcompat {
    use borsh::{BorshDeserialize, BorshSerialize};
    use calimero_primitives::application::ApplicationId;

    use super::{ContextMeta, Hash};
    use crate::key;

    // The on-disk layout before `authored_remaining` was appended.
    #[derive(BorshSerialize)]
    struct OldContextMeta {
        application: key::ApplicationMeta,
        root_hash: Hash,
        dag_heads: Vec<[u8; 32]>,
        service_name: Option<Box<str>>,
    }

    // An old row (no authored_remaining bytes) deserializes with the field 0.
    #[test]
    fn old_row_defaults_authored_remaining_to_zero() {
        let app = key::ApplicationMeta::new(ApplicationId::from([7u8; 32]));
        let old = OldContextMeta {
            application: app,
            root_hash: [3u8; 32],
            dag_heads: vec![[9u8; 32]],
            service_name: Some("svc".into()),
        };
        let bytes = borsh::to_vec(&old).expect("serialize old");
        let meta = ContextMeta::try_from_slice(&bytes).expect("deserialize new");
        assert_eq!(meta.authored_remaining, 0);
        assert_eq!(meta.application, app);
        assert_eq!(meta.root_hash, [3u8; 32]);
        assert_eq!(meta.service_name.as_deref(), Some("svc"));
    }

    // A new row round-trips the field.
    #[test]
    fn new_row_roundtrips_authored_remaining() {
        let mut meta = ContextMeta::new(
            key::ApplicationMeta::new(ApplicationId::from([7u8; 32])),
            [0u8; 32],
            vec![],
            None,
        );
        meta.authored_remaining = 5;
        let bytes = borsh::to_vec(&meta).expect("serialize new");
        let back = ContextMeta::try_from_slice(&bytes).expect("deserialize");
        assert_eq!(back.authored_remaining, 5);
    }
}
