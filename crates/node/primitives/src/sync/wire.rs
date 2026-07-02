//! Wire protocol types for sync stream communication.
//!
//! This module contains the message types used for all sync protocol
//! communication over network streams:
//!
//! - [`StreamMessage`]: Top-level message wrapper (Init or Message)
//! - [`InitPayload`]: Initial request types (blob share, key share, delta, snapshot, etc.)
//! - [`MessagePayload`]: Response and follow-up message types
//!
//! # Protocol Flow
//!
//! ```text
//! Initiator                              Responder
//! │                                            │
//! │ ── StreamMessage::Init { payload } ──────► │
//! │                                            │
//! │ ◄── StreamMessage::Message { payload } ── │
//! │                                            │
//! │ ... (continue as needed) ...               │
//! └────────────────────────────────────────────┘
//! ```
//!
//! # Adding New Protocols
//!
//! To add a new sync protocol's wire messages:
//! 1. Add request variant to [`InitPayload`]
//! 2. Add response variant(s) to [`MessagePayload`]
//! 3. Update re-exports in `sync.rs`

use std::borrow::Cow;

use borsh::{BorshDeserialize, BorshSerialize};
use calimero_crypto::Nonce;
use calimero_primitives::blobs::BlobId;
use calimero_primitives::context::ContextId;
use calimero_primitives::hash::Hash;
use calimero_primitives::identity::PublicKey;

use super::hash_comparison::TreeNode;
use super::levelwise::LevelNode;
use super::snapshot::SnapshotError;

/// Maximum depth allowed in TreeNodeRequest.
///
/// Prevents malicious peers from requesting expensive deep traversals.
/// Handlers should validate against this limit before processing.
pub const MAX_TREE_REQUEST_DEPTH: u8 = 16;

// =============================================================================
// Stream Message Wrapper
// =============================================================================

/// Top-level message for sync stream communication.
///
/// All sync protocol messages are wrapped in this enum, which provides:
/// - Context and identity information (in Init)
/// - Sequence tracking (in Message)
/// - Nonce for replay protection
#[derive(Debug, BorshSerialize, BorshDeserialize)]
pub enum StreamMessage<'a> {
    /// Initial message to start a sync operation.
    Init {
        /// Context being synchronized.
        context_id: ContextId,
        /// Identity of the sending party.
        party_id: PublicKey,
        /// The specific request payload.
        payload: InitPayload,
        /// Nonce for the next message.
        next_nonce: Nonce,
        /// Proof that the sender controls `party_id`, bound to the dialer's
        /// transport `PeerId` (see [`InitProof`]). `None` on handshake acks,
        /// sentinel/keyless party ids, and pre-upgrade peers. The responder
        /// **requires** a valid proof before serving state-read requests
        /// (deltas, DAG heads, snapshots, tree/level nodes) or acting on a
        /// namespace/subgroup join, and ignores it on paths whose payload is
        /// already bound to the claimed identity by other means (e.g. blob and
        /// group-key shares are ECDH-wrapped to `party_id`).
        ///
        /// # Wire Format Change
        ///
        /// New trailing field on the `Init` variant. Borsh is positional, so
        /// pre-upgrade peers cannot deserialize an `Init` carrying it and vice
        /// versa — a coordinated network upgrade is required, the same
        /// constraint documented on `Message::sequence_id` and the
        /// `NotMaterialized` variant.
        pop: Option<InitProof>,
    },
    /// Follow-up message in an ongoing sync operation.
    Message {
        /// Sequence number for ordering.
        ///
        /// # Wire Format Change
        ///
        /// Changed from `usize` to `u64` for cross-platform portability.
        /// This is a breaking wire format change - nodes must be upgraded
        /// together to avoid deserialization failures.
        sequence_id: u64,
        /// The message payload.
        payload: MessagePayload<'a>,
        /// Nonce for the next message.
        next_nonce: Nonce,
    },
    /// Opaque error - reveals nothing about node state.
    ///
    /// Used when something goes wrong but we don't want to leak
    /// information to potentially malicious peers (e.g. unverified
    /// dialer, cross-namespace stream leak).
    OpaqueError,
    /// Typed "I am a valid peer for this context but haven't materialised
    /// it locally yet" response. Sent by the receiver when the inbound
    /// stream's `dialer_verified` check passed (the dialer IS a member of
    /// the context's group) but the receiver itself has no local entry
    /// for the context — e.g. they opted out of auto-follow, or the
    /// `JoinContext` is still in flight.
    ///
    /// The initiator MUST treat this as benign: do not increment
    /// `failure_count`, do not apply exponential backoff, just drop this
    /// peer for this round and continue. Closes the Ronit/Fran incident
    /// path where namespace-fallback peer selection picked a non-following
    /// peer and the resulting `OpaqueError` cascaded into 256s backoff
    /// against a peer that fundamentally cannot serve the context.
    ///
    /// # Wire Format Change
    ///
    /// New variant at index 3, appended to the enum tail. Borsh enum
    /// serialization is by declaration order, so old nodes will fail to
    /// deserialize this variant. Coordinated upgrade required across the
    /// network — same constraint as the `sequence_id` u64 change
    /// documented on the `Message` variant. Pre-upgrade peers fall back
    /// to the previous behaviour (no benign signal) and continue working
    /// for all other variants.
    NotMaterialized,
}

// =============================================================================
// Init Proof (transport-bound proof of possession)
// =============================================================================

/// Proof that the sender of an [`StreamMessage::Init`] controls the private
/// key of the identity it names in `party_id`, bound to the dialer's transport
/// `PeerId`.
///
/// # Why
///
/// The responder serves context state — DAG heads, snapshots, deltas, tree and
/// level nodes — and performs namespace/subgroup pre-registration keyed off the
/// `party_id` (or `joiner_public_key`) carried in the `Init`. Those values are
/// attacker-chosen: without a proof, any peer that learns a member's public key
/// can name it and be served as that member, or register an identity it does
/// not control. Membership checks alone don't help — they only confirm the
/// *named* identity is a member, not that the *caller* holds it.
///
/// # Construction
///
/// An Ed25519 signature by `party_id`'s key over [`InitProof::message`], which
/// binds:
/// - the domain separator (protocol/version),
/// - the `context_id`,
/// - the claimed `party_id`,
/// - the **initiator's own** libp2p `PeerId` bytes.
///
/// # Why it resists replay
///
/// The bound `PeerId` is the dialer's. The responder recomputes the message
/// with the `PeerId` it observes on the transport — which libp2p's noise
/// handshake authenticates — so a proof captured from one member cannot be
/// presented by a different peer (the observed `PeerId` would differ), and a
/// caller cannot forge a proof for an identity whose key it lacks. The proof is
/// independent of the payload and nonce, so one signature is reusable for every
/// request a node issues for a given (context, identity) — it is a capability
/// to *speak as* that identity from that peer, not a per-message token.
#[derive(Clone, Copy, Debug, BorshSerialize, BorshDeserialize)]
pub struct InitProof {
    /// Ed25519 signature over [`InitProof::message`] by `party_id`'s key.
    pub signature: [u8; 64],
}

impl InitProof {
    /// Domain separator for the signed message. Bump the version suffix on any
    /// change to the signed layout so proofs never cross protocol versions.
    pub const DOMAIN: &'static [u8] = b"calimero:sync:init-pop:v1";

    /// Canonical bytes the [`signature`](InitProof::signature) covers:
    /// `DOMAIN ‖ context_id ‖ party_id ‖ dialer_peer_id`.
    ///
    /// `dialer_peer_id` must be the raw `libp2p::PeerId` bytes
    /// (`PeerId::to_bytes()`) of the node that opens the stream — the signer on
    /// the initiator side, the transport-observed peer on the responder side.
    #[must_use]
    pub fn message(context_id: &ContextId, party_id: &PublicKey, dialer_peer_id: &[u8]) -> Vec<u8> {
        let mut msg = Vec::with_capacity(Self::DOMAIN.len() + 32 + 32 + dialer_peer_id.len());
        msg.extend_from_slice(Self::DOMAIN);
        msg.extend_from_slice(context_id.digest());
        msg.extend_from_slice(party_id.digest());
        msg.extend_from_slice(dialer_peer_id);
        msg
    }

    /// Verify this proof authorizes `party_id` to speak for `context_id` from
    /// the peer identified by `dialer_peer_id` (raw `PeerId` bytes).
    #[must_use]
    pub fn verify(
        &self,
        context_id: &ContextId,
        party_id: &PublicKey,
        dialer_peer_id: &[u8],
    ) -> bool {
        let message = Self::message(context_id, party_id, dialer_peer_id);
        party_id
            .verify_raw_signature(&message, &self.signature)
            .is_ok()
    }
}

// =============================================================================
// Init Payload (Requests)
// =============================================================================

/// Initial request payloads for various sync protocols.
///
/// Each variant represents a different type of sync request that can
/// be initiated by a node.
#[derive(Clone, Debug, BorshSerialize, BorshDeserialize)]
pub enum InitPayload {
    /// Request to share a blob.
    BlobShare {
        /// ID of the blob to share.
        blob_id: BlobId,
    },

    /// Request a specific delta by ID (for DAG gap filling).
    DeltaRequest {
        /// Context for the delta.
        context_id: ContextId,
        /// ID of the specific delta to request.
        delta_id: [u8; 32],
    },

    /// Request peer's current DAG heads for catchup.
    DagHeadsRequest {
        /// Context to get DAG heads for.
        context_id: ContextId,
    },

    /// Request snapshot boundary negotiation.
    SnapshotBoundaryRequest {
        /// Context for snapshot sync.
        context_id: ContextId,
        /// Optional requested cutoff timestamp.
        requested_cutoff_timestamp: Option<u64>,
    },

    /// Request to stream snapshot pages.
    SnapshotStreamRequest {
        /// Context for snapshot sync.
        context_id: ContextId,
        /// Root hash that was negotiated in boundary request.
        boundary_root_hash: Hash,
        /// Maximum pages per response.
        page_limit: u16,
        /// Maximum bytes per response.
        byte_limit: u32,
        /// Resume cursor from previous page (for pagination).
        resume_cursor: Option<Vec<u8>>,
    },

    /// Request tree node(s) for HashComparison sync (CIP §4).
    ///
    /// Used by the HashComparison protocol to request subtrees from a peer
    /// for Merkle tree comparison.
    TreeNodeRequest {
        /// Context being synchronized.
        context_id: ContextId,
        /// ID of the node to request (root hash or entity ID).
        node_id: [u8; 32],
        /// Maximum depth to traverse from this node.
        /// None means only the requested node, Some(1) includes immediate children.
        max_depth: Option<u8>,
    },

    /// Request nodes at a specific level for LevelWise sync (CIP Appendix B).
    ///
    /// Used by the LevelWise protocol for breadth-first tree synchronization,
    /// optimized for wide, shallow trees (depth ≤ 2).
    LevelWiseRequest {
        /// Context being synchronized.
        context_id: ContextId,
        /// Level to request (0 = root's children, 1 = grandchildren, etc.).
        level: u32,
        /// Parent IDs to fetch children for.
        /// - `None` = fetch all nodes at this level
        /// - `Some(ids)` = fetch only children of specified parents
        parent_ids: Option<Vec<[u8; 32]>>,
    },

    /// Push local-only entities to the peer for bidirectional HashComparison sync.
    ///
    /// When the initiator detects subtrees that exist locally but not on the peer
    /// (`RemoteMissing` or `Different` with `local_only_children`), it collects
    /// the leaf data and pushes it to the responder via this message.
    ///
    /// The responder applies CRDT merge (Invariant I5) for each entity.
    EntityPush {
        /// Context being synchronized.
        context_id: ContextId,
        /// Leaf entities to push to the peer.
        entities: Vec<super::hash_comparison::TreeLeafData>,
    },

    /// Push entity *deletions* (tombstones) to the peer during HashComparison.
    ///
    /// The tree comparison is add-wins, so a deletion has to be propagated
    /// explicitly or a peer that still holds the entry never converges (the
    /// clear split-brain). The responder applies each via the authenticated
    /// `Action::DeleteRef` path (delete-wins by HLC).
    EntityDeletePush {
        /// Context being synchronized.
        context_id: ContextId,
        /// Tombstones to apply on the peer.
        deletions: Vec<super::hash_comparison::EntityDeletion>,
    },

    /// Request encrypted payloads for namespace governance skeletons.
    /// Used during lazy backfill when a member joins a new group and
    /// needs to decrypt previously-stored opaque skeletons.
    NamespaceBackfillRequest {
        namespace_id: [u8; 32],
        /// Delta IDs for which we have skeletons but need full payloads.
        delta_ids: Vec<[u8; 32]>,
    },

    /// Direct request to join a namespace. The joiner sends their signed
    /// invitation and public key; the responder validates and returns the
    /// group key + context list in one shot.
    NamespaceJoinRequest {
        namespace_id: [u8; 32],
        /// Borsh-serialized SignedGroupOpenInvitation
        invitation_bytes: Vec<u8>,
        /// The joiner's public key for ECDH key wrapping
        joiner_public_key: PublicKey,
    },

    /// Direct request to materialise inherited membership in an Open
    /// subgroup (issue #2357). Lets the inherited self-join path skip
    /// the gossip-only `MemberJoinedOpen` → `KeyDelivery` round-trip
    /// that times out in small clusters where the gossipsub mesh stays
    /// empty (#2293).
    ///
    /// Responder validates that `joiner_public_key` has
    /// `MembershipPath::Inherited` to `subgroup_id` (proof of
    /// Open-chain authorisation), then wraps the subgroup key via
    /// ECDH and replies with `OpenSubgroupJoinResponse`. Mirrors
    /// `NamespaceJoinRequest`'s direct-stream determinism for the
    /// inherited case.
    OpenSubgroupJoinRequest {
        namespace_id: [u8; 32],
        subgroup_id: [u8; 32],
        /// The joiner's public key for ECDH key wrapping.
        joiner_public_key: PublicKey,
    },

    /// Pull-based recovery request for a group key the requester is
    /// already an admitted member of but does not yet hold locally.
    ///
    /// This is the durable replacement for the old on-DAG `KeyDelivery`
    /// governance op: instead of an existing member *pushing* the key
    /// once (and only once) when it first applies the join, the joiner
    /// — which is online and syncing — *pulls* the key from its sync
    /// peer every sync round until it has it. The responder validates
    /// that `requester_public_key` is a current member of `group_id`,
    /// then ECDH-wraps the group key and replies with
    /// [`GroupKeyResponse`](MessagePayload::GroupKeyResponse). A
    /// responder that doesn't hold the key replies with an empty
    /// envelope and the joiner tries another peer next round.
    ///
    /// **Borsh ordering**: appended at the tail of `InitPayload` (after
    /// `RotationLogSyncRequest`) so all existing variant discriminants are
    /// unchanged.
    GroupKeyRequest {
        namespace_id: [u8; 32],
        group_id: [u8; 32],
        /// The requester's namespace identity public key, used both for
        /// the membership check and as the ECDH wrap recipient.
        requester_public_key: PublicKey,
        /// The specific key epoch the requester needs (the `key_id` of a
        /// buffered op it can't decrypt), or `None` to ask for the group's
        /// current key (a keyless joiner bootstrapping). Serving the exact
        /// epoch lets a member recover a rotated-out key its stranded op was
        /// encrypted under, which a current-key-only responder could not.
        key_id: Option<[u8; 32]>,
    },
}

// =============================================================================
// Message Payload (Responses)
// =============================================================================

/// Response and follow-up message payloads.
///
/// Each variant represents a different type of response or continuation
/// message in a sync protocol exchange.
#[derive(Debug, BorshSerialize, BorshDeserialize)]
pub enum MessagePayload<'a> {
    /// Blob data chunk.
    BlobShare {
        /// Chunk of blob data.
        chunk: Cow<'a, [u8]>,
    },

    /// Response to DeltaRequest containing the requested delta.
    DeltaResponse {
        /// The serialized delta data.
        delta: Cow<'a, [u8]>,
        /// Signing identity of the node that authored this delta.
        /// Required — the responder only serves deltas that have a
        /// recorded author (rows missing the field return
        /// `DeltaNotFound`, forcing the initiator to fall back to
        /// snapshot sync where the per-entity signature path applies).
        /// Initiator runs `membership_status_at` against this author
        /// unconditionally; there is no legacy-accept escape hatch.
        author_id: calimero_primitives::identity::PublicKey,
        /// Serialized `calimero_context_config::types::GovernanceParentEdge`
        /// (borsh bytes) at the delta's sign time. Pairs with
        /// `author_id` for the apply-time `membership_status_at` check.
        /// `None` only for non-group contexts where the author has no
        /// governance cut to cite — initiator skips the membership
        /// check in that case (there's nothing to check against).
        governance_position_blob: Option<Cow<'a, [u8]>>,
        /// Ed25519 signature by `author_id`'s identity key over the
        /// canonical [`super::delta_auth::DeltaSignaturePayload`].
        /// Closes the anti-impersonation gap on the delta envelope:
        /// without this, a current group-key holder could relabel a
        /// foreign delta as their own (or vice versa) since
        /// `membership_status_at` would pass for both members.
        ///
        /// `Option` because legacy rows (deltas authored before the
        /// envelope-signature feature landed) have no signature on
        /// file — the responder forwards `None` for those rather than
        /// withholding the delta entirely. Freshly-authored deltas
        /// always carry `Some(_)` (`internal_execute` signs against
        /// the same `governance_position` it persists on the row).
        /// Initiators MUST verify any present signature; `None` is
        /// tolerated only for that legacy-row case and will tighten
        /// to required once those rows have aged out of every peer's
        /// storage.
        delta_signature: Option<[u8; 64]>,
    },

    /// Delta not found response.
    DeltaNotFound,

    /// Response to DagHeadsRequest containing peer's current heads and root hash.
    DagHeadsResponse {
        /// Current DAG head hashes.
        dag_heads: Vec<[u8; 32]>,
        /// Current root hash (the storage-layer entity Merkle root).
        root_hash: Hash,
        /// The peer's `scope_root` for this context — `root_hash` folded with the
        /// governance projection's ACL + membership/admin hashes (unified-causal-log
        /// cutover C0 shadow). `None` when the responder can't resolve/fold the
        /// scope yet (cold projection), so the initiator skips the shadow compare
        /// rather than reading a false divergence. Observe-only: no sync decision
        /// reads this in C0; C1 promotes it to the authoritative convergence signal.
        scope_root: Option<Hash>,
    },

    /// Response to SnapshotBoundaryRequest.
    SnapshotBoundaryResponse {
        /// Authoritative boundary timestamp (nanoseconds since epoch).
        boundary_timestamp: u64,
        /// Root hash for the boundary state.
        boundary_root_hash: Hash,
        /// Peer's DAG heads at the boundary.
        dag_heads: Vec<[u8; 32]>,
    },

    /// A page of snapshot data.
    SnapshotPage {
        /// Compressed payload data.
        payload: Cow<'a, [u8]>,
        /// Uncompressed length for validation.
        uncompressed_len: u32,
        /// Cursor for resuming (None if complete).
        cursor: Option<Vec<u8>>,
        /// Total page count.
        page_count: u64,
        /// Pages sent so far.
        sent_count: u64,
        /// Grand total of shippable `Entity` records across the whole snapshot
        /// — every entity with both an `Index` and an `Entry`, counted from the
        /// sender's full boundary scan and stable across bursts. Excludes
        /// orphans that are never shipped, so it's the exact denominator the
        /// receiver's cumulative applied count reaches (percent → 100, ETA →
        /// 0). `0` means "unknown" — an empty snapshot, or a peer too old to
        /// advertise it; the receiver then reports raw progress only.
        total_records: u64,
    },

    /// Snapshot sync error.
    SnapshotError {
        /// The error that occurred.
        error: SnapshotError,
    },

    /// Response to TreeNodeRequest for HashComparison sync (CIP §4).
    ///
    /// Contains tree nodes from the requested subtree for Merkle comparison.
    TreeNodeResponse {
        /// Tree nodes in the requested subtree.
        ///
        /// For a request with max_depth=0: contains just the requested node.
        /// For max_depth=1: contains the node and its immediate children.
        nodes: Vec<TreeNode>,
        /// True if the requested node was not found.
        not_found: bool,
    },

    /// Response to LevelWiseRequest for LevelWise sync (CIP Appendix B).
    ///
    /// Contains all nodes at the requested level for breadth-first comparison.
    LevelWiseResponse {
        /// Level these nodes are at.
        level: u32,
        /// Nodes at this level.
        ///
        /// Each node includes:
        /// - `id` and `hash` for comparison
        /// - `parent_id` for tree structure
        /// - `leaf_data` if this is a leaf (includes full entity data for CRDT merge)
        nodes: Vec<LevelNode>,
        /// Whether there are more levels below this one.
        has_more_levels: bool,
        /// Authenticated tombstones for children of the queried parents that
        /// this responder has deleted. Lets a holder that still has the entity
        /// (the initiator) learn of the deletion and apply it delete-wins —
        /// the LevelWise analogue of `TreeNode::deleted_children`. Without it a
        /// cleared responder returns no nodes at level 0 and the initiator's
        /// loop exits before ever seeing the deletion (holder-initiates clear).
        deleted_children: Vec<super::hash_comparison::EntityDeletion>,
    },

    /// Acknowledgment of received EntityPush for bidirectional HashComparison sync.
    ///
    /// Sent by the responder after applying CRDT merge for pushed entities.
    EntityPushAck {
        /// Number of entities successfully applied via CRDT merge.
        applied_count: u32,
    },

    /// Acknowledgment of a received `EntityDeletePush`.
    ///
    /// Sent by the responder after applying delete-wins for pushed tombstones.
    EntityDeletePushAck {
        /// Number of deletions successfully applied (delete-wins) by the responder.
        applied_count: u32,
    },

    /// Response containing namespace governance delta payloads for backfill.
    NamespaceBackfillResponse {
        /// Pairs of (delta_id, borsh(SignedNamespaceOp)).
        /// Only includes deltas the responder has full payloads for.
        deltas: Vec<([u8; 32], Vec<u8>)>,
    },

    /// Response to NamespaceJoinRequest with everything the joiner needs.
    NamespaceJoinResponse {
        /// ECDH-wrapped group key envelope (borsh-serialized KeyEnvelope).
        /// Empty if the responder doesn't hold the group key.
        key_envelope_bytes: Vec<u8>,
        /// Context IDs registered under this namespace/group.
        context_ids: Vec<ContextId>,
        /// The application ID used by contexts in this group.
        application_id: [u8; 32],
        /// All namespace governance ops (borsh-serialized SignedNamespaceOp)
        /// so the joiner can replay the full governance history.
        governance_ops: Vec<Vec<u8>>,
        /// Namespace's `default_capabilities` value at the moment the
        /// invitation is fulfilled. Issue #2256: traveling this with the
        /// bundle eliminates the joiner-side hard-coded fallback (the
        /// `create_group` constant) so admin overrides via
        /// `DefaultCapabilitiesSet` are respected even before the
        /// governance op finishes propagating to the joiner.
        default_capabilities: u32,
    },

    /// The responder rejected the join request.
    NamespaceJoinRejected { reason: String },

    /// Response to `OpenSubgroupJoinRequest` (issue #2357). Carries the
    /// ECDH-wrapped subgroup-key envelope; empty if the responder
    /// doesn't hold the key (joiner should try another peer).
    OpenSubgroupJoinResponse {
        /// ECDH-wrapped subgroup-key envelope (borsh-serialized
        /// `KeyEnvelope`).
        key_envelope_bytes: Vec<u8>,
    },

    /// The responder rejected the open-subgroup-join request — e.g.,
    /// joiner has no `MembershipPath::Inherited` to the subgroup, or
    /// the subgroup doesn't belong to the named namespace.
    OpenSubgroupJoinRejected { reason: String },

    /// Response to [`GroupKeyRequest`](InitPayload::GroupKeyRequest).
    /// Carries the ECDH-wrapped group-key envelope; empty if the
    /// responder doesn't hold the key or the requester isn't a member
    /// (the joiner should try another peer on its next sync round).
    ///
    /// `responder_identity` is the responder's namespace identity — the
    /// trust anchor a keyless bootstrap joiner uses to seed the
    /// namespace admin when it receives the root-group key without an
    /// invitation (replaces the old KeyDelivery-signer trust anchor).
    ///
    /// **Borsh ordering**: appended at the tail of `MessagePayload` (after
    /// `RotationLogSyncResponse`) so all existing variant discriminants are
    /// unchanged.
    GroupKeyResponse {
        /// ECDH-wrapped group-key envelope (borsh-serialized
        /// `KeyEnvelope`). Empty ⇒ no key delivered.
        key_envelope_bytes: Vec<u8>,
        /// Responder's namespace identity public key (the wrap sender).
        responder_identity: PublicKey,
    },
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_init_payload_tree_node_request() {
        let request = InitPayload::TreeNodeRequest {
            context_id: ContextId::from([1u8; 32]),
            node_id: [2u8; 32],
            max_depth: Some(1),
        };

        let encoded = borsh::to_vec(&request).expect("serialize");
        let decoded: InitPayload = borsh::from_slice(&encoded).expect("deserialize");

        match decoded {
            InitPayload::TreeNodeRequest {
                context_id,
                node_id,
                max_depth,
            } => {
                assert_eq!(*context_id.as_ref(), [1u8; 32]);
                assert_eq!(node_id, [2u8; 32]);
                assert_eq!(max_depth, Some(1));
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn test_message_payload_tree_node_response() {
        use crate::sync::hash_comparison::{LeafMetadata, TreeLeafData, TreeNode};

        let leaf_data = TreeLeafData::new(
            [10u8; 32],
            vec![1, 2, 3],
            LeafMetadata::new(
                crate::sync::hash_comparison::CrdtType::lww_register("test"),
                100,
                [0u8; 32],
            ),
        );
        let node = TreeNode::leaf([1u8; 32], [2u8; 32], leaf_data);

        let response = MessagePayload::TreeNodeResponse {
            nodes: vec![node],
            not_found: false,
        };

        let encoded = borsh::to_vec(&response).expect("serialize");
        let decoded: MessagePayload = borsh::from_slice(&encoded).expect("deserialize");

        match decoded {
            MessagePayload::TreeNodeResponse { nodes, not_found } => {
                assert_eq!(nodes.len(), 1);
                assert!(!not_found);
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn test_message_payload_tree_node_response_not_found() {
        let response = MessagePayload::TreeNodeResponse {
            nodes: vec![],
            not_found: true,
        };

        let encoded = borsh::to_vec(&response).expect("serialize");
        let decoded: MessagePayload = borsh::from_slice(&encoded).expect("deserialize");

        match decoded {
            MessagePayload::TreeNodeResponse { nodes, not_found } => {
                assert!(nodes.is_empty());
                assert!(not_found);
            }
            _ => panic!("wrong variant"),
        }
    }

    // =========================================================================
    // LevelWise Wire Protocol Tests
    // =========================================================================

    #[test]
    fn test_init_payload_levelwise_request_full_level() {
        let request = InitPayload::LevelWiseRequest {
            context_id: ContextId::from([1u8; 32]),
            level: 0,
            parent_ids: None,
        };

        let encoded = borsh::to_vec(&request).expect("serialize");
        let decoded: InitPayload = borsh::from_slice(&encoded).expect("deserialize");

        match decoded {
            InitPayload::LevelWiseRequest {
                context_id,
                level,
                parent_ids,
            } => {
                assert_eq!(*context_id.as_ref(), [1u8; 32]);
                assert_eq!(level, 0);
                assert!(parent_ids.is_none());
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn test_init_payload_levelwise_request_with_parents() {
        let parents = vec![[10u8; 32], [20u8; 32], [30u8; 32]];
        let request = InitPayload::LevelWiseRequest {
            context_id: ContextId::from([2u8; 32]),
            level: 1,
            parent_ids: Some(parents.clone()),
        };

        let encoded = borsh::to_vec(&request).expect("serialize");
        let decoded: InitPayload = borsh::from_slice(&encoded).expect("deserialize");

        match decoded {
            InitPayload::LevelWiseRequest {
                context_id,
                level,
                parent_ids,
            } => {
                assert_eq!(*context_id.as_ref(), [2u8; 32]);
                assert_eq!(level, 1);
                assert_eq!(parent_ids, Some(parents));
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn test_message_payload_levelwise_response_internal_nodes() {
        use crate::sync::levelwise::LevelNode;

        let nodes = vec![
            LevelNode::internal([1u8; 32], [10u8; 32], None),
            LevelNode::internal([2u8; 32], [20u8; 32], None),
        ];

        let response = MessagePayload::LevelWiseResponse {
            level: 0,
            nodes: nodes.clone(),
            has_more_levels: true,
            deleted_children: vec![],
        };

        let encoded = borsh::to_vec(&response).expect("serialize");
        let decoded: MessagePayload = borsh::from_slice(&encoded).expect("deserialize");

        match decoded {
            MessagePayload::LevelWiseResponse {
                level,
                nodes: decoded_nodes,
                has_more_levels,
                deleted_children,
            } => {
                assert_eq!(level, 0);
                assert_eq!(decoded_nodes.len(), 2);
                assert!(has_more_levels);
                assert!(decoded_nodes[0].is_internal());
                assert!(decoded_nodes[1].is_internal());
                assert!(deleted_children.is_empty());
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn test_message_payload_levelwise_response_with_leaves() {
        use crate::sync::hash_comparison::{CrdtType, LeafMetadata, TreeLeafData};
        use crate::sync::levelwise::LevelNode;

        let metadata = LeafMetadata::new(CrdtType::lww_register("test"), 100, [0u8; 32]);
        let leaf_data = TreeLeafData::new([5u8; 32], vec![1, 2, 3, 4], metadata);

        let nodes = vec![
            LevelNode::internal([1u8; 32], [10u8; 32], None),
            LevelNode::leaf([2u8; 32], [20u8; 32], Some([1u8; 32]), leaf_data),
        ];

        let response = MessagePayload::LevelWiseResponse {
            level: 1,
            nodes,
            has_more_levels: false,
            deleted_children: vec![],
        };

        let encoded = borsh::to_vec(&response).expect("serialize");
        let decoded: MessagePayload = borsh::from_slice(&encoded).expect("deserialize");

        match decoded {
            MessagePayload::LevelWiseResponse {
                level,
                nodes: decoded_nodes,
                has_more_levels,
                deleted_children: _,
            } => {
                assert_eq!(level, 1);
                assert_eq!(decoded_nodes.len(), 2);
                assert!(!has_more_levels);
                assert!(decoded_nodes[0].is_internal());
                assert!(decoded_nodes[1].is_leaf());
                assert_eq!(decoded_nodes[1].parent_id, Some([1u8; 32]));
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn test_message_payload_levelwise_response_empty() {
        let response = MessagePayload::LevelWiseResponse {
            level: 2,
            nodes: vec![],
            has_more_levels: false,
            deleted_children: vec![],
        };

        let encoded = borsh::to_vec(&response).expect("serialize");
        let decoded: MessagePayload = borsh::from_slice(&encoded).expect("deserialize");

        match decoded {
            MessagePayload::LevelWiseResponse {
                level,
                nodes,
                has_more_levels,
                deleted_children,
            } => {
                assert_eq!(level, 2);
                assert!(nodes.is_empty());
                assert!(!has_more_levels);
                assert!(deleted_children.is_empty());
            }
            _ => panic!("wrong variant"),
        }
    }

    // =========================================================================
    // EntityPush / EntityPushAck Wire Protocol Tests
    // =========================================================================

    #[test]
    fn test_init_payload_entity_push_roundtrip() {
        use crate::sync::hash_comparison::{CrdtType, LeafMetadata, TreeLeafData};

        let metadata = LeafMetadata::new(CrdtType::lww_register("test"), 100, [0u8; 32]);
        let leaf = TreeLeafData::new([10u8; 32], vec![1, 2, 3, 4], metadata);

        let request = InitPayload::EntityPush {
            context_id: ContextId::from([5u8; 32]),
            entities: vec![leaf],
        };

        let encoded = borsh::to_vec(&request).expect("serialize");
        let decoded: InitPayload = borsh::from_slice(&encoded).expect("deserialize");

        match decoded {
            InitPayload::EntityPush {
                context_id,
                entities,
            } => {
                assert_eq!(*context_id.as_ref(), [5u8; 32]);
                assert_eq!(entities.len(), 1);
                assert_eq!(entities[0].key, [10u8; 32]);
                assert_eq!(entities[0].value, vec![1, 2, 3, 4]);
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn test_init_payload_entity_push_empty() {
        let request = InitPayload::EntityPush {
            context_id: ContextId::from([1u8; 32]),
            entities: vec![],
        };

        let encoded = borsh::to_vec(&request).expect("serialize");
        let decoded: InitPayload = borsh::from_slice(&encoded).expect("deserialize");

        match decoded {
            InitPayload::EntityPush { entities, .. } => {
                assert!(entities.is_empty());
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn test_message_payload_entity_push_ack_roundtrip() {
        let response = MessagePayload::EntityPushAck { applied_count: 42 };

        let encoded = borsh::to_vec(&response).expect("serialize");
        let decoded: MessagePayload = borsh::from_slice(&encoded).expect("deserialize");

        match decoded {
            MessagePayload::EntityPushAck { applied_count } => {
                assert_eq!(applied_count, 42);
            }
            _ => panic!("wrong variant"),
        }
    }

    // ---------------------------------------------------------------------
    // InitProof (transport-bound proof of possession)
    // ---------------------------------------------------------------------

    use calimero_primitives::identity::PrivateKey;

    /// A deterministic keypair from a seed byte (no `rand` feature needed).
    fn test_keypair(seed: u8) -> (PrivateKey, PublicKey) {
        let sk = PrivateKey::from([seed; 32]);
        let pk = sk.public_key();
        (sk, pk)
    }

    fn sign_pop(sk: &PrivateKey, ctx: &ContextId, party: &PublicKey, peer: &[u8]) -> InitProof {
        let message = InitProof::message(ctx, party, peer);
        InitProof {
            signature: sk.sign(&message).expect("sign").to_bytes(),
        }
    }

    #[test]
    fn init_proof_verifies_for_matching_context_party_and_peer() {
        let ctx = ContextId::from([7u8; 32]);
        let (sk, pk) = test_keypair(1);
        let peer = b"peer-id-bytes-A";

        let proof = sign_pop(&sk, &ctx, &pk, peer);
        assert!(proof.verify(&ctx, &pk, peer));
    }

    #[test]
    fn init_proof_rejects_wrong_peer_id() {
        // A proof captured for one dialer must not verify when replayed from a
        // different transport peer — this is what stops identity spoofing.
        let ctx = ContextId::from([7u8; 32]);
        let (sk, pk) = test_keypair(1);

        let proof = sign_pop(&sk, &ctx, &pk, b"peer-id-bytes-A");
        assert!(!proof.verify(&ctx, &pk, b"peer-id-bytes-B"));
    }

    #[test]
    fn init_proof_rejects_wrong_party_id() {
        // Signing with one key but claiming another identity must fail: a caller
        // cannot prove possession of a key it does not hold.
        let ctx = ContextId::from([7u8; 32]);
        let (attacker_sk, _attacker_pk) = test_keypair(1);
        let (_victim_sk, victim_pk) = test_keypair(2);
        let peer = b"peer-id-bytes-A";

        // Attacker signs with its own key but stamps the victim's public key.
        let forged = sign_pop(&attacker_sk, &ctx, &victim_pk, peer);
        assert!(!forged.verify(&ctx, &victim_pk, peer));
    }

    #[test]
    fn init_proof_rejects_wrong_context() {
        let (sk, pk) = test_keypair(1);
        let peer = b"peer-id-bytes-A";

        let proof = sign_pop(&sk, &ContextId::from([7u8; 32]), &pk, peer);
        assert!(!proof.verify(&ContextId::from([8u8; 32]), &pk, peer));
    }

    #[test]
    fn init_proof_rejects_tampered_signature() {
        let ctx = ContextId::from([7u8; 32]);
        let (sk, pk) = test_keypair(1);
        let peer = b"peer-id-bytes-A";

        let mut proof = sign_pop(&sk, &ctx, &pk, peer);
        proof.signature[0] ^= 0xff;
        assert!(!proof.verify(&ctx, &pk, peer));
    }

    #[test]
    fn init_message_roundtrips_with_and_without_pop() {
        let ctx = ContextId::from([7u8; 32]);
        let (sk, pk) = test_keypair(3);
        let peer = b"peer-id-bytes-A";
        let proof = sign_pop(&sk, &ctx, &pk, peer);

        for pop in [Some(proof), None] {
            let msg = StreamMessage::Init {
                context_id: ctx,
                party_id: pk,
                payload: InitPayload::DagHeadsRequest { context_id: ctx },
                next_nonce: [0u8; 12],
                pop,
            };
            let encoded = borsh::to_vec(&msg).expect("serialize");
            let decoded: StreamMessage<'_> = borsh::from_slice(&encoded).expect("deserialize");
            match decoded {
                StreamMessage::Init {
                    pop: decoded_pop, ..
                } => {
                    assert_eq!(decoded_pop.map(|p| p.signature), pop.map(|p| p.signature));
                }
                _ => panic!("wrong variant"),
            }
        }
    }
}
