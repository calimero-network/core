//! Snapshot sync types (CIP §6 - Snapshot Sync Constraints).
//!
//! Types for snapshot-based synchronization.
//!
//! Wire protocol types (StreamMessage, InitPayload, MessagePayload) are in [`super::wire`].
//!
//! # When to Use
//!
//! - **ONLY** for fresh nodes with NO existing state (Invariant I5)
//! - When delta history is pruned and state-based sync is impossible
//! - For initial bootstrap of new nodes joining a context
//!
//! # Critical Invariants
//!
//! - **I5**: Initialized nodes MUST use CRDT merge, NEVER snapshot overwrite
//! - **I7**: Root hash MUST be verified BEFORE applying any snapshot data
//!
//! # Validation
//!
//! All types have `is_valid()` methods that should be called after deserializing
//! from untrusted sources to prevent resource exhaustion attacks.

use std::borrow::Cow;

use borsh::{BorshDeserialize, BorshSerialize};
use calimero_context_config::types::GovernanceParentEdge;
use calimero_crypto::Nonce;
use calimero_network_primitives::specialized_node_invite::SpecializedNodeType;
use calimero_primitives::context::ContextId;
use calimero_primitives::hash::Hash;
use calimero_primitives::identity::PublicKey;

use super::hash_comparison::LeafMetadata;

// =============================================================================
// Constants
// =============================================================================

/// Default page size for snapshot transfer (256 KB).
///
/// Balances between memory usage and transfer efficiency.
pub const DEFAULT_SNAPSHOT_PAGE_SIZE: u32 = 256 * 1024;

/// Maximum page size for snapshot transfer (4 MB).
///
/// Limits memory usage for individual pages to prevent DoS attacks.
pub const MAX_SNAPSHOT_PAGE_SIZE: u32 = 4 * 1024 * 1024;

/// Maximum entities per snapshot page.
///
/// Limits the size of `SnapshotEntityPage::entities` to prevent
/// memory exhaustion from malicious peers.
pub const MAX_ENTITIES_PER_PAGE: usize = 1_000;

/// Maximum total pages in a snapshot transfer.
///
/// Prevents unbounded memory allocation during snapshot reception.
/// At 256KB per page, this allows ~2.5GB total transfer.
pub const MAX_SNAPSHOT_PAGES: usize = 10_000;

/// Maximum entity data size (1 MB).
///
/// Limits individual entity payload to prevent memory exhaustion.
pub const MAX_ENTITY_DATA_SIZE: usize = 1_048_576;

/// Maximum DAG heads in a snapshot completion message.
///
/// Limits the size of `SnapshotComplete::dag_heads`.
pub const MAX_DAG_HEADS: usize = 100;

/// Maximum compressed payload size (8 MB).
///
/// Limits the size of compressed snapshot page payloads to prevent
/// memory exhaustion before decompression. Set higher than uncompressed
/// limit to allow for edge cases where compression expands data.
pub const MAX_COMPRESSED_PAYLOAD_SIZE: usize = 8 * 1024 * 1024;

// =============================================================================
// Snapshot Record Types
// =============================================================================

// Note: the snapshot boundary/stream/page request and response shapes live on
// the wire protocol enums [`super::wire::InitPayload`] and
// [`super::wire::MessagePayload`] (variants `SnapshotBoundaryRequest`,
// `SnapshotBoundaryResponse`, `SnapshotStreamRequest`, `SnapshotPage`). The
// message-side variants use `Cow<[u8]>` so pages can be borrowed zero-copy on
// send, which standalone owned structs could not express — hence there are no
// separate struct definitions here.

/// A single record inside a snapshot page payload (the lz4-compressed
/// sequence of borsh-encoded records carried by
/// [`super::wire::MessagePayload::SnapshotPage`]).
///
/// Pre-#2387 the page payload was an opaque `(state_key, value)`
/// stream where `state_key = sha256(discriminator || id)`. That
/// made per-entity signature verification impossible on the
/// receiver — the entity id and which `Key::*` variant a record
/// belonged to were not recoverable from the wire.
///
/// Post-#2387 the wire carries entity id and record kind
/// explicitly so the receiver can:
///
/// 1. Group `Entry` (data) + `Index` (EntityIndex with the signed
///    `signature_data`) records by id.
/// 2. Verify the writer's signature against the metadata + data
///    via [`Interface::verify_snapshot_entity_signature`].
/// 3. Reject any tampered or unsigned entity record before
///    `handle.put` lands the bytes.
///
/// Non-entity records (per-entity rotation log, local sync-state
/// pointers) ship as [`SnapshotRecord::Auxiliary`] — they're
/// either implicit-from-the-signed-entity (rotation log) or
/// local-state-ish (sync state) and not individually verifiable.
/// A hand-written [`BorshDeserialize`] (not the derive) keeps the trailing
/// `Entity.schema_app_key` field backward-compatible: a peer running the
/// pre-#2539 binary serialises `Entity` as `{id, entry, index}` and stops, so
/// the reader must tolerate a clean EOF at that boundary and decode
/// `schema_app_key` as `None`. This mirrors the
/// `LeafMetadata.schema_app_key` / `GroupUpgradeValue.cascade_hlc`
/// backward-compatible-trailing-field precedent.
#[derive(Clone, Debug, PartialEq, BorshSerialize)]
pub enum SnapshotRecord {
    /// An entity's `Entry` (data) and `Index` (metadata) shipped
    /// together so the receiver can verify the signature before
    /// persisting either blob. Both bytes are required: signature
    /// verification needs the metadata in `index`, and applying the
    /// entity needs `entry`.
    Entity {
        /// The entity's 32-byte id.
        id: [u8; 32],
        /// Raw bytes for `Key::Entry(id)` — the entity's data
        /// blob as `save_internal` persisted it.
        entry: Vec<u8>,
        /// Borsh-serialized `EntityIndex` for `Key::Index(id)` —
        /// carries the `Metadata` with the writer's signed
        /// `signature_data` inside `storage_type`.
        index: Vec<u8>,
        /// App-schema (loaded-reader) key the **sender** was running when it
        /// emitted this entity — `blob_id(loaded bytecode)`, the same
        /// discriminator the state-delta fence keys on
        /// (`loaded_reader_app_key`).
        ///
        /// PR-6b / #2539 sync-repair coverage. The snapshot apply path writes
        /// each verified entity via a raw `handle.put` — it deliberately does
        /// NOT route through `apply_leaf_with_crdt_merge`, so without this
        /// marker a receiver still on an older reader would persist unreadable
        /// future-schema bytes (the "v1-binary-fed-v2-bytes" corruption
        /// hazard). With it, the receiver declines + buffers any entity whose
        /// `schema_app_key` differs from its **loaded** reader into the absorb
        /// buffer, rather than storing it.
        ///
        /// `None` for legacy peers (pre-#2539 binary) — treated as "no newer
        /// schema" → apply. Defaulted via the hand-written backward-compatible
        /// `BorshDeserialize`.
        schema_app_key: Option<[u8; 32]>,
    },
    /// Auxiliary state keyed under the same context but not
    /// signature-verifiable per record. Currently used for:
    ///
    /// * `kind = 2`: `Key::SyncState(id)` — last-sync-with-peer
    ///   pointers. Local-state-adjacent; preserved on snapshot
    ///   to avoid resetting receiver-side sync timers.
    /// * `kind = 3`: `Key::RotationLog(id)` — per-entity writer
    ///   rotation history. Its authenticity is implicit from the
    ///   signed entity's writer set (the rotation log just records
    ///   transitions between writer-set-signed states).
    ///
    /// The receiver re-derives the storage key via
    /// `Key::SyncState(id).to_bytes()` /
    /// `Key::RotationLog(id).to_bytes()` and writes through.
    Auxiliary {
        /// Discriminator byte from `calimero_storage::store::Key`.
        kind: u8,
        /// The entity / context id this record is keyed under.
        id: [u8; 32],
        /// Raw value bytes from the source peer's store.
        value: Vec<u8>,
    },
}

impl BorshDeserialize for SnapshotRecord {
    fn deserialize_reader<R: borsh::io::Read>(reader: &mut R) -> borsh::io::Result<Self> {
        // Mirror the derived enum encoding: a u8 variant discriminant followed
        // by the variant's fields in declaration order.
        let variant = u8::deserialize_reader(reader)?;
        match variant {
            0 => {
                let id = <[u8; 32]>::deserialize_reader(reader)?;
                let entry = Vec::<u8>::deserialize_reader(reader)?;
                let index = Vec::<u8>::deserialize_reader(reader)?;
                // Backward-compatible trailing field (#2539): legacy peers stop
                // after `index`, so a clean EOF here means `schema_app_key =
                // None`. The byte present at this position (if any) is the
                // `Option` discriminant: 0 = None, 1 = Some([u8; 32]). Same
                // scheme as `LeafMetadata.schema_app_key`.
                let mut first = [0u8; 1];
                let schema_app_key =
                    match crate::sync::hash_comparison::read_option_tag(reader, &mut first)? {
                        None => None,
                        Some(0) => None,
                        Some(1) => Some(<[u8; 32]>::deserialize_reader(reader)?),
                        Some(tag) => {
                            return Err(borsh::io::Error::new(
                                borsh::io::ErrorKind::InvalidData,
                                format!(
                                    "invalid Option tag {tag} for \
                                     SnapshotRecord::Entity.schema_app_key"
                                ),
                            ))
                        }
                    };
                Ok(Self::Entity {
                    id,
                    entry,
                    index,
                    schema_app_key,
                })
            }
            1 => {
                let kind = u8::deserialize_reader(reader)?;
                let id = <[u8; 32]>::deserialize_reader(reader)?;
                let value = Vec::<u8>::deserialize_reader(reader)?;
                Ok(Self::Auxiliary { kind, id, value })
            }
            other => Err(borsh::io::Error::new(
                borsh::io::ErrorKind::InvalidData,
                format!("invalid SnapshotRecord variant discriminant {other}"),
            )),
        }
    }
}

/// Snapshot record kind discriminators — mirror the
/// `calimero_storage::store::Key` enum tags so the receiver can
/// reconstruct the storage key from a `SnapshotRecord::Auxiliary`.
pub mod snapshot_record_kind {
    /// `Key::Index(id)` — not used in `Auxiliary` (Index is shipped
    /// inside `Entity`); kept here for completeness.
    pub const INDEX: u8 = 0;
    /// `Key::Entry(id)` — not used in `Auxiliary` (Entry is shipped
    /// inside `Entity`); kept here for completeness.
    pub const ENTRY: u8 = 1;
    /// `Key::SyncState(id)` — last-sync timestamps.
    pub const SYNC_STATE: u8 = 2;
    /// `Key::RotationLog(id)` — per-entity writer rotation history.
    pub const ROTATION_LOG: u8 = 3;
}

/// Cursor for resuming snapshot pagination.
#[derive(Clone, Debug, PartialEq, BorshSerialize, BorshDeserialize)]
pub struct SnapshotCursor {
    /// Last key sent in canonical order.
    pub last_key: [u8; 32],
}

// =============================================================================
// Snapshot Bootstrap Types (CIP §6 - Snapshot Sync Constraints)
// =============================================================================

/// Request to initiate a full snapshot transfer.
///
/// CRITICAL: This is ONLY for fresh nodes with NO existing state.
/// Invariant I5: Initialized nodes MUST use CRDT merge, not snapshot overwrite.
#[derive(Clone, Debug, PartialEq, BorshSerialize, BorshDeserialize)]
pub struct SnapshotRequest {
    /// Whether to compress the snapshot data.
    pub compressed: bool,

    /// Maximum page size in bytes (0 = use responder's default).
    pub max_page_size: u32,

    /// Whether the initiator is definitely a fresh node (for safety check).
    /// If false, responder SHOULD verify this claim.
    pub is_fresh_node: bool,
}

impl SnapshotRequest {
    /// Create a request for compressed snapshot.
    #[must_use]
    pub fn compressed() -> Self {
        Self {
            compressed: true,
            max_page_size: 0,
            is_fresh_node: true,
        }
    }

    /// Create a request for uncompressed snapshot.
    #[must_use]
    pub fn uncompressed() -> Self {
        Self {
            compressed: false,
            max_page_size: 0,
            is_fresh_node: true,
        }
    }

    /// Set maximum page size.
    #[must_use]
    pub fn with_max_page_size(mut self, size: u32) -> Self {
        self.max_page_size = size;
        self
    }

    /// Get the validated page size.
    ///
    /// Returns DEFAULT_SNAPSHOT_PAGE_SIZE if 0, otherwise clamps to MAX.
    #[must_use]
    pub fn validated_page_size(&self) -> u32 {
        if self.max_page_size == 0 {
            DEFAULT_SNAPSHOT_PAGE_SIZE
        } else {
            self.max_page_size.min(MAX_SNAPSHOT_PAGE_SIZE)
        }
    }
}

/// A single entity in a snapshot.
///
/// Contains all information needed to reconstruct the entity.
#[derive(Clone, Debug, PartialEq, BorshSerialize, BorshDeserialize)]
pub struct SnapshotEntity {
    /// Entity ID (deterministic, based on path).
    pub id: [u8; 32],

    /// Serialized entity data.
    pub data: Vec<u8>,

    /// Entity metadata (crdt_type, timestamps, etc.).
    pub metadata: LeafMetadata,

    /// Collection ID this entity belongs to.
    pub collection_id: [u8; 32],

    /// Parent entity ID (for nested structures).
    pub parent_id: Option<[u8; 32]>,
}

impl SnapshotEntity {
    /// Create a new snapshot entity.
    #[must_use]
    pub fn new(
        id: [u8; 32],
        data: Vec<u8>,
        metadata: LeafMetadata,
        collection_id: [u8; 32],
    ) -> Self {
        Self {
            id,
            data,
            metadata,
            collection_id,
            parent_id: None,
        }
    }

    /// Set parent entity ID.
    #[must_use]
    pub fn with_parent(mut self, parent_id: [u8; 32]) -> Self {
        self.parent_id = Some(parent_id);
        self
    }

    /// Check if this is a root-level entity.
    #[must_use]
    pub fn is_root(&self) -> bool {
        self.parent_id.is_none()
    }

    /// Check if entity is within valid bounds.
    ///
    /// Validates data size to prevent memory exhaustion from malicious peers.
    /// LeafMetadata has fixed-size fields, so it's always valid if present.
    #[must_use]
    pub fn is_valid(&self) -> bool {
        self.data.len() <= MAX_ENTITY_DATA_SIZE
    }
}

/// A page of snapshot entities for paginated transfer.
#[derive(Clone, Debug, PartialEq, BorshSerialize, BorshDeserialize)]
pub struct SnapshotEntityPage {
    /// Page number (0-indexed).
    pub page_number: usize,

    /// Total number of pages (may be estimated).
    pub total_pages: usize,

    /// Entities in this page.
    pub entities: Vec<SnapshotEntity>,

    /// Whether this is the last page.
    pub is_last: bool,
}

impl SnapshotEntityPage {
    /// Create a new snapshot page.
    #[must_use]
    pub fn new(
        page_number: usize,
        total_pages: usize,
        entities: Vec<SnapshotEntity>,
        is_last: bool,
    ) -> Self {
        Self {
            page_number,
            total_pages,
            entities,
            is_last,
        }
    }

    /// Number of entities in this page.
    #[must_use]
    pub fn entity_count(&self) -> usize {
        self.entities.len()
    }

    /// Check if this page is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entities.is_empty()
    }

    /// Check if page is within valid bounds.
    ///
    /// Call this after deserializing from untrusted sources.
    #[must_use]
    pub fn is_valid(&self) -> bool {
        // Check entity count limit
        if self.entities.len() > MAX_ENTITIES_PER_PAGE {
            return false;
        }

        // Check total pages limit
        if self.total_pages > MAX_SNAPSHOT_PAGES {
            return false;
        }

        // Check page number is within bounds (page_number is 0-indexed)
        if self.total_pages > 0 && self.page_number >= self.total_pages {
            return false;
        }

        // Check is_last coherence: if is_last, must be the final page
        if self.is_last && self.total_pages > 0 && self.page_number + 1 != self.total_pages {
            return false;
        }

        // Validate all entities
        self.entities.iter().all(SnapshotEntity::is_valid)
    }
}

/// Completion marker for snapshot transfer.
///
/// Sent after all pages have been transferred.
/// Contains verification information for Invariant I7.
#[derive(Clone, Debug, PartialEq, BorshSerialize, BorshDeserialize)]
pub struct SnapshotComplete {
    /// Root hash of the complete snapshot.
    /// INVARIANT I7: MUST be verified before applying any entities.
    pub root_hash: [u8; 32],

    /// Total number of entities transferred.
    pub total_entities: usize,

    /// Total number of pages transferred.
    pub total_pages: usize,

    /// Uncompressed size in bytes.
    pub uncompressed_size: u64,

    /// Compressed size in bytes (if compression was used).
    pub compressed_size: Option<u64>,

    /// DAG heads at the time of snapshot.
    /// Used to create checkpoint delta after apply.
    pub dag_heads: Vec<[u8; 32]>,
}

impl SnapshotComplete {
    /// Create a new snapshot completion marker.
    #[must_use]
    pub fn new(
        root_hash: [u8; 32],
        total_entities: usize,
        total_pages: usize,
        uncompressed_size: u64,
    ) -> Self {
        Self {
            root_hash,
            total_entities,
            total_pages,
            uncompressed_size,
            compressed_size: None,
            dag_heads: vec![],
        }
    }

    /// Set compressed size.
    #[must_use]
    pub fn with_compressed_size(mut self, size: u64) -> Self {
        self.compressed_size = Some(size);
        self
    }

    /// Set DAG heads.
    #[must_use]
    pub fn with_dag_heads(mut self, heads: Vec<[u8; 32]>) -> Self {
        self.dag_heads = heads;
        self
    }

    /// Calculate compression ratio (if compression was used).
    #[must_use]
    pub fn compression_ratio(&self) -> Option<f64> {
        self.compressed_size
            .map(|c| c as f64 / self.uncompressed_size.max(1) as f64)
    }

    /// Check if completion is within valid bounds.
    #[must_use]
    pub fn is_valid(&self) -> bool {
        self.total_pages <= MAX_SNAPSHOT_PAGES && self.dag_heads.len() <= MAX_DAG_HEADS
    }
}

// =============================================================================
// Snapshot Verification (Invariant I7)
// =============================================================================

/// Result of verifying a snapshot.
#[derive(Clone, Debug, PartialEq)]
pub enum SnapshotVerifyResult {
    /// Verification passed - safe to apply.
    Valid,

    /// Root hash mismatch - DO NOT apply.
    RootHashMismatch {
        expected: [u8; 32],
        computed: [u8; 32],
    },

    /// Entity count mismatch.
    EntityCountMismatch { expected: usize, actual: usize },

    /// Missing pages detected.
    MissingPages { missing: Vec<usize> },
}

impl SnapshotVerifyResult {
    /// Check if verification passed.
    #[must_use]
    pub fn is_valid(&self) -> bool {
        matches!(self, Self::Valid)
    }

    /// Convert to error if invalid.
    #[must_use]
    pub fn to_error(&self) -> Option<SnapshotError> {
        match self {
            Self::Valid => None,
            Self::RootHashMismatch { expected, computed } => {
                Some(SnapshotError::RootHashMismatch {
                    expected: *expected,
                    computed: *computed,
                })
            }
            Self::EntityCountMismatch { expected, actual } => {
                Some(SnapshotError::EntityCountMismatch {
                    expected: *expected,
                    actual: *actual,
                })
            }
            Self::MissingPages { missing } => Some(SnapshotError::MissingPages {
                missing: missing.clone(),
            }),
        }
    }
}

// =============================================================================
// Snapshot Errors
// =============================================================================

/// Errors that can occur during snapshot sync.
#[derive(Clone, Debug, PartialEq, BorshSerialize, BorshDeserialize)]
pub enum SnapshotError {
    /// Peer's delta history is pruned; full snapshot required.
    SnapshotRequired,

    /// The requested boundary is invalid or no longer available.
    InvalidBoundary,

    /// Resume cursor is invalid or expired.
    ResumeCursorInvalid,

    /// Attempted to apply snapshot on a node with existing state.
    /// INVARIANT I5: Snapshot is ONLY for fresh nodes.
    SnapshotOnInitializedNode,

    /// Root hash verification failed.
    /// INVARIANT I7: Verification REQUIRED before apply.
    RootHashMismatch {
        expected: [u8; 32],
        computed: [u8; 32],
    },

    /// Snapshot transfer was interrupted.
    TransferInterrupted { pages_received: usize },

    /// Decompression failed.
    DecompressionFailed,

    /// Entity count does not match expected count.
    EntityCountMismatch { expected: usize, actual: usize },

    /// Some pages are missing from the snapshot transfer.
    MissingPages { missing: Vec<usize> },
}

// =============================================================================
// Safety Functions
// =============================================================================

/// Safety check before applying snapshot.
///
/// Returns error if the local node has existing state.
/// INVARIANT I5: Snapshot is ONLY for fresh nodes.
pub fn check_snapshot_safety(has_local_state: bool) -> Result<(), SnapshotError> {
    if has_local_state {
        Err(SnapshotError::SnapshotOnInitializedNode)
    } else {
        Ok(())
    }
}

// =============================================================================
// Wire Protocol Messages
// =============================================================================

/// Maximum byte length for governance op payloads in [`BroadcastMessage::NamespaceGovernanceDelta`].
pub const MAX_SIGNED_GROUP_OP_PAYLOAD_BYTES: usize = 64 * 1024;

/// Upper bound on a decrypted [`SealedDeltaPayload`]'s `artifact` (the
/// borsh-encoded storage delta).
///
/// This is a defense-in-depth backstop, NOT the primary limit: an inbound
/// gossip message is already capped at gossipsub's default
/// `max_transmit_size` (64 KiB), so a network-delivered delta's plaintext is
/// bounded well below this before it reaches decryption. The cap matters for
/// the buffered-replay path (which decrypts payloads loaded from local
/// storage) and as a hard ceiling against a malicious group-key holder, who
/// can seal an arbitrarily large payload that still passes AEAD.
///
/// Deliberately distinct from [`MAX_COMPRESSED_PAYLOAD_SIZE`], which bounds
/// *compressed* snapshot pages and is intentionally looser to absorb
/// compression expansion. A state-delta plaintext is uncompressed, so it gets
/// its own, tighter, named bound. Sized generously above any legitimate delta
/// (16× the gossip transmit cap) but far below a memory-exhaustion payload.
pub const MAX_STATE_DELTA_PLAINTEXT_BYTES: usize = 1024 * 1024;

/// Plaintext that gets encrypted into the `artifact` field of a
/// [`BroadcastMessage::StateDelta`].
///
/// Bundling the expected post-apply `root_hash` and the execution `events`
/// together with the storage-delta bytes means all three are sealed under the
/// group key instead of riding on the wire in cleartext. Only key holders
/// (members) can read them, which is the point: the root hash is a state
/// fingerprint and the events are the application's emitted activity —
/// broadcasting either openly would leak how a context's state evolves to
/// non-members subscribed to the gossip topic.
///
/// All three share the delta's single nonce because they are sealed together
/// in one AEAD operation; encrypting `events` as a separate field would have
/// required a second nonce to avoid GCM nonce reuse.
#[derive(Debug, BorshSerialize, BorshDeserialize)]
pub struct SealedDeltaPayload {
    /// Expected state root after the receiver applies `artifact`. Becomes the
    /// `expected_root_hash` of the reconstructed causal delta and is verified
    /// against the locally recomputed root once the delta is applied.
    pub root_hash: Hash,

    /// Borsh-encoded `calimero_storage::delta::StorageDelta` — the actual
    /// state mutation. Kept as opaque bytes here so this type doesn't need a
    /// dependency on the storage-delta layout; the receiver deserializes it
    /// after decryption.
    pub artifact: Vec<u8>,

    /// Execution events emitted during the state change, as the serialized
    /// `Vec<ExecutionEvent>` the receiver replays handlers from. `None` when
    /// the delta emitted no events. Sealed alongside `artifact` rather than
    /// sent in cleartext.
    pub events: Option<Vec<u8>>,
}

impl SealedDeltaPayload {
    /// Size guard for the wrapped `artifact` plus `events`, mirroring the
    /// `is_valid()` convention the other wire types in this module follow.
    /// Callers that deserialize a `SealedDeltaPayload` from untrusted
    /// (post-decryption) bytes should reject it when this returns `false`
    /// before deserializing the inner storage delta or replaying events.
    /// See [`MAX_STATE_DELTA_PLAINTEXT_BYTES`].
    #[must_use]
    pub fn is_valid(&self) -> bool {
        let events_len = self.events.as_ref().map_or(0, Vec::len);
        self.artifact.len().saturating_add(events_len) <= MAX_STATE_DELTA_PLAINTEXT_BYTES
    }
}

#[derive(Debug, BorshSerialize, BorshDeserialize)]
#[non_exhaustive]
#[expect(clippy::large_enum_variant, reason = "Of no consequence here")]
pub enum BroadcastMessage<'a> {
    StateDelta {
        context_id: ContextId,
        author_id: PublicKey,

        /// DAG: Unique delta ID (content hash)
        delta_id: [u8; 32],

        /// DAG: Parent delta IDs (for causal ordering)
        parent_ids: Vec<[u8; 32]>,

        /// Hybrid Logical Clock timestamp for causal ordering
        hlc: calimero_storage::logical_clock::HybridTimestamp,

        /// Encrypted delta payload — a borsh-encoded [`SealedDeltaPayload`]
        /// holding the storage-delta actions, the expected post-apply
        /// `root_hash`, AND the execution `events`. All three travel inside
        /// the ciphertext (not as cleartext fields) so they cannot be read
        /// off the gossip topic by non-members: the root hash is a state
        /// fingerprint and the events are application activity, both of which
        /// would otherwise leak how state evolves to peers who hold no group
        /// key.
        artifact: Cow<'a, [u8]>,
        nonce: Nonce,

        /// Cross-DAG reference: names the exact governance DAG cut the
        /// author relied on when producing this delta. Receivers use it
        /// to perform the apply-time authorization check (B3): "was the
        /// author a member at this cut?" Buffered (B2) when the
        /// referenced governance heads are not yet known locally.
        ///
        /// `Some(pos)` for group-context deltas; `None` for legacy
        /// non-group contexts that have no governance DAG.
        governance_position: Option<GovernanceParentEdge>,

        /// `sha256(group_key)` — identifies which group key encrypted this
        /// delta. Receivers look up the corresponding key from their local
        /// `GroupKeyEntry` store to decrypt.
        key_id: [u8; 32],

        /// Ed25519 signature by `author_id`'s identity key over the
        /// canonical [`super::delta_auth::DeltaSignaturePayload`]
        /// `(context_id, delta_id, author_id, governance_position)`.
        /// Closes the anti-impersonation gap on the delta envelope: a
        /// current group-key holder can no longer relabel a foreign
        /// delta as their own (or vice versa) — `membership_status_at`
        /// alone would accept both since both are members.
        ///
        /// `Option` for the wire-up transition. The schema is in place
        /// and receivers verify when `Some`; signing at the `execute`
        /// site is wired up in a follow-up, after which this tightens
        /// to required and `None` becomes a hard reject.
        delta_signature: Option<[u8; 64]>,

        /// `GroupMeta.app_key` the sender was executing under at the
        /// time this delta was produced. Derived from the context's
        /// owning group's meta row (`app_key = blob_id(bytecode)`);
        /// `None` for non-group contexts (no owning group) or when the
        /// meta row cannot be resolved at send time.
        ///
        /// Receivers use this field to fence stale-schema deltas after
        /// a cascade migration: a delta arriving with an `app_key` that
        /// no longer matches the local group meta was authored by a node
        /// still on the old schema and must be buffered / rejected.
        /// The fence logic itself lives in a later task — this field
        /// is stamped here so the wire carries the information.
        ///
        /// Lockstep wire addition: all merod nodes are expected to
        /// upgrade together (same assumption as the cascade GroupOp
        /// wire additions). `BroadcastMessage` is transient gossip and
        /// is not persisted, so no stored-data migration is required.
        producing_app_key: Option<[u8; 32]>,
    },

    /// Hash heartbeat for divergence detection
    ///
    /// Periodically broadcast by nodes to allow peers to detect silent divergence.
    /// If a peer has a different hash for the same DAG heads, it indicates a problem.
    HashHeartbeat {
        context_id: ContextId,
        /// Current root hash
        root_hash: Hash,
        /// Current DAG head(s)
        dag_heads: Vec<[u8; 32]>,
    },

    /// Specialized node discovery request
    ///
    /// Broadcast by a node to discover and invite specialized nodes (e.g., read-only TEE nodes).
    /// Specialized nodes receiving this will respond via request-response protocol
    /// to the message source (available from gossipsub message).
    ///
    /// Note: context_id is NOT included - it's tracked internally by the requesting
    /// node using the nonce as the lookup key.
    SpecializedNodeDiscovery {
        /// Random nonce to bind verification to this request
        nonce: [u8; 32],
        /// Type of specialized node being invited
        node_type: SpecializedNodeType,
    },

    /// Confirmation that a specialized node has joined a context
    ///
    /// Broadcast by specialized nodes on the context topic after successfully joining.
    /// The inviting node receives this and removes the pending invite entry.
    SpecializedNodeJoinConfirmation {
        /// The nonce from the original discovery request
        nonce: [u8; 32],
    },

    /// TEE node announces its attestation to join a group.
    /// Broadcast on the group gossip topic by fleet nodes after being assigned by the gatekeeper.
    TeeAttestationAnnounce {
        /// TDX attestation quote bytes
        quote_bytes: Vec<u8>,
        /// The announcing node's identity public key
        public_key: PublicKey,
        /// Group DAG head hash for freshness binding
        nonce: [u8; 32],
        /// Type of specialized node
        node_type: SpecializedNodeType,
    },

    /// Signed namespace governance operation (Phase 2 rewrite).
    ///
    /// Published on the `ns/<hex(namespace_id)>` topic. Contains a
    /// `SignedNamespaceOp` which may be a cleartext root op or an encrypted
    /// group-scoped op.
    NamespaceGovernanceDelta {
        namespace_id: [u8; 32],
        delta_id: [u8; 32],
        parent_ids: Vec<[u8; 32]>,
        /// `borsh(SignedNamespaceOp)` — must be ≤ [`MAX_SIGNED_GROUP_OP_PAYLOAD_BYTES`].
        payload: Vec<u8>,
    },

    /// Periodic heartbeat for namespace governance DAG divergence detection.
    NamespaceStateHeartbeat {
        namespace_id: [u8; 32],
        dag_heads: Vec<[u8; 32]>,
    },
}

// Wire protocol types (StreamMessage, InitPayload, MessagePayload) are in wire.rs

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sync::hash_comparison::CrdtType;

    // =========================================================================
    // Helper Functions
    // =========================================================================

    fn make_metadata() -> LeafMetadata {
        LeafMetadata::new(CrdtType::lww_register("test"), 100, [1; 32])
    }

    fn make_entity(id: u8, data: Vec<u8>) -> SnapshotEntity {
        SnapshotEntity::new([id; 32], data, make_metadata(), [2; 32])
    }

    // =========================================================================
    // SnapshotRequest Tests
    // =========================================================================

    #[test]
    fn test_snapshot_request_compressed() {
        let request = SnapshotRequest::compressed();

        assert!(request.compressed);
        assert!(request.is_fresh_node);
        assert_eq!(request.max_page_size, 0);
        assert_eq!(request.validated_page_size(), DEFAULT_SNAPSHOT_PAGE_SIZE);
    }

    #[test]
    fn test_snapshot_request_uncompressed() {
        let request = SnapshotRequest::uncompressed().with_max_page_size(1024 * 1024);

        assert!(!request.compressed);
        assert_eq!(request.max_page_size, 1024 * 1024);
        assert_eq!(request.validated_page_size(), 1024 * 1024);
    }

    #[test]
    fn test_snapshot_request_page_size_clamping() {
        let request = SnapshotRequest::compressed().with_max_page_size(u32::MAX);

        // Should clamp to MAX_SNAPSHOT_PAGE_SIZE
        assert_eq!(request.validated_page_size(), MAX_SNAPSHOT_PAGE_SIZE);
    }

    #[test]
    fn test_snapshot_request_roundtrip() {
        let request = SnapshotRequest::compressed().with_max_page_size(65536);

        let encoded = borsh::to_vec(&request).expect("serialize");
        let decoded: SnapshotRequest = borsh::from_slice(&encoded).expect("deserialize");

        assert_eq!(request, decoded);
    }

    // =========================================================================
    // SnapshotEntity Tests
    // =========================================================================

    #[test]
    fn test_snapshot_entity_new() {
        let entity = make_entity(1, vec![1, 2, 3]);

        assert_eq!(entity.id, [1; 32]);
        assert!(entity.is_root());
        assert!(entity.parent_id.is_none());
        assert!(entity.is_valid());
    }

    #[test]
    fn test_snapshot_entity_with_parent() {
        let entity = make_entity(2, vec![4, 5, 6]).with_parent([1; 32]);

        assert!(!entity.is_root());
        assert_eq!(entity.parent_id, Some([1; 32]));
        assert!(entity.is_valid());
    }

    #[test]
    fn test_snapshot_entity_validation() {
        // Valid entity
        let valid = make_entity(1, vec![1, 2, 3]);
        assert!(valid.is_valid());

        // Invalid entity: oversized data
        let oversized = make_entity(1, vec![0u8; MAX_ENTITY_DATA_SIZE + 1]);
        assert!(!oversized.is_valid());
    }

    #[test]
    fn test_snapshot_entity_roundtrip() {
        let entity = make_entity(3, vec![7, 8, 9]).with_parent([2; 32]);

        let encoded = borsh::to_vec(&entity).expect("serialize");
        let decoded: SnapshotEntity = borsh::from_slice(&encoded).expect("deserialize");

        assert_eq!(entity, decoded);
    }

    // =========================================================================
    // SnapshotEntityPage Tests
    // =========================================================================

    #[test]
    fn test_snapshot_entity_page() {
        let entity1 = make_entity(1, vec![1, 2]);
        let entity2 = make_entity(2, vec![3, 4]);

        let page = SnapshotEntityPage::new(0, 3, vec![entity1, entity2], false);

        assert_eq!(page.page_number, 0);
        assert_eq!(page.total_pages, 3);
        assert_eq!(page.entity_count(), 2);
        assert!(!page.is_last);
        assert!(!page.is_empty());
        assert!(page.is_valid());
    }

    #[test]
    fn test_snapshot_entity_page_last() {
        let entity = make_entity(1, vec![1, 2, 3]);
        let page = SnapshotEntityPage::new(2, 3, vec![entity], true);

        assert!(page.is_last);
        assert!(page.is_valid());
    }

    #[test]
    fn test_snapshot_entity_page_empty() {
        let page = SnapshotEntityPage::new(0, 1, vec![], true);

        assert!(page.is_empty());
        assert_eq!(page.entity_count(), 0);
        assert!(page.is_valid());
    }

    #[test]
    fn test_snapshot_entity_page_validation() {
        // Valid page at entity limit
        let entities: Vec<SnapshotEntity> = (0..MAX_ENTITIES_PER_PAGE)
            .map(|i| make_entity(i as u8, vec![i as u8]))
            .collect();
        let at_limit = SnapshotEntityPage::new(0, 1, entities, true);
        assert!(at_limit.is_valid());

        // Invalid page: over entity limit
        let entities: Vec<SnapshotEntity> = (0..=MAX_ENTITIES_PER_PAGE)
            .map(|i| make_entity(i as u8, vec![i as u8]))
            .collect();
        let over_limit = SnapshotEntityPage::new(0, 1, entities, true);
        assert!(!over_limit.is_valid());

        // Invalid page: over total pages limit
        let entity = make_entity(1, vec![1]);
        let over_pages = SnapshotEntityPage::new(0, MAX_SNAPSHOT_PAGES + 1, vec![entity], false);
        assert!(!over_pages.is_valid());

        // Invalid page: page_number >= total_pages
        let entity = make_entity(1, vec![1]);
        let invalid_page_num = SnapshotEntityPage::new(5, 3, vec![entity], false);
        assert!(!invalid_page_num.is_valid());

        // Invalid page: is_last but not the final page
        let entity = make_entity(1, vec![1]);
        let invalid_last = SnapshotEntityPage::new(0, 3, vec![entity], true);
        assert!(!invalid_last.is_valid());

        // Valid page: is_last and is the final page
        let entity = make_entity(1, vec![1]);
        let valid_last = SnapshotEntityPage::new(2, 3, vec![entity], true);
        assert!(valid_last.is_valid());
    }

    #[test]
    fn test_snapshot_entity_page_roundtrip() {
        let entity = make_entity(4, vec![10, 11]);
        let page = SnapshotEntityPage::new(1, 5, vec![entity], false);

        let encoded = borsh::to_vec(&page).expect("serialize");
        let decoded: SnapshotEntityPage = borsh::from_slice(&encoded).expect("deserialize");

        assert_eq!(page, decoded);
    }

    // =========================================================================
    // SnapshotComplete Tests
    // =========================================================================

    #[test]
    fn test_snapshot_complete() {
        let complete = SnapshotComplete::new([1; 32], 1000, 10, 1024 * 1024)
            .with_compressed_size(256 * 1024)
            .with_dag_heads(vec![[2; 32], [3; 32]]);

        assert_eq!(complete.root_hash, [1; 32]);
        assert_eq!(complete.total_entities, 1000);
        assert_eq!(complete.total_pages, 10);
        assert_eq!(complete.dag_heads.len(), 2);
        assert!(complete.is_valid());

        // Compression ratio: 256KB / 1MB = 0.25
        let ratio = complete.compression_ratio().unwrap();
        assert!((ratio - 0.25).abs() < 0.01);
    }

    #[test]
    fn test_snapshot_complete_no_compression() {
        let complete = SnapshotComplete::new([1; 32], 100, 1, 10000);

        assert!(complete.compression_ratio().is_none());
        assert!(complete.is_valid());
    }

    #[test]
    fn test_snapshot_complete_validation() {
        // Valid completion
        let valid = SnapshotComplete::new([1; 32], 1000, 10, 1024 * 1024);
        assert!(valid.is_valid());

        // Invalid: too many pages
        let over_pages = SnapshotComplete::new([1; 32], 1000, MAX_SNAPSHOT_PAGES + 1, 1024);
        assert!(!over_pages.is_valid());

        // Invalid: too many DAG heads
        let heads: Vec<[u8; 32]> = (0..=MAX_DAG_HEADS).map(|i| [i as u8; 32]).collect();
        let over_heads = SnapshotComplete::new([1; 32], 1000, 10, 1024).with_dag_heads(heads);
        assert!(!over_heads.is_valid());
    }

    #[test]
    fn test_snapshot_complete_roundtrip() {
        let complete = SnapshotComplete::new([1; 32], 500, 5, 512 * 1024)
            .with_compressed_size(128 * 1024)
            .with_dag_heads(vec![[2; 32]]);

        let encoded = borsh::to_vec(&complete).expect("serialize");
        let decoded: SnapshotComplete = borsh::from_slice(&encoded).expect("deserialize");

        assert_eq!(complete, decoded);
    }

    // =========================================================================
    // SnapshotVerifyResult Tests
    // =========================================================================

    #[test]
    fn test_snapshot_verify_result_valid() {
        let result = SnapshotVerifyResult::Valid;
        assert!(result.is_valid());
        assert!(result.to_error().is_none());
    }

    #[test]
    fn test_snapshot_verify_result_hash_mismatch() {
        let result = SnapshotVerifyResult::RootHashMismatch {
            expected: [1; 32],
            computed: [2; 32],
        };
        assert!(!result.is_valid());

        let error = result.to_error().unwrap();
        assert!(matches!(error, SnapshotError::RootHashMismatch { .. }));
    }

    #[test]
    fn test_snapshot_verify_result_entity_count() {
        let result = SnapshotVerifyResult::EntityCountMismatch {
            expected: 100,
            actual: 99,
        };
        assert!(!result.is_valid());
        let error = result.to_error().unwrap();
        assert!(matches!(
            error,
            SnapshotError::EntityCountMismatch {
                expected: 100,
                actual: 99
            }
        ));
    }

    #[test]
    fn test_snapshot_verify_result_missing_pages() {
        let result = SnapshotVerifyResult::MissingPages {
            missing: vec![3, 5, 7],
        };
        assert!(!result.is_valid());
        let error = result.to_error().unwrap();
        match error {
            SnapshotError::MissingPages { missing } => {
                assert_eq!(missing, vec![3, 5, 7]);
            }
            _ => panic!("Expected MissingPages error"),
        }
    }

    // =========================================================================
    // Safety Function Tests (Invariant I5)
    // =========================================================================

    #[test]
    fn test_check_snapshot_safety_fresh_node() {
        // Fresh node (no state) - OK
        assert!(check_snapshot_safety(false).is_ok());
    }

    #[test]
    fn test_check_snapshot_safety_initialized_node() {
        // Initialized node (has state) - ERROR
        let result = check_snapshot_safety(true);
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            SnapshotError::SnapshotOnInitializedNode
        ));
    }

    // =========================================================================
    // SnapshotError Tests
    // =========================================================================

    #[test]
    fn test_snapshot_error_roundtrip() {
        let errors = vec![
            SnapshotError::SnapshotRequired,
            SnapshotError::InvalidBoundary,
            SnapshotError::ResumeCursorInvalid,
            SnapshotError::SnapshotOnInitializedNode,
            SnapshotError::RootHashMismatch {
                expected: [1; 32],
                computed: [2; 32],
            },
            SnapshotError::TransferInterrupted { pages_received: 5 },
            SnapshotError::DecompressionFailed,
            SnapshotError::EntityCountMismatch {
                expected: 100,
                actual: 99,
            },
            SnapshotError::MissingPages {
                missing: vec![3, 5, 7],
            },
        ];

        for error in errors {
            let encoded = borsh::to_vec(&error).expect("serialize");
            let decoded: SnapshotError = borsh::from_slice(&encoded).expect("deserialize");
            assert_eq!(error, decoded);
        }
    }

    // =========================================================================
    // Boundary Condition Tests
    // =========================================================================

    #[test]
    fn test_snapshot_entity_data_at_limit() {
        // Exactly at MAX_ENTITY_DATA_SIZE - should be valid
        let at_limit = make_entity(1, vec![0u8; MAX_ENTITY_DATA_SIZE]);
        assert!(at_limit.is_valid());

        // One byte over - should be invalid
        let over_limit = make_entity(1, vec![0u8; MAX_ENTITY_DATA_SIZE + 1]);
        assert!(!over_limit.is_valid());
    }

    #[test]
    fn test_snapshot_entity_page_at_entity_limit() {
        // Exactly at MAX_ENTITIES_PER_PAGE - should be valid
        let entities: Vec<SnapshotEntity> = (0..MAX_ENTITIES_PER_PAGE)
            .map(|i| make_entity((i % 256) as u8, vec![(i % 256) as u8]))
            .collect();
        let at_limit = SnapshotEntityPage::new(0, 1, entities, true);
        assert!(at_limit.is_valid());
        assert_eq!(at_limit.entity_count(), MAX_ENTITIES_PER_PAGE);

        // One entity over - should be invalid
        let entities: Vec<SnapshotEntity> = (0..=MAX_ENTITIES_PER_PAGE)
            .map(|i| make_entity((i % 256) as u8, vec![(i % 256) as u8]))
            .collect();
        let over_limit = SnapshotEntityPage::new(0, 1, entities, true);
        assert!(!over_limit.is_valid());
    }

    #[test]
    fn test_snapshot_complete_at_page_limit() {
        // Exactly at MAX_SNAPSHOT_PAGES - should be valid
        let at_limit = SnapshotComplete::new([1; 32], 1000, MAX_SNAPSHOT_PAGES, 1024);
        assert!(at_limit.is_valid());

        // One page over - should be invalid
        let over_limit = SnapshotComplete::new([1; 32], 1000, MAX_SNAPSHOT_PAGES + 1, 1024);
        assert!(!over_limit.is_valid());
    }

    #[test]
    fn test_snapshot_complete_at_dag_heads_limit() {
        // Exactly at MAX_DAG_HEADS - should be valid
        let heads: Vec<[u8; 32]> = (0..MAX_DAG_HEADS).map(|i| [(i % 256) as u8; 32]).collect();
        let at_limit = SnapshotComplete::new([1; 32], 1000, 10, 1024).with_dag_heads(heads);
        assert!(at_limit.is_valid());

        // One head over - should be invalid
        let heads: Vec<[u8; 32]> = (0..=MAX_DAG_HEADS).map(|i| [(i % 256) as u8; 32]).collect();
        let over_limit = SnapshotComplete::new([1; 32], 1000, 10, 1024).with_dag_heads(heads);
        assert!(!over_limit.is_valid());
    }

    // =========================================================================
    // Security / Exploit Prevention Tests
    // =========================================================================

    #[test]
    fn test_snapshot_request_memory_exhaustion_prevention() {
        // Attempt to request extremely large page size - should be clamped
        let request = SnapshotRequest::compressed().with_max_page_size(u32::MAX);
        assert_eq!(request.validated_page_size(), MAX_SNAPSHOT_PAGE_SIZE);
    }

    #[test]
    fn test_snapshot_entity_page_cross_validation() {
        // Page containing an invalid entity should be invalid
        let invalid_entity = make_entity(1, vec![0u8; MAX_ENTITY_DATA_SIZE + 1]);
        let page = SnapshotEntityPage::new(0, 1, vec![invalid_entity], true);
        assert!(!page.is_valid());

        // Page with mix of valid and invalid entities
        let valid_entity = make_entity(1, vec![1, 2, 3]);
        let invalid_entity = make_entity(2, vec![0u8; MAX_ENTITY_DATA_SIZE + 1]);
        let mixed_page = SnapshotEntityPage::new(0, 1, vec![valid_entity, invalid_entity], true);
        assert!(!mixed_page.is_valid());
    }

    #[test]
    fn test_snapshot_complete_compression_ratio_zero_uncompressed() {
        // Edge case: zero uncompressed size (uses max(1) to prevent division by zero)
        let complete = SnapshotComplete::new([1; 32], 0, 0, 0).with_compressed_size(100);

        let ratio = complete.compression_ratio().unwrap();
        // 100 / max(0, 1) = 100.0
        assert_eq!(ratio, 100.0);
    }

    // =========================================================================
    // Special Values Tests
    // =========================================================================

    #[test]
    fn test_snapshot_entity_all_zeros() {
        let entity = SnapshotEntity::new([0u8; 32], vec![], make_metadata(), [0u8; 32]);
        assert!(entity.is_valid());
        assert!(entity.is_root());
        assert!(entity.data.is_empty());

        // Roundtrip
        let encoded = borsh::to_vec(&entity).expect("serialize");
        let decoded: SnapshotEntity = borsh::from_slice(&encoded).expect("deserialize");
        assert_eq!(entity, decoded);
    }

    #[test]
    fn test_snapshot_entity_all_ones() {
        let entity = SnapshotEntity::new([0xFF; 32], vec![0xFF; 100], make_metadata(), [0xFF; 32])
            .with_parent([0xFF; 32]);
        assert!(entity.is_valid());
        assert!(!entity.is_root());

        // Roundtrip
        let encoded = borsh::to_vec(&entity).expect("serialize");
        let decoded: SnapshotEntity = borsh::from_slice(&encoded).expect("deserialize");
        assert_eq!(entity, decoded);
    }

    #[test]
    fn test_snapshot_complete_all_zeros() {
        let complete = SnapshotComplete::new([0u8; 32], 0, 0, 0);
        assert!(complete.is_valid());
        assert!(complete.compression_ratio().is_none());
        assert!(complete.dag_heads.is_empty());

        // Roundtrip
        let encoded = borsh::to_vec(&complete).expect("serialize");
        let decoded: SnapshotComplete = borsh::from_slice(&encoded).expect("deserialize");
        assert_eq!(complete, decoded);
    }

    #[test]
    fn test_snapshot_complete_max_values() {
        let complete = SnapshotComplete::new([0xFF; 32], usize::MAX, MAX_SNAPSHOT_PAGES, u64::MAX)
            .with_compressed_size(u64::MAX);
        assert!(complete.is_valid());

        // Roundtrip
        let encoded = borsh::to_vec(&complete).expect("serialize");
        let decoded: SnapshotComplete = borsh::from_slice(&encoded).expect("deserialize");
        assert_eq!(complete, decoded);
    }

    #[test]
    fn test_snapshot_request_all_flags() {
        // Test both compressed and uncompressed
        let compressed = SnapshotRequest::compressed();
        assert!(compressed.compressed);
        assert!(compressed.is_fresh_node);

        let uncompressed = SnapshotRequest::uncompressed();
        assert!(!uncompressed.compressed);
        assert!(uncompressed.is_fresh_node);

        // Test non-fresh node flag (edge case)
        let mut not_fresh = SnapshotRequest::compressed();
        not_fresh.is_fresh_node = false;
        assert!(!not_fresh.is_fresh_node);
    }

    // =========================================================================
    // Serialization Edge Cases
    // =========================================================================

    #[test]
    fn test_snapshot_entity_page_with_many_entities_roundtrip() {
        // Test serialization with many entities (but within limit)
        let entities: Vec<SnapshotEntity> = (0..1000)
            .map(|i| make_entity((i % 256) as u8, vec![(i % 256) as u8; 10]))
            .collect();
        let page = SnapshotEntityPage::new(5, 100, entities, false);
        assert!(page.is_valid());

        let encoded = borsh::to_vec(&page).expect("serialize");
        let decoded: SnapshotEntityPage = borsh::from_slice(&encoded).expect("deserialize");

        assert_eq!(page, decoded);
        assert_eq!(decoded.entity_count(), 1000);
    }

    #[test]
    fn test_snapshot_verify_result_all_variants_behavior() {
        // Test is_valid returns correctly for all variants
        assert!(SnapshotVerifyResult::Valid.is_valid());
        assert!(!SnapshotVerifyResult::RootHashMismatch {
            expected: [1; 32],
            computed: [2; 32]
        }
        .is_valid());
        assert!(!SnapshotVerifyResult::EntityCountMismatch {
            expected: 100,
            actual: 50
        }
        .is_valid());
        assert!(!SnapshotVerifyResult::MissingPages { missing: vec![1] }.is_valid());

        // Test to_error returns None only for Valid
        assert!(SnapshotVerifyResult::Valid.to_error().is_none());
        assert!(SnapshotVerifyResult::RootHashMismatch {
            expected: [1; 32],
            computed: [2; 32]
        }
        .to_error()
        .is_some());
        assert!(SnapshotVerifyResult::EntityCountMismatch {
            expected: 100,
            actual: 50
        }
        .to_error()
        .is_some());
        assert!(SnapshotVerifyResult::MissingPages { missing: vec![1] }
            .to_error()
            .is_some());
    }

    // =========================================================================
    // Zero-Length Collection Tests
    // =========================================================================

    #[test]
    fn test_snapshot_entity_empty_data() {
        let entity = make_entity(1, vec![]);
        assert!(entity.is_valid());
        assert!(entity.data.is_empty());
    }

    #[test]
    fn test_snapshot_complete_empty_dag_heads() {
        let complete = SnapshotComplete::new([1; 32], 100, 1, 1000);
        assert!(complete.dag_heads.is_empty());
        assert!(complete.is_valid());
    }

    #[test]
    fn test_snapshot_verify_result_missing_pages_empty() {
        // Empty missing pages list
        let result = SnapshotVerifyResult::MissingPages { missing: vec![] };
        assert!(!result.is_valid()); // Still invalid even with empty list
        assert!(result.to_error().is_some());
    }

    // =========================================================================
    // Invariant Enforcement Tests
    // =========================================================================

    #[test]
    fn test_invariant_i5_snapshot_safety() {
        // I5: Snapshot ONLY for fresh nodes

        // Fresh node - allowed
        assert!(check_snapshot_safety(false).is_ok());

        // Initialized node - rejected with specific error
        let err = check_snapshot_safety(true).unwrap_err();
        assert!(matches!(err, SnapshotError::SnapshotOnInitializedNode));
    }

    #[test]
    fn test_invariant_i7_verification_errors() {
        // I7: Root hash verification required

        // Hash mismatch should produce RootHashMismatch error
        let result = SnapshotVerifyResult::RootHashMismatch {
            expected: [1; 32],
            computed: [2; 32],
        };
        let error = result.to_error().unwrap();
        match error {
            SnapshotError::RootHashMismatch { expected, computed } => {
                assert_eq!(expected, [1; 32]);
                assert_eq!(computed, [2; 32]);
            }
            _ => panic!("Expected RootHashMismatch error"),
        }
    }

    #[test]
    fn test_snapshot_error_transfer_interrupted_preserves_count() {
        let error = SnapshotError::TransferInterrupted { pages_received: 42 };
        let encoded = borsh::to_vec(&error).expect("serialize");
        let decoded: SnapshotError = borsh::from_slice(&encoded).expect("deserialize");

        match decoded {
            SnapshotError::TransferInterrupted { pages_received } => {
                assert_eq!(pages_received, 42);
            }
            _ => panic!("Expected TransferInterrupted"),
        }
    }

    #[test]
    fn test_snapshot_cursor_roundtrip() {
        let cursor = SnapshotCursor {
            last_key: [0xAB; 32],
        };

        let encoded = borsh::to_vec(&cursor).expect("serialize");
        let decoded: SnapshotCursor = borsh::from_slice(&encoded).expect("deserialize");

        assert_eq!(cursor, decoded);
        assert_eq!(decoded.last_key, [0xAB; 32]);
    }

    #[test]
    fn test_snapshot_entity_schema_app_key_defaults_none_and_round_trips() {
        // Default constructor leaves the schema marker absent (legacy semantics).
        let bare = SnapshotRecord::Entity {
            id: [1u8; 32],
            entry: vec![1, 2, 3],
            index: vec![4, 5, 6],
            schema_app_key: None,
        };
        let encoded = borsh::to_vec(&bare).expect("serialize");
        let decoded: SnapshotRecord = borsh::from_slice(&encoded).expect("deserialize");
        assert_eq!(bare, decoded);

        // A stamped schema survives the round-trip.
        let stamped = SnapshotRecord::Entity {
            id: [2u8; 32],
            entry: vec![7, 8],
            index: vec![9],
            schema_app_key: Some([7u8; 32]),
        };
        let encoded = borsh::to_vec(&stamped).expect("serialize");
        let decoded: SnapshotRecord = borsh::from_slice(&encoded).expect("deserialize");
        assert_eq!(stamped, decoded);
        match decoded {
            SnapshotRecord::Entity { schema_app_key, .. } => {
                assert_eq!(schema_app_key, Some([7u8; 32]));
            }
            SnapshotRecord::Auxiliary { .. } => panic!("expected Entity"),
        }

        // Auxiliary is unaffected by the new trailing field.
        let aux = SnapshotRecord::Auxiliary {
            kind: snapshot_record_kind::ROTATION_LOG,
            id: [3u8; 32],
            value: vec![1],
        };
        let encoded = borsh::to_vec(&aux).expect("serialize");
        let decoded: SnapshotRecord = borsh::from_slice(&encoded).expect("deserialize");
        assert_eq!(aux, decoded);
    }

    #[test]
    fn test_snapshot_entity_legacy_bytes_decode_as_none() {
        // A pre-#2539 sender serialised `Entity` as the three fields
        // {id, entry, index} with NO trailing `schema_app_key`. Reconstruct
        // exactly those legacy bytes and confirm they decode with the marker
        // absent (clean EOF tolerated).
        let id = [9u8; 32];
        let entry = vec![1u8, 2, 3];
        let index = vec![4u8, 5];
        let mut legacy = Vec::new();
        // Enum discriminant for `Entity` (variant 0).
        legacy.push(0u8);
        legacy.extend_from_slice(&id);
        legacy.extend_from_slice(&borsh::to_vec(&entry).unwrap());
        legacy.extend_from_slice(&borsh::to_vec(&index).unwrap());
        // No trailing Option byte — legacy stops here.

        let decoded: SnapshotRecord = borsh::from_slice(&legacy).expect("deserialize legacy");
        assert_eq!(
            decoded,
            SnapshotRecord::Entity {
                id,
                entry,
                index,
                schema_app_key: None,
            }
        );
    }
}
