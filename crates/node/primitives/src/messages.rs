use actix::Message;
use calimero_governance_types::SignedNamespaceOp;
use calimero_primitives::context::ContextId;
use calimero_primitives::identity::PublicKey;

use tokio::sync::oneshot;

pub mod get_blob_bytes;

use get_blob_bytes::GetBlobBytesRequest;

#[derive(Debug, Message)]
#[rtype("()")]
pub enum NodeMessage {
    GetBlobBytes {
        request: GetBlobBytesRequest,
        outcome: oneshot::Sender<<GetBlobBytesRequest as Message>::Result>,
    },
    /// Last known dialable addresses for the given member identities of a
    /// group, each paired with the peer that answers there.
    ///
    /// Two caches away from the caller and both are node-local, which is why
    /// this crosses the actor boundary: identity-to-peer lives on `NodeState`,
    /// peer-to-address lives in the network manager, and neither is reachable
    /// from `crates/context`.
    ///
    /// Best-effort by construction. Both caches expire entries, so an empty
    /// answer means "nothing fresh to offer" and never "this identity does not
    /// exist" — a caller must treat it as a missing hint, not a missing member.
    PeerAddrsForIdentities {
        group_id: calimero_context_config::types::ContextGroupId,
        identities: Vec<calimero_primitives::identity::PublicKey>,
        outcome: oneshot::Sender<Vec<(libp2p::PeerId, libp2p::Multiaddr)>>,
    },
    /// Forward a `NamespaceOpApplied` signal from the publisher path
    /// (which lives in `crates/context`, with no direct line into the
    /// node-side `ReadinessManager` actor) to the readiness FSM. The
    /// gossipsub-receive path notifies the FSM directly via the actor
    /// address held on `NodeManager`; the publisher path crosses the
    /// crate boundary by routing through `NodeClient -> NodeManager`,
    /// which then forwards to `readiness_addr` here.
    ///
    /// Without this, `state_per_namespace` for a node that *only*
    /// publishes (single-publisher long-lived namespace, or simply the
    /// publisher's own ops) is never observed by the FSM — the doc
    /// claim "FSM observes every monotonic advance regardless of
    /// origin" only held for the receive path until #2237 follow-up.
    ForwardNamespaceOpApplied { namespace_id: [u8; 32] },
    /// Forward a `NamespaceSubscribed` signal to the readiness FSM so it
    /// seeds `subscribed_at` at subscribe time. Routed `NodeClient ->
    /// NodeManager -> readiness_addr`, mirroring `ForwardNamespaceOpApplied`;
    /// the subscribe path (`join_namespace`) holds a `NodeClient`, not the
    /// actor address.
    ForwardNamespaceSubscribed { namespace_id: [u8; 32] },
    /// Queue a signed membership op whose publish reached no peer, so the
    /// readiness FSM rebroadcasts it once a namespace peer subscribes.
    /// Routed `NodeClient -> NodeManager -> readiness_addr` for the same
    /// reason as the two variants above: the join path lives in
    /// `crates/context` and cannot name the node-side actor.
    ForwardPendingRepublish {
        namespace_id: [u8; 32],
        op: Box<SignedNamespaceOp>,
    },
    /// Hand a governance op this node just PUBLISHED to its own namespace
    /// governance DAG, by the same route a peer's op takes.
    ///
    /// The publisher path commits the live mutation, advances the persisted
    /// governance head and writes the op-log directly — it never touches the
    /// in-memory `DagStore` or the unified-op projection that the apply feed
    /// maintains. So without this signal an author's own ops reach its own DAG
    /// only when some peer echoes them back: until then the DAG holds every
    /// child op a peer sends as `Pending` (its parents are the author's own
    /// unfed ops), which costs a backfill round-trip per op, and the projection
    /// lags the live store it is supposed to mirror.
    ///
    /// Routed `NodeClient -> NodeManager -> ContextClient` because
    /// `crates/governance-store` holds a `NodeClient` and cannot name the
    /// context actor. Fire-and-forget by design: the apply must not be awaited
    /// from inside the context handler that published (the actor is blocked on
    /// that handler's future, so awaiting its own message would deadlock), and
    /// a dropped signal only restores the old peer-echo behaviour.
    ApplyLocalNamespaceOp { op: Box<SignedNamespaceOp> },
    /// Edge-trigger the migration-heartbeat emitter to recompute and re-publish
    /// this node's facts for a namespace, out of band of the periodic tick.
    /// Routed `NodeClient -> NodeManager` (the emitter address lives on
    /// `NodeManager`, which the sync crate cannot name). Used by the resync-heal
    /// path: `settle_snapshot_activation` clears the strand marker and rebinds
    /// the activation, but without this the recovered facts wouldn't reach the
    /// admin rollup until the next periodic beat — so a just-resynced member
    /// lingers as stale `failed`. Fire-and-forget; the periodic tick is the
    /// fallback if the signal is dropped.
    RefreshMigrationFacts { namespace_id: [u8; 32] },
    /// Read the best-effort sync-status snapshot the sync run-loop has
    /// recorded for a context. Routed through `NodeClient -> NodeManager`
    /// because the snapshot lives on the node-crate-private `NodeState`,
    /// which the server layer cannot name directly. `outcome` carries
    /// `None` when the run-loop has no record for the context (never
    /// synced — e.g. created locally or just joined).
    GetSyncStatus {
        context_id: ContextId,
        outcome: oneshot::Sender<Option<crate::SyncStatusSnapshot>>,
    },
    /// Set the local node's ephemeral-presence slice for `(context_id, author)`.
    ///
    /// Routes to `handlers::ephemeral::outbound::set_local_ephemeral` on the
    /// actor so seq-counter management and the async gossip-publish stay on the
    /// actor's Arbiter. Returns `Err` when the slice exceeds
    /// `EPHEMERAL_MAX_BYTES` or crypto/key lookup fails; the publish failure
    /// (no mesh peers) is best-effort and not propagated.
    SetLocalEphemeral {
        context_id: ContextId,
        author: PublicKey,
        slice: Vec<u8>,
        outcome: oneshot::Sender<eyre::Result<()>>,
    },
    /// Snapshot the live awareness entries for a context from the in-memory
    /// [`AwarenessStore`]. Returns an empty `Vec` when the context has no
    /// recorded entries.
    ///
    /// [`AwarenessStore`]: crate::handlers::ephemeral::store::AwarenessStore
    GetEphemeralSnapshot {
        context_id: ContextId,
        /// `(author, slice, age_ms)` — age is relative to the responding
        /// node's clock, so a reader on another machine needs no clock sync.
        outcome: oneshot::Sender<Vec<(PublicKey, Vec<u8>, u64)>>,
    },
    /// Snapshot the node-side migration-heartbeat TTL cache (Task 6c.8) for a
    /// namespace into the per-member reports the `get_migration_status` rollup
    /// (Task 6c.9) consumes. Routed through `NodeClient -> NodeManager` because
    /// the cache lives on the node-crate-private `NodeManager`, which the server
    /// layer cannot name directly. Observability only — a member absent from the
    /// returned map resolves to `unknown` in the rollup.
    ///
    /// Returns the transport-neutral [`MigrationStatusReport`] DTO rather than
    /// `calimero-context-client`'s `MemberMigrationReport`: that crate depends on
    /// *this* one, so naming it here would be a dependency cycle. The server
    /// admin handler (which sees both crates) maps the DTO across.
    GetMigrationStatusReports {
        namespace_id: [u8; 32],
        outcome: oneshot::Sender<std::collections::BTreeMap<PublicKey, MigrationStatusReport>>,
    },
}

/// Transport-neutral snapshot of a peer's freshest in-TTL migration heartbeat,
/// projected from the node-side cache (Task 6c.8) and handed to the server admin
/// layer, which maps it into `calimero-context-client`'s `MemberMigrationReport`
/// for the `get_migration_status` rollup (Task 6c.9).
///
/// Defined here (not in `calimero-context-client`) because that crate depends on
/// this one — referencing its `MemberMigrationReport` in [`NodeMessage`] would
/// form a dependency cycle. Field-for-field identical to the rollup's report.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MigrationStatusReport {
    /// Schema/binary version the member has loaded.
    pub schema_version: u32,
    /// Unconverted Convergent ("auto") entries the member still has pending.
    pub residue_auto: u64,
    /// Governance HLC the member has synced/applied through.
    pub synced_up_to_hlc: u64,
    /// Member-signed millis-since-epoch from the heartbeat itself.
    pub reported_at: u64,
    /// Member's self-reported pending-authored count (sum across its namespace
    /// contexts); feeds the rollup's `membersPendingSignature` (6f).
    pub authored_remaining: u64,
    /// Member's self-reported migration-failure discriminant (`0` = none, `1` =
    /// migration-check aborted, `2` = apply errored). Raw `u8` — kept primitive
    /// so this crate need not depend on `calimero-context-client` (cycle); the
    /// server maps it to a typed kind for the rollup.
    pub migration_failed: u8,
}
