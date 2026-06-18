use actix::Message;
use calimero_primitives::context::ContextId;
use calimero_primitives::identity::PublicKey;
use tokio::sync::oneshot;

pub mod get_blob_bytes;

use get_blob_bytes::GetBlobBytesRequest;

/// Request to register a pending specialized node invite in the node's state.
#[derive(Clone, Debug)]
pub struct RegisterPendingSpecializedNodeInvite {
    /// The nonce from the specialized node invite broadcast
    pub nonce: [u8; 32],
    /// The context to invite specialized nodes to
    pub context_id: ContextId,
    /// The identity performing the invitation
    pub inviter_id: PublicKey,
}

/// Request to remove a pending specialized node invite from the node's state.
/// Used to clean up if broadcast fails after registration.
#[derive(Clone, Debug)]
pub struct RemovePendingSpecializedNodeInvite {
    /// The nonce to remove
    pub nonce: [u8; 32],
}

#[derive(Debug, Message)]
#[rtype("()")]
pub enum NodeMessage {
    GetBlobBytes {
        request: GetBlobBytesRequest,
        outcome: oneshot::Sender<<GetBlobBytesRequest as Message>::Result>,
    },
    RegisterPendingSpecializedNodeInvite {
        request: RegisterPendingSpecializedNodeInvite,
    },
    RemovePendingSpecializedNodeInvite {
        request: RemovePendingSpecializedNodeInvite,
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
    /// Unconverted identity-gated entries the member still has pending.
    pub residue_identity: u64,
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
