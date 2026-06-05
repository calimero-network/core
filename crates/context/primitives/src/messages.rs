use actix::Message;
use calimero_context_config::types::ContextGroupId;
use calimero_primitives::alias::Alias;
use calimero_primitives::application::ApplicationId;
use calimero_primitives::context::ContextId;
use calimero_primitives::hash::Hash;
use calimero_primitives::identity::{PrivateKey, PublicKey};
use serde::{Deserialize, Serialize};
use thiserror::Error as ThisError;
use tokio::sync::oneshot;

use crate::group::{
    AddGroupMembersRequest, AdmitTeeNodeRequest, BroadcastGroupLocalStateRequest,
    CreateGroupInvitationRequest, CreateGroupRequest, DeleteGroupRequest, DeleteNamespaceRequest,
    DetachContextFromGroupRequest, GetCascadeStatusRequest, GetContextMetadataRequest,
    GetGroupForContextRequest, GetGroupInfoRequest, GetGroupMetadataRequest,
    GetGroupUpgradeStatusRequest, GetMemberCapabilitiesRequest, GetMemberMetadataRequest,
    GetMigrationStatusRequest, GetNamespaceIdentityRequest, IssueNamespaceOwnershipProofRequest,
    IssueOwnershipProofRequest, JoinContextRequest, JoinGroupRequest,
    JoinSubgroupInheritanceRequest, LeaveContextRequest, LeaveGroupRequest, LeaveNamespaceRequest,
    ListAllGroupsRequest, ListGroupContextsRequest, ListGroupMembersRequest,
    ListNamespacesForApplicationRequest, ListNamespacesRequest, RemoveGroupMembersRequest,
    RetryGroupUpgradeRequest, SetContextMetadataRequest, SetDefaultCapabilitiesRequest,
    SetGroupMetadataRequest, SetMemberAutoFollowRequest, SetMemberCapabilitiesRequest,
    SetMemberMetadataRequest, SetSubgroupVisibilityRequest, SetTeeAdmissionPolicyRequest,
    StoreContextMetadataRequest, StoreDefaultCapabilitiesRequest, StoreGroupContextRequest,
    StoreGroupMetaRequest, StoreGroupMetadataRequest, StoreMemberCapabilityRequest,
    StoreMemberMetadataRequest, StoreSubgroupVisibilityRequest, SyncGroupRequest,
    UpdateGroupSettingsRequest, UpdateMemberRoleRequest, UpgradeGroupRequest,
};
use crate::{ContextAtomic, ContextAtomicKey};

#[derive(Debug)]
pub struct CreateContextRequest {
    pub protocol: String,
    pub seed: Option<[u8; 32]>,
    pub application_id: ApplicationId,
    /// Which service from the bundle to run. None for single-service apps.
    pub service_name: Option<String>,
    pub identity_secret: Option<PrivateKey>,
    pub init_params: Vec<u8>,
    pub group_id: ContextGroupId,
    pub name: Option<String>,
}

impl Message for CreateContextRequest {
    type Result = eyre::Result<CreateContextResponse>;
}

#[derive(Clone, Debug)]
pub struct CreateContextResponse {
    pub context_id: ContextId,
    pub identity: PublicKey,
    pub group_id: Option<calimero_context_config::types::ContextGroupId>,
    pub group_created: bool,
}

#[derive(Copy, Clone, Debug)]
pub struct DeleteContextRequest {
    pub context_id: ContextId,
    pub requester: Option<PublicKey>,
}

impl Message for DeleteContextRequest {
    type Result = eyre::Result<DeleteContextResponse>;
}

#[derive(Copy, Clone, Debug)]
pub struct DeleteContextResponse {
    pub deleted: bool,
}

#[derive(Debug)]
pub struct ExecuteRequest {
    pub context: ContextId,
    pub executor: PublicKey,
    pub method: String,
    pub payload: Vec<u8>,
    pub aliases: Vec<Alias<PublicKey>>,
    pub atomic: Option<ContextAtomic>,
}

#[derive(Debug)]
pub struct ExecuteResponse {
    pub returns: eyre::Result<Option<Vec<u8>>>,
    pub logs: Vec<String>,
    pub events: Vec<ExecuteEvent>,
    pub root_hash: Hash,
    pub artifact: Vec<u8>,
    pub atomic: Option<ContextAtomicKey>,
}

#[derive(Debug)]
pub struct ExecuteEvent {
    pub kind: String,
    pub data: Vec<u8>,
    pub handler: Option<String>,
}

impl Message for ExecuteRequest {
    type Result = Result<ExecuteResponse, ExecuteError>;
}

/// Acquire the per-context execution lock without running a method.
///
/// Returns the same `Arc<Mutex<ContextId>>` guard that [`ExecuteRequest`]
/// holds for the duration of a WASM run. Host-side storage mutations that
/// happen outside the executor (notably the sync session's `EntityPush` /
/// `EntityDeletePush` apply paths) must hold this guard so they are mutually
/// exclusive with concurrent `__calimero_sync_next` delta merges. Without it
/// the two interleave their ancestor-hash recomputes and record a torn root
/// hash that delta-sync can't repair (split-brain).
///
/// `None` is returned only when the context is unknown — the caller then
/// applies best-effort without a guard, matching the pre-lock behaviour (the
/// apply itself no-ops on a missing context).
#[derive(Copy, Clone, Debug)]
pub struct AcquireContextLockRequest {
    pub context: ContextId,
}

impl Message for AcquireContextLockRequest {
    type Result = Option<ContextAtomicKey>;
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, ThisError)]
#[serde(tag = "type", content = "data")]
#[non_exhaustive]
pub enum ExecuteError {
    #[error("context not found")]
    ContextNotFound,
    #[error("cannot execute request as '{public_key}' on context '{context_id}'")]
    Unauthorized {
        context_id: ContextId,
        public_key: PublicKey,
    },
    #[error("context state not initialized, awaiting state sync")]
    Uninitialized,
    #[error("application not installed: '{application_id}'")]
    ApplicationNotInstalled { application_id: ApplicationId },
    #[error("internal error")]
    InternalError,
    #[error("error resolving identity alias '{alias}'")]
    AliasResolutionFailed { alias: Alias<PublicKey> },
    /// Group-context execute attempted before its `KeyDelivery` op
    /// arrived. The local DAG is healthy and the membership row exists,
    /// but the group key needed to encrypt the resulting state delta has
    /// not yet been received via either the direct join response or the
    /// gossip-fallback wait. Distinct from `Uninitialized` (which means
    /// state-sync is still in progress) and from `InternalError` (which
    /// is a permanent failure). Surfaced to clients as a transient
    /// retry-able condition.
    #[error("group key not yet delivered for context '{context_id}' — retry shortly")]
    GroupKeyPending { context_id: ContextId },
    /// The context belongs to a group whose cascade upgrade is currently
    /// `InProgress`. **Writes** are refused until migration completes (a write
    /// could commit to a mixed-version state); **reads remain available**,
    /// served from the pre-migration root, so only mutating calls hit this
    /// error. Transient and retry-able: re-issue the write once the group's
    /// `GroupUpgradeStatus` reaches `Completed { .. }`.
    ///
    /// PR-2 (cascade engine) chose group-level granularity over
    /// per-context-HLC gating to avoid coupling this gate to the
    /// storage-layout migration that introduces `cascade_hlc` (deferred to
    /// PR-3). The trade-off: a user calling a context in a 100-context
    /// group will be blocked for the duration of the group's whole
    /// propagation, not just until *their* context has been migrated.
    /// This is acceptable because:
    ///   * Eager upgrades complete in bounded time (one execute per
    ///     context, serialised).
    ///   * `LazyOnAccess` groups never enter `InProgress` here — they
    ///     upgrade per-call inside `maybe_lazy_upgrade` and skip the
    ///     group-wide `propagate_upgrade` path entirely.
    ///   * The alternative (no gate) lets a pre-migration call land on a
    ///     context whose neighbours already migrated, creating exactly the
    ///     cross-version drift the cascade is meant to prevent.
    #[error(
        "context upgrade in progress for group {group_id:?}; writes refused until migration completes"
    )]
    UpgradeInProgress { group_id: ContextGroupId },
}

#[derive(Copy, Clone, Debug)]
pub struct SyncRequest {
    pub context_id: ContextId,
    pub application_id: ApplicationId,
}

impl Message for SyncRequest {
    type Result = ();
}

#[derive(Debug, Clone)]
pub struct ApplySignedGroupOpRequest {
    pub op: crate::local_governance::SignedGroupOp,
}

impl Message for ApplySignedGroupOpRequest {
    type Result = eyre::Result<bool>;
}

/// Post-apply state-hash divergence detected by the cross-DAG
/// convergence check (the signed-claims path on `MemberRemoved` /
/// `MemberLeft`). The node-side handler routes this to the
/// reconcile-via-anchor sync trigger so the receiver can pull
/// canonical state from a trusted-anchor peer.
///
/// `hash_differs` carries the **signed expected** hash alongside
/// each divergent `ContextId`. The reconcile path verifies the
/// received state's root hash against this value before adopting —
/// not against the anchor's claim, which could be lying.
#[derive(Debug, Clone)]
pub struct DivergenceReport {
    pub group_id: ContextGroupId,
    pub op_kind: &'static str,
    pub group_hash_diverges: bool,
    pub hash_differs: Vec<(ContextId, [u8; 32])>,
    pub only_in_expected: Vec<ContextId>,
    pub only_in_actual: Vec<ContextId>,
}

/// Outcome of applying a signed namespace governance op.
///
/// Needed by callers that must distinguish "pending, please trigger backfill"
/// from "duplicate, do nothing" — the underlying DAG used to collapse both
/// into `Ok(false)`, causing every duplicate gossip op to open a redundant
/// backfill stream.
#[derive(Debug, Clone)]
pub enum NamespaceApplyOutcome {
    /// Op was applied immediately. Carries the optional post-apply
    /// state-hash divergence report — `Some` when the signed
    /// `MemberRemoved` / `MemberLeft` claims didn't match local
    /// post-apply state, `None` otherwise (and for op variants that
    /// don't carry signed claims).
    Applied {
        divergence: Option<DivergenceReport>,
    },
    /// Op was accepted but is waiting for missing parents; caller should
    /// proactively trigger a namespace backfill.
    Pending,
    /// Op was already present in the governance DAG; no action required.
    Duplicate,
}

impl NamespaceApplyOutcome {
    /// `true` if the op is pending and a backfill should be triggered.
    pub fn is_pending(&self) -> bool {
        matches!(self, Self::Pending)
    }
}

#[derive(Debug, Clone)]
pub struct ApplySignedNamespaceOpRequest {
    pub op: crate::local_governance::SignedNamespaceOp,
}

impl Message for ApplySignedNamespaceOpRequest {
    type Result = eyre::Result<NamespaceApplyOutcome>;
}

/// Query the number of pending (not-yet-applied) ops in a namespace's
/// governance DAG. Used by the cross-peer parent-pull loop (#2198) to
/// decide whether another backfill round is needed.
#[derive(Debug, Clone)]
pub struct NamespacePendingOpCountRequest {
    pub namespace_id: [u8; 32],
}

impl Message for NamespacePendingOpCountRequest {
    type Result = eyre::Result<usize>;
}

/// Parameters for executing a state migration during application update.
///
/// When updating a context's application, an optional migration function can be
/// specified to transform the existing state to the new application's schema.
/// The migration function is embedded in the new application's WASM module and
/// decorated with `#[app::migrate]`.
#[derive(Debug, Clone)]
pub struct MigrationParams {
    /// Name of the migration function to execute (e.g., `migrate_v1_to_v2`).
    pub method: String,
}

#[derive(Debug)]
pub struct UpdateApplicationRequest {
    pub context_id: ContextId,
    pub application_id: ApplicationId,
    pub public_key: PublicKey,
    /// Optional migration parameters for state transformation during update.
    pub migration: Option<MigrationParams>,
}

impl Message for UpdateApplicationRequest {
    type Result = eyre::Result<()>;
}

#[derive(Debug, Message)]
#[rtype("()")]
pub enum ContextMessage {
    Execute {
        request: ExecuteRequest,
        outcome: oneshot::Sender<<ExecuteRequest as Message>::Result>,
    },
    AcquireContextLock {
        request: AcquireContextLockRequest,
        outcome: oneshot::Sender<<AcquireContextLockRequest as Message>::Result>,
    },
    CreateContext {
        request: CreateContextRequest,
        outcome: oneshot::Sender<<CreateContextRequest as Message>::Result>,
    },
    DeleteContext {
        request: DeleteContextRequest,
        outcome: oneshot::Sender<<DeleteContextRequest as Message>::Result>,
    },
    UpdateApplication {
        request: UpdateApplicationRequest,
        outcome: oneshot::Sender<<UpdateApplicationRequest as Message>::Result>,
    },
    Sync {
        request: SyncRequest,
        outcome: oneshot::Sender<<SyncRequest as Message>::Result>,
    },
    CreateGroup {
        request: CreateGroupRequest,
        outcome: oneshot::Sender<<CreateGroupRequest as Message>::Result>,
    },
    DeleteGroup {
        request: DeleteGroupRequest,
        outcome: oneshot::Sender<<DeleteGroupRequest as Message>::Result>,
    },
    DeleteNamespace {
        request: DeleteNamespaceRequest,
        outcome: oneshot::Sender<<DeleteNamespaceRequest as Message>::Result>,
    },
    AddGroupMembers {
        request: AddGroupMembersRequest,
        outcome: oneshot::Sender<<AddGroupMembersRequest as Message>::Result>,
    },
    ApplySignedGroupOp {
        request: ApplySignedGroupOpRequest,
        outcome: oneshot::Sender<<ApplySignedGroupOpRequest as Message>::Result>,
    },
    ApplySignedNamespaceOp {
        request: ApplySignedNamespaceOpRequest,
        outcome: oneshot::Sender<<ApplySignedNamespaceOpRequest as Message>::Result>,
    },
    NamespacePendingOpCount {
        request: NamespacePendingOpCountRequest,
        outcome: oneshot::Sender<<NamespacePendingOpCountRequest as Message>::Result>,
    },
    RemoveGroupMembers {
        request: RemoveGroupMembersRequest,
        outcome: oneshot::Sender<<RemoveGroupMembersRequest as Message>::Result>,
    },
    GetGroupInfo {
        request: GetGroupInfoRequest,
        outcome: oneshot::Sender<<GetGroupInfoRequest as Message>::Result>,
    },
    ListGroupMembers {
        request: ListGroupMembersRequest,
        outcome: oneshot::Sender<<ListGroupMembersRequest as Message>::Result>,
    },
    ListGroupContexts {
        request: ListGroupContextsRequest,
        outcome: oneshot::Sender<<ListGroupContextsRequest as Message>::Result>,
    },
    UpgradeGroup {
        request: UpgradeGroupRequest,
        outcome: oneshot::Sender<<UpgradeGroupRequest as Message>::Result>,
    },
    GetGroupUpgradeStatus {
        request: GetGroupUpgradeStatusRequest,
        outcome: oneshot::Sender<<GetGroupUpgradeStatusRequest as Message>::Result>,
    },
    RetryGroupUpgrade {
        request: RetryGroupUpgradeRequest,
        outcome: oneshot::Sender<<RetryGroupUpgradeRequest as Message>::Result>,
    },
    CreateGroupInvitation {
        request: CreateGroupInvitationRequest,
        outcome: oneshot::Sender<<CreateGroupInvitationRequest as Message>::Result>,
    },
    JoinGroup {
        request: JoinGroupRequest,
        outcome: oneshot::Sender<<JoinGroupRequest as Message>::Result>,
    },
    ListAllGroups {
        request: ListAllGroupsRequest,
        outcome: oneshot::Sender<<ListAllGroupsRequest as Message>::Result>,
    },
    UpdateGroupSettings {
        request: UpdateGroupSettingsRequest,
        outcome: oneshot::Sender<<UpdateGroupSettingsRequest as Message>::Result>,
    },
    UpdateMemberRole {
        request: UpdateMemberRoleRequest,
        outcome: oneshot::Sender<<UpdateMemberRoleRequest as Message>::Result>,
    },
    DetachContextFromGroup {
        request: DetachContextFromGroupRequest,
        outcome: oneshot::Sender<<DetachContextFromGroupRequest as Message>::Result>,
    },
    GetGroupForContext {
        request: GetGroupForContextRequest,
        outcome: oneshot::Sender<<GetGroupForContextRequest as Message>::Result>,
    },
    SyncGroup {
        request: SyncGroupRequest,
        outcome: oneshot::Sender<<SyncGroupRequest as Message>::Result>,
    },
    JoinContext {
        request: JoinContextRequest,
        outcome: oneshot::Sender<<JoinContextRequest as Message>::Result>,
    },
    JoinSubgroupInheritance {
        request: JoinSubgroupInheritanceRequest,
        outcome: oneshot::Sender<<JoinSubgroupInheritanceRequest as Message>::Result>,
    },
    LeaveContext {
        request: LeaveContextRequest,
        outcome: oneshot::Sender<<LeaveContextRequest as Message>::Result>,
    },
    LeaveGroup {
        request: LeaveGroupRequest,
        outcome: oneshot::Sender<<LeaveGroupRequest as Message>::Result>,
    },
    LeaveNamespace {
        request: LeaveNamespaceRequest,
        outcome: oneshot::Sender<<LeaveNamespaceRequest as Message>::Result>,
    },
    SetMemberCapabilities {
        request: SetMemberCapabilitiesRequest,
        outcome: oneshot::Sender<<SetMemberCapabilitiesRequest as Message>::Result>,
    },
    SetMemberAutoFollow {
        request: SetMemberAutoFollowRequest,
        outcome: oneshot::Sender<<SetMemberAutoFollowRequest as Message>::Result>,
    },
    GetMemberCapabilities {
        request: GetMemberCapabilitiesRequest,
        outcome: oneshot::Sender<<GetMemberCapabilitiesRequest as Message>::Result>,
    },
    SetDefaultCapabilities {
        request: SetDefaultCapabilitiesRequest,
        outcome: oneshot::Sender<<SetDefaultCapabilitiesRequest as Message>::Result>,
    },
    SetTeeAdmissionPolicy {
        request: SetTeeAdmissionPolicyRequest,
        outcome: oneshot::Sender<<SetTeeAdmissionPolicyRequest as Message>::Result>,
    },
    AdmitTeeNode {
        request: AdmitTeeNodeRequest,
        outcome: oneshot::Sender<<AdmitTeeNodeRequest as Message>::Result>,
    },
    SetSubgroupVisibility {
        request: SetSubgroupVisibilityRequest,
        outcome: oneshot::Sender<<SetSubgroupVisibilityRequest as Message>::Result>,
    },
    StoreContextMetadata {
        request: StoreContextMetadataRequest,
        outcome: oneshot::Sender<<StoreContextMetadataRequest as Message>::Result>,
    },
    BroadcastGroupLocalState {
        request: BroadcastGroupLocalStateRequest,
        outcome: oneshot::Sender<<BroadcastGroupLocalStateRequest as Message>::Result>,
    },
    StoreMemberCapability {
        request: StoreMemberCapabilityRequest,
        outcome: oneshot::Sender<<StoreMemberCapabilityRequest as Message>::Result>,
    },
    StoreDefaultCapabilities {
        request: StoreDefaultCapabilitiesRequest,
        outcome: oneshot::Sender<<StoreDefaultCapabilitiesRequest as Message>::Result>,
    },
    StoreSubgroupVisibility {
        request: StoreSubgroupVisibilityRequest,
        outcome: oneshot::Sender<<StoreSubgroupVisibilityRequest as Message>::Result>,
    },
    SetMemberMetadata {
        request: SetMemberMetadataRequest,
        outcome: oneshot::Sender<<SetMemberMetadataRequest as Message>::Result>,
    },
    StoreMemberMetadata {
        request: StoreMemberMetadataRequest,
        outcome: oneshot::Sender<<StoreMemberMetadataRequest as Message>::Result>,
    },
    SetGroupMetadata {
        request: SetGroupMetadataRequest,
        outcome: oneshot::Sender<<SetGroupMetadataRequest as Message>::Result>,
    },
    StoreGroupMetadata {
        request: StoreGroupMetadataRequest,
        outcome: oneshot::Sender<<StoreGroupMetadataRequest as Message>::Result>,
    },
    SetContextMetadata {
        request: SetContextMetadataRequest,
        outcome: oneshot::Sender<<SetContextMetadataRequest as Message>::Result>,
    },
    GetGroupMetadata {
        request: GetGroupMetadataRequest,
        outcome: oneshot::Sender<<GetGroupMetadataRequest as Message>::Result>,
    },
    GetMemberMetadata {
        request: GetMemberMetadataRequest,
        outcome: oneshot::Sender<<GetMemberMetadataRequest as Message>::Result>,
    },
    GetContextMetadata {
        request: GetContextMetadataRequest,
        outcome: oneshot::Sender<<GetContextMetadataRequest as Message>::Result>,
    },
    StoreGroupContext {
        request: StoreGroupContextRequest,
        outcome: oneshot::Sender<<StoreGroupContextRequest as Message>::Result>,
    },
    StoreGroupMeta {
        request: StoreGroupMetaRequest,
        outcome: oneshot::Sender<<StoreGroupMetaRequest as Message>::Result>,
    },
    ListNamespaces {
        request: ListNamespacesRequest,
        outcome: oneshot::Sender<<ListNamespacesRequest as Message>::Result>,
    },
    GetNamespaceIdentity {
        request: GetNamespaceIdentityRequest,
        outcome: oneshot::Sender<<GetNamespaceIdentityRequest as Message>::Result>,
    },
    ListNamespacesForApplication {
        request: ListNamespacesForApplicationRequest,
        outcome: oneshot::Sender<<ListNamespacesForApplicationRequest as Message>::Result>,
    },
    IssueOwnershipProof {
        request: IssueOwnershipProofRequest,
        outcome: oneshot::Sender<<IssueOwnershipProofRequest as Message>::Result>,
    },
    IssueNamespaceOwnershipProof {
        request: IssueNamespaceOwnershipProofRequest,
        outcome: oneshot::Sender<<IssueNamespaceOwnershipProofRequest as Message>::Result>,
    },
    GetCascadeStatus {
        request: GetCascadeStatusRequest,
        outcome: oneshot::Sender<<GetCascadeStatusRequest as Message>::Result>,
    },
    GetMigrationStatus {
        request: GetMigrationStatusRequest,
        outcome: oneshot::Sender<<GetMigrationStatusRequest as Message>::Result>,
    },
}
