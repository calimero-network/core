use std::collections::BTreeMap;

use actix::Message;
use calimero_context_config::types::{AppKey, ContextGroupId, SignedGroupOpenInvitation};
use calimero_primitives::application::ApplicationId;
use calimero_primitives::context::{ContextId, GroupMemberRole, UpgradePolicy};
use calimero_primitives::identity::PublicKey;
use calimero_primitives::metadata::MetadataRecord;
use calimero_storage::logical_clock::HybridTimestamp;
use thiserror::Error as ThisError;

use crate::messages::MigrationParams;

pub use calimero_store::key::GroupUpgradeStatus;

/// Snapshot of an in-progress or completed group upgrade, returned by the API.
///
/// Contains the full context of the upgrade operation including source/target
/// versions, optional migration method, and current progress status.
#[derive(Clone, Debug)]
pub struct GroupUpgradeInfo {
    /// Semver version of the application before the upgrade, read from the
    /// current application's `ApplicationMeta.version`.
    pub from_version: String,
    /// Semver version of the target application, read from the target
    /// application's `ApplicationMeta.version`.
    pub to_version: String,
    /// Optional Borsh-serialized migration method name.
    pub migration: Option<Vec<u8>>,
    /// Unix timestamp (seconds) when the upgrade was initiated.
    pub initiated_at: u64,
    /// Identity of the admin who initiated the upgrade.
    pub initiated_by: PublicKey,
    /// Current progress of the upgrade.
    pub status: GroupUpgradeStatus,
}

#[derive(Debug)]
pub struct CreateGroupRequest {
    pub group_id: Option<ContextGroupId>,
    pub app_key: Option<AppKey>,
    pub application_id: ApplicationId,
    pub upgrade_policy: UpgradePolicy,
    pub name: Option<String>,
    pub parent_group_id: Option<ContextGroupId>,
}

impl Message for CreateGroupRequest {
    type Result = eyre::Result<CreateGroupResponse>;
}

#[derive(Copy, Clone, Debug)]
pub struct CreateGroupResponse {
    pub group_id: ContextGroupId,
}

#[derive(Clone, Debug)]
pub struct DeleteGroupRequest {
    pub group_id: ContextGroupId,
    pub requester: Option<PublicKey>,
}

impl Message for DeleteGroupRequest {
    type Result = eyre::Result<DeleteGroupResponse>;
}

#[derive(Copy, Clone, Debug)]
pub struct DeleteGroupResponse {
    pub deleted: bool,
}

/// Request to tear down a namespace and its entire subtree (all descendant
/// groups + contexts + namespace-level state) on the local node.
///
/// Namespace deletion is purely local — it has no DAG-replicated op
/// counterpart. Each node independently tears down its own namespace state.
/// `RootOp::GroupDeleted` explicitly rejects the namespace root (see
/// `execute_group_deleted`), mirroring how namespace *creation* is also
/// local-only.
#[derive(Clone, Debug)]
pub struct DeleteNamespaceRequest {
    pub namespace_id: ContextGroupId,
    pub requester: Option<PublicKey>,
}

impl Message for DeleteNamespaceRequest {
    type Result = eyre::Result<DeleteNamespaceResponse>;
}

#[derive(Copy, Clone, Debug)]
pub struct DeleteNamespaceResponse {
    pub deleted: bool,
}

#[derive(Debug)]
pub struct AddGroupMembersRequest {
    pub group_id: ContextGroupId,
    pub members: Vec<(PublicKey, GroupMemberRole)>,
    pub requester: Option<PublicKey>,
}

impl Message for AddGroupMembersRequest {
    type Result = eyre::Result<()>;
}

#[derive(Debug)]
pub struct RemoveGroupMembersRequest {
    pub group_id: ContextGroupId,
    pub members: Vec<PublicKey>,
    pub requester: Option<PublicKey>,
}

impl Message for RemoveGroupMembersRequest {
    type Result = eyre::Result<()>;
}

#[derive(Copy, Clone, Debug)]
pub struct GetGroupInfoRequest {
    pub group_id: ContextGroupId,
}

impl Message for GetGroupInfoRequest {
    type Result = eyre::Result<GroupInfoResponse>;
}

#[derive(Clone, Debug)]
pub struct GroupInfoResponse {
    pub group_id: ContextGroupId,
    pub app_key: AppKey,
    pub target_application_id: ApplicationId,
    pub upgrade_policy: UpgradePolicy,
    pub member_count: u64,
    pub context_count: u64,
    pub active_upgrade: Option<GroupUpgradeInfo>,
    pub default_capabilities: u32,
    pub subgroup_visibility: String,
    /// Full metadata record for the group (name + opaque `data` map).
    /// The group's metadata record, or `None` if none has been set.
    pub metadata: Option<MetadataRecord>,
    /// SHA-256 hash of the group's authorization-relevant state
    /// (members + roles + admin + owner + target app), produced by
    /// `compute_group_state_hash`. Used by clients to detect governance
    /// convergence across nodes.
    pub state_hash: [u8; 32],
}

#[derive(Debug)]
pub struct ListGroupMembersRequest {
    pub group_id: ContextGroupId,
    pub offset: usize,
    pub limit: usize,
}

impl Message for ListGroupMembersRequest {
    type Result = eyre::Result<ListGroupMembersResponse>;
}

#[derive(Clone, Debug)]
pub struct ListGroupMembersResponse {
    pub members: Vec<GroupMemberEntry>,
    /// The node's own group-level identity (SignerId) so the client knows
    /// which member in the list represents the current node.
    pub self_identity: PublicKey,
}

#[derive(Clone, Debug)]
pub struct GroupMemberEntry {
    pub identity: PublicKey,
    pub role: GroupMemberRole,
    pub name: Option<String>,
}

#[derive(Clone, Debug)]
pub struct GroupContextEntry {
    pub context_id: ContextId,
    pub name: Option<String>,
}

#[derive(Debug)]
pub struct ListGroupContextsRequest {
    pub group_id: ContextGroupId,
    pub offset: usize,
    pub limit: usize,
}

impl Message for ListGroupContextsRequest {
    type Result = eyre::Result<Vec<GroupContextEntry>>;
}

/// Direct local persist — used when applying replicated governance ops.
/// For user-initiated changes, use [`GroupOp::ContextMetadataSet`](crate::local_governance::GroupOp::ContextMetadataSet)
/// via `sign_apply_and_publish` (governance op, replicated via gossip).
#[derive(Debug)]
pub struct StoreContextMetadataRequest {
    pub group_id: ContextGroupId,
    pub context_id: ContextId,
    pub record: MetadataRecord,
}

impl Message for StoreContextMetadataRequest {
    type Result = eyre::Result<()>;
}

#[derive(Debug, Clone)]
pub struct UpgradeGroupRequest {
    pub group_id: ContextGroupId,
    pub target_application_id: ApplicationId,
    pub requester: Option<PublicKey>,
    pub migration: Option<MigrationParams>,
    /// When `true`, the handler emits the single atomic [`GroupOp::CascadeUpgrade`]
    /// op (carrying `target_application_id`, `app_key`, `migration`, and the
    /// fence `cascade_hlc`) that fans out to every descendant subgroup whose
    /// current `app_key` matches the signed group's current `app_key`. When
    /// `false` (the default for `Default::default()` and for any caller
    /// that does not explicitly set it), the handler stays on the
    /// existing single-group path that emits the per-group
    /// [`GroupOp::TargetApplicationSet`] / [`GroupOp::GroupMigrationSet`]
    /// ops.
    ///
    /// Default: `false` so existing callers stay bit-identical.
    pub cascade: bool,
}

impl UpgradeGroupRequest {
    /// Construct a non-cascade (single-group) upgrade request — the
    /// historical default. Use the struct literal with `cascade: true`
    /// for the cascade variant.
    #[must_use]
    pub fn new(
        group_id: ContextGroupId,
        target_application_id: ApplicationId,
        requester: Option<PublicKey>,
        migration: Option<MigrationParams>,
    ) -> Self {
        Self {
            group_id,
            target_application_id,
            requester,
            migration,
            cascade: false,
        }
    }
}

impl Message for UpgradeGroupRequest {
    type Result = eyre::Result<UpgradeGroupResponse>;
}

#[derive(Clone, Debug)]
pub struct UpgradeGroupResponse {
    pub group_id: ContextGroupId,
    pub status: GroupUpgradeStatus,
}

#[derive(Debug)]
pub struct GetGroupUpgradeStatusRequest {
    pub group_id: ContextGroupId,
}

impl Message for GetGroupUpgradeStatusRequest {
    type Result = eyre::Result<Option<GroupUpgradeInfo>>;
}

#[derive(Debug)]
pub struct RetryGroupUpgradeRequest {
    pub group_id: ContextGroupId,
    pub requester: Option<PublicKey>,
}

impl Message for RetryGroupUpgradeRequest {
    type Result = eyre::Result<UpgradeGroupResponse>;
}

#[derive(Debug)]
pub struct CreateGroupInvitationRequest {
    pub group_id: ContextGroupId,
    pub requester: Option<PublicKey>,
    /// Duration in seconds for the invitation validity.
    /// Defaults to 1 year when not provided.
    pub expiration_timestamp: Option<u64>,
}

impl Message for CreateGroupInvitationRequest {
    type Result = eyre::Result<CreateGroupInvitationResponse>;
}

#[derive(Debug)]
pub struct CreateGroupInvitationResponse {
    pub invitation: SignedGroupOpenInvitation,
    pub group_name: Option<String>,
}

#[derive(Debug)]
pub struct JoinGroupRequest {
    pub invitation: SignedGroupOpenInvitation,
    pub group_name: Option<String>,
}

impl Message for JoinGroupRequest {
    type Result = eyre::Result<JoinGroupResponse>;
}

#[derive(Clone, Debug)]
pub struct JoinGroupResponse {
    pub group_id: ContextGroupId,
    pub member_identity: PublicKey,
    /// Serialized `SignedGroupOp` (borsh) containing the `JoinWithInvitationClaim`.
    /// The orchestrator must relay this to the inviting node's claim-invitation
    /// endpoint so the member is registered on the remote side.
    pub governance_op_bytes: Vec<u8>,
}

#[derive(Debug)]
pub struct ListAllGroupsRequest {
    pub offset: usize,
    pub limit: usize,
}

impl Message for ListAllGroupsRequest {
    type Result = eyre::Result<Vec<GroupSummary>>;
}

#[derive(Clone, Debug)]
pub struct GroupSummary {
    pub group_id: ContextGroupId,
    pub app_key: AppKey,
    pub target_application_id: ApplicationId,
    pub upgrade_policy: UpgradePolicy,
    pub created_at: u64,
    pub name: Option<String>,
}

#[derive(Debug)]
pub struct UpdateGroupSettingsRequest {
    pub group_id: ContextGroupId,
    pub requester: Option<PublicKey>,
    pub upgrade_policy: UpgradePolicy,
}

impl Message for UpdateGroupSettingsRequest {
    type Result = eyre::Result<()>;
}

#[derive(Debug)]
pub struct UpdateMemberRoleRequest {
    pub group_id: ContextGroupId,
    pub identity: PublicKey,
    pub new_role: GroupMemberRole,
    pub requester: Option<PublicKey>,
}

impl Message for UpdateMemberRoleRequest {
    type Result = eyre::Result<()>;
}

#[derive(Debug)]
pub struct DetachContextFromGroupRequest {
    pub group_id: ContextGroupId,
    pub context_id: ContextId,
    pub requester: Option<PublicKey>,
}

impl Message for DetachContextFromGroupRequest {
    type Result = eyre::Result<()>;
}

#[derive(Copy, Clone, Debug)]
pub struct GetGroupForContextRequest {
    pub context_id: ContextId,
}

impl Message for GetGroupForContextRequest {
    type Result = eyre::Result<Option<ContextGroupId>>;
}

#[derive(Debug)]
pub struct SyncGroupRequest {
    pub group_id: ContextGroupId,
    pub requester: Option<PublicKey>,
}

impl Message for SyncGroupRequest {
    type Result = eyre::Result<SyncGroupResponse>;
}

#[derive(Clone, Debug)]
pub struct SyncGroupResponse {
    pub group_id: ContextGroupId,
    pub app_key: [u8; 32],
    pub target_application_id: ApplicationId,
    pub member_count: u64,
    pub context_count: u64,
}

#[derive(Debug)]
pub struct JoinContextRequest {
    pub context_id: ContextId,
}

impl Message for JoinContextRequest {
    type Result = eyre::Result<JoinContextResponse>;
}

#[derive(Clone, Debug)]
pub struct JoinContextResponse {
    pub context_id: ContextId,
    pub member_public_key: PublicKey,
}

/// Direct "join this Open subgroup as an inherited member" request — no
/// admin-signed invitation, no child context id required.
///
/// The handler verifies the caller has an Inherited (or Direct) membership
/// path to `group_id` via `check_group_membership_path`; if Inherited, it
/// publishes a `RootOp::MemberJoinedOpen` so a peer holding the group key
/// responds with `KeyDelivery`, then blocks on
/// `config.key_delivery_fallback_wait` until the key arrives.
#[derive(Copy, Clone, Debug)]
pub struct JoinSubgroupInheritanceRequest {
    pub group_id: ContextGroupId,
}

impl Message for JoinSubgroupInheritanceRequest {
    type Result = eyre::Result<JoinSubgroupInheritanceResponse>;
}

#[derive(Clone, Debug)]
pub struct JoinSubgroupInheritanceResponse {
    pub group_id: ContextGroupId,
    pub member_public_key: PublicKey,
    /// `true` if the caller had to publish a `MemberJoinedOpen` op to
    /// materialise inherited membership; `false` if they were already a
    /// direct member and the call was a no-op.
    pub was_inherited: bool,
}

/// Typed failure cases for the join-via-inheritance flow. Carried inside
/// the actor's `eyre::Report` so the HTTP layer can `downcast_ref`
/// without depending on error-message wording.
#[derive(Clone, Copy, Debug, ThisError)]
pub enum JoinSubgroupInheritanceError {
    #[error("group not found")]
    GroupNotFound,
    #[error("no namespace identity for this group; join the parent namespace first")]
    NoNamespaceIdentity,
    #[error("identity not eligible for inheritance-based join")]
    NotEligible,
}

/// Request to leave a context locally on this node. Purely a node-local opt-out:
/// no governance op is published, no key rotation is performed, peers never
/// observe the leave. The handler:
///
/// 1. Deletes the local `ContextIdentity` row, which stops sync (the sync layer
///    iterates `ContextIdentity` rows to determine what to replicate).
/// 2. Writes a `ContextLeftMarker` tombstone in the `Column::ContextLocal`
///    column, which the auto-follow handler checks before re-joining.
///
/// Cleared by an explicit `JoinContextRequest` from the user, which removes the
/// marker as a side effect of joining.
#[derive(Debug)]
pub struct LeaveContextRequest {
    pub context_id: ContextId,
}

impl Message for LeaveContextRequest {
    type Result = eyre::Result<LeaveContextResponse>;
}

#[derive(Clone, Debug)]
pub struct LeaveContextResponse {
    pub context_id: ContextId,
    pub member_public_key: PublicKey,
}

/// Self-leave from a single group. Distributed governance op:
/// publishes `GroupOp::MemberLeft { member: signer }` which deletes
/// the leaver's direct membership row across all peers and cascades
/// the per-context identity rows under the group.
///
/// Preconditions enforced at apply: signer must be a direct member
/// (not just inherited), must not be Owner, and last-admin protection
/// applies (admin can't leave if they're the only admin).
///
/// **Forward-secrecy note:** this op deliberately does not trigger
/// the key-rotation pipeline that admin-initiated `MemberRemoved`
/// does. For full cryptographic leave today, pair with admin
/// follow-up; the proper two-phase rotation is a deferred follow-up.
#[derive(Debug)]
pub struct LeaveGroupRequest {
    pub group_id: ContextGroupId,
}

impl Message for LeaveGroupRequest {
    type Result = eyre::Result<LeaveGroupResponse>;
}

#[derive(Clone, Debug)]
pub struct LeaveGroupResponse {
    pub group_id: ContextGroupId,
    pub member_public_key: PublicKey,
}

/// Self-leave from a namespace (root group). Operationally a `MemberLeft`
/// at the namespace root, but the apply path detects "this group has no
/// parent" and cascades through every descendant group where the leaver
/// has a direct row — running owner + last-admin checks across all of
/// them BEFORE any mutation. See § 6 of the design doc.
///
/// Same forward-secrecy caveat as `leave_group`: row-removal cascade
/// only, no per-scope key rotation in this PR.
#[derive(Debug)]
pub struct LeaveNamespaceRequest {
    pub namespace_id: ContextGroupId,
}

impl Message for LeaveNamespaceRequest {
    type Result = eyre::Result<LeaveNamespaceResponse>;
}

#[derive(Clone, Debug)]
pub struct LeaveNamespaceResponse {
    pub namespace_id: ContextGroupId,
    pub member_public_key: PublicKey,
}

// ---- Metadata getters ----

#[derive(Copy, Clone, Debug)]
pub struct GetGroupMetadataRequest {
    pub group_id: ContextGroupId,
}

impl Message for GetGroupMetadataRequest {
    type Result = eyre::Result<Option<MetadataRecord>>;
}

#[derive(Copy, Clone, Debug)]
pub struct GetMemberMetadataRequest {
    pub group_id: ContextGroupId,
    pub member: PublicKey,
}

impl Message for GetMemberMetadataRequest {
    type Result = eyre::Result<Option<MetadataRecord>>;
}

#[derive(Copy, Clone, Debug)]
pub struct GetContextMetadataRequest {
    pub group_id: ContextGroupId,
    pub context_id: ContextId,
}

impl Message for GetContextMetadataRequest {
    type Result = eyre::Result<Option<MetadataRecord>>;
}

/// Request issued by an admin client to obtain a signed ownership-claim
/// payload for a calimero group.
///
/// The handler resolves the node's identity for the group's namespace,
/// verifies it is a direct admin, looks up the group's signing key, and
/// returns a base64-encoded canonical JSON payload + an ed25519 signature
/// over `OWNERSHIP_PROOF_DOMAIN || signed_payload_bytes`. The verifier on
/// the other side (mdma) re-parses the opaque payload bytes; field order
/// in the payload is fixed by the struct definition order in the handler.
///
/// See the issue-ownership-proof handler for the locked wire format.
#[derive(Debug)]
pub struct IssueOwnershipProofRequest {
    pub group_id: ContextGroupId,
    pub context_id: ContextId,
    pub audience: String,
    pub subject: String,
    /// Hex string, validated by the API layer to be 32..=128 chars.
    pub nonce: String,
    /// Caller-requested expiry in unix ms. Clamped server-side to
    /// `min(expires_at_ms, issued_at_ms + 5*60*1000)`.
    pub expires_at_ms: u64,
}

impl Message for IssueOwnershipProofRequest {
    type Result = eyre::Result<IssueOwnershipProofResponse>;
}

/// Namespace-scoped sibling of [`IssueOwnershipProofRequest`].
///
/// Identical wire/signing contract MINUS the `context_id`: the signed payload
/// carries `context_id == ""` and the handler skips the context lookup +
/// containment walk. The authorization root is the namespace-root `group_id`
/// (direct-admin gate), and the response type is reused verbatim.
///
/// See the issue-ownership-proof handler for the locked wire format.
#[derive(Debug)]
pub struct IssueNamespaceOwnershipProofRequest {
    pub group_id: ContextGroupId,
    pub audience: String,
    pub subject: String,
    /// Hex string, validated by the API layer to be 32..=128 chars.
    pub nonce: String,
    /// Caller-requested expiry in unix ms. Clamped server-side to
    /// `min(expires_at_ms, issued_at_ms + 5*60*1000)`.
    pub expires_at_ms: u64,
}

impl Message for IssueNamespaceOwnershipProofRequest {
    type Result = eyre::Result<IssueOwnershipProofResponse>;
}

#[derive(Clone, Debug)]
pub struct IssueOwnershipProofResponse {
    /// Ed25519 public key of the signer (the node's group signing identity).
    pub signer_public_key: PublicKey,
    /// Opaque UTF-8 JSON bytes of the canonical claim payload. The verifier
    /// re-parses these bytes; the API layer base64-encodes them on the wire.
    pub signed_payload: Vec<u8>,
    /// Raw 64-byte ed25519 signature over `OWNERSHIP_PROOF_DOMAIN || signed_payload`.
    pub signature: [u8; 64],
}

// ---- Group Permission Types ----

#[derive(Debug)]
pub struct SetMemberCapabilitiesRequest {
    pub group_id: ContextGroupId,
    pub member: PublicKey,
    pub capabilities: u32,
    pub requester: Option<PublicKey>,
}

impl Message for SetMemberCapabilitiesRequest {
    type Result = eyre::Result<()>;
}

/// Set per-member auto-follow flags on a group.
///
/// Authorized by group admin (for any `target`) or by the `target` itself
/// (self-setting). The apply path enforces the admin-or-self rule —
/// see `GroupOp::MemberSetAutoFollow` and the auto-follow architecture doc.
#[derive(Debug)]
pub struct SetMemberAutoFollowRequest {
    pub group_id: ContextGroupId,
    pub target: PublicKey,
    pub auto_follow_contexts: bool,
    pub auto_follow_subgroups: bool,
    pub requester: Option<PublicKey>,
}

impl Message for SetMemberAutoFollowRequest {
    type Result = eyre::Result<()>;
}

#[derive(Debug)]
pub struct GetMemberCapabilitiesRequest {
    pub group_id: ContextGroupId,
    pub member: PublicKey,
}

impl Message for GetMemberCapabilitiesRequest {
    type Result = eyre::Result<GetMemberCapabilitiesResponse>;
}

#[derive(Clone, Debug)]
pub struct GetMemberCapabilitiesResponse {
    pub capabilities: u32,
}

#[derive(Debug)]
pub struct SetDefaultCapabilitiesRequest {
    pub group_id: ContextGroupId,
    pub default_capabilities: u32,
    pub requester: Option<PublicKey>,
}

impl Message for SetDefaultCapabilitiesRequest {
    type Result = eyre::Result<()>;
}

#[derive(Debug)]
pub struct SetTeeAdmissionPolicyRequest {
    pub group_id: ContextGroupId,
    pub allowed_mrtd: Vec<String>,
    pub allowed_rtmr0: Vec<String>,
    pub allowed_rtmr1: Vec<String>,
    pub allowed_rtmr2: Vec<String>,
    pub allowed_rtmr3: Vec<String>,
    pub allowed_tcb_statuses: Vec<String>,
    pub accept_mock: bool,
    pub requester: Option<PublicKey>,
}

impl Message for SetTeeAdmissionPolicyRequest {
    type Result = eyre::Result<()>;
}

#[derive(Debug)]
pub struct AdmitTeeNodeRequest {
    pub group_id: ContextGroupId,
    pub member: PublicKey,
    pub quote_hash: [u8; 32],
    pub mrtd: String,
    pub rtmr0: String,
    pub rtmr1: String,
    pub rtmr2: String,
    pub rtmr3: String,
    pub tcb_status: String,
    pub is_mock: bool,
}

impl Message for AdmitTeeNodeRequest {
    type Result = eyre::Result<()>;
}

#[derive(Debug)]
pub struct SetSubgroupVisibilityRequest {
    pub group_id: ContextGroupId,
    pub subgroup_visibility: calimero_context_config::VisibilityMode,
    pub requester: Option<PublicKey>,
}

impl Message for SetSubgroupVisibilityRequest {
    type Result = eyre::Result<()>;
}

/// Direct local persist — used when applying replicated governance ops.
/// For user-initiated changes, use [`SetSubgroupVisibilityRequest`] instead, which
/// goes through `sign_apply_and_publish` (governance op, replicated via gossip).
#[derive(Debug)]
pub struct StoreSubgroupVisibilityRequest {
    pub group_id: ContextGroupId,
    pub mode: u8,
}

impl Message for StoreSubgroupVisibilityRequest {
    type Result = eyre::Result<()>;
}

#[derive(Debug)]
pub struct BroadcastGroupLocalStateRequest {
    pub group_id: ContextGroupId,
}

impl Message for BroadcastGroupLocalStateRequest {
    type Result = eyre::Result<()>;
}

#[derive(Debug)]
pub struct SetMemberMetadataRequest {
    pub group_id: ContextGroupId,
    pub member: PublicKey,
    pub name: Option<String>,
    pub data: BTreeMap<String, String>,
    pub requester: Option<PublicKey>,
}

impl Message for SetMemberMetadataRequest {
    type Result = eyre::Result<()>;
}

/// Direct local persist — used when applying replicated governance ops.
/// For user-initiated changes, use [`SetMemberMetadataRequest`] instead, which
/// goes through `sign_apply_and_publish` (governance op, replicated via gossip).
#[derive(Debug)]
pub struct StoreMemberMetadataRequest {
    pub group_id: ContextGroupId,
    pub member: PublicKey,
    pub record: MetadataRecord,
}

impl Message for StoreMemberMetadataRequest {
    type Result = eyre::Result<()>;
}

#[derive(Debug)]
pub struct SetGroupMetadataRequest {
    pub group_id: ContextGroupId,
    pub name: Option<String>,
    pub data: BTreeMap<String, String>,
    pub requester: Option<PublicKey>,
}

impl Message for SetGroupMetadataRequest {
    type Result = eyre::Result<()>;
}

#[derive(Debug)]
pub struct SetContextMetadataRequest {
    pub group_id: ContextGroupId,
    pub context_id: ContextId,
    pub name: Option<String>,
    pub data: BTreeMap<String, String>,
    pub requester: Option<PublicKey>,
}

impl Message for SetContextMetadataRequest {
    type Result = eyre::Result<()>;
}

/// Direct local persist — used when applying replicated governance ops.
/// For user-initiated changes, use [`SetGroupMetadataRequest`] instead, which
/// goes through `sign_apply_and_publish` (governance op, replicated via gossip).
#[derive(Debug)]
pub struct StoreGroupMetadataRequest {
    pub group_id: ContextGroupId,
    pub record: MetadataRecord,
}

impl Message for StoreGroupMetadataRequest {
    type Result = eyre::Result<()>;
}

/// Direct local persist — used when applying replicated governance ops.
/// For user-initiated changes, use [`GroupOp::ContextRegistered`](crate::local_governance::GroupOp::ContextRegistered)
/// via `sign_apply_and_publish` (governance op, replicated via gossip).
#[derive(Debug)]
pub struct StoreGroupContextRequest {
    pub group_id: ContextGroupId,
    pub context_id: ContextId,
}

impl Message for StoreGroupContextRequest {
    type Result = eyre::Result<()>;
}

/// Direct local persist — used when applying replicated governance ops.
/// For user-initiated changes, use the appropriate `Set*Request` types or other
/// signed [`GroupOp`](crate::local_governance::GroupOp) flows that go through
/// `sign_apply_and_publish` (governance op, replicated via gossip).
#[derive(Debug)]
pub struct StoreGroupMetaRequest {
    pub group_id: ContextGroupId,
    /// Borsh-serialized `GroupMetaValue`.
    pub meta_payload: Vec<u8>,
}

impl Message for StoreGroupMetaRequest {
    type Result = eyre::Result<()>;
}

#[derive(Debug)]
pub struct StoreMemberCapabilityRequest {
    pub group_id: ContextGroupId,
    pub member: PublicKey,
    pub capabilities: u32,
}

impl Message for StoreMemberCapabilityRequest {
    type Result = eyre::Result<()>;
}

/// Direct local persist — used when applying replicated governance ops.
/// For user-initiated changes, use [`SetDefaultCapabilitiesRequest`] instead, which
/// goes through `sign_apply_and_publish` (governance op, replicated via gossip).
#[derive(Debug)]
pub struct StoreDefaultCapabilitiesRequest {
    pub group_id: ContextGroupId,
    pub capabilities: u32,
}

impl Message for StoreDefaultCapabilitiesRequest {
    type Result = eyre::Result<()>;
}

// ---------------------------------------------------------------------------
// Namespace queries
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct NamespaceSummary {
    pub namespace_id: ContextGroupId,
    pub app_key: AppKey,
    pub target_application_id: ApplicationId,
    pub upgrade_policy: UpgradePolicy,
    pub created_at: u64,
    pub name: Option<String>,
    pub member_count: usize,
    pub context_count: usize,
    pub subgroup_count: usize,
}

#[derive(Debug)]
pub struct ListNamespacesRequest {
    pub offset: usize,
    pub limit: usize,
}

impl Message for ListNamespacesRequest {
    type Result = eyre::Result<Vec<NamespaceSummary>>;
}

#[derive(Debug)]
pub struct GetNamespaceIdentityRequest {
    pub group_id: ContextGroupId,
}

impl Message for GetNamespaceIdentityRequest {
    type Result = eyre::Result<Option<(ContextGroupId, PublicKey)>>;
}

#[derive(Debug)]
pub struct ListNamespacesForApplicationRequest {
    pub application_id: ApplicationId,
    pub offset: usize,
    pub limit: usize,
}

impl Message for ListNamespacesForApplicationRequest {
    type Result = eyre::Result<Vec<NamespaceSummary>>;
}

// ---------------------------------------------------------------------------
// Cascade status query
// ---------------------------------------------------------------------------

/// Per-context cascade migration status across a namespace subtree.
///
/// Returned as one element per group (including the namespace root itself) by
/// the `get_cascade_status` RPC.
#[derive(Clone, Debug)]
pub struct GetCascadeStatusRequest {
    pub namespace_id: ContextGroupId,
}

impl Message for GetCascadeStatusRequest {
    type Result = eyre::Result<Vec<CascadeStatusEntry>>;
}

// ---------------------------------------------------------------------------
// Admin abort-migration (PR-6d task 6d.4)
// ---------------------------------------------------------------------------

/// Logically abort an in-flight namespace migration (Task 6d.4).
///
/// Flips the group's pending migration target back to the pre-migration
/// application id and drops the pending `migration` marker so not-yet-applied
/// lazy contexts stop migrating on their next access (`maybe_lazy_upgrade` no
/// longer triggers). This is a **logical** abort: there is no byte snapshot to
/// restore, and an already-committed v2 context is *not* recalled (that would
/// be the replicated-delta recall this train explicitly does not do — spec §7
/// invariant 5). Idempotent: aborting a group with no pending migration is a
/// no-op success.
#[derive(Clone, Debug)]
pub struct AbortMigrationRequest {
    pub namespace_id: ContextGroupId,
}

impl Message for AbortMigrationRequest {
    type Result = eyre::Result<AbortMigrationResponse>;
}

/// Outcome of an [`AbortMigrationRequest`].
#[derive(Clone, Debug)]
pub struct AbortMigrationResponse {
    pub namespace_id: ContextGroupId,
    /// `true` when a pending migration was found and flipped back; `false` when
    /// there was nothing to abort (the idempotent no-op case).
    pub aborted: bool,
}

/// Request the migration-status rollup for a namespace subtree (Task 6c.9).
///
/// Resolves the pinned-cohort expected members (the inherited-membership
/// closure pinned at the expand-entry HLC) and rolls up the per-member
/// heartbeat reports into a [`MigrationStatus`]. Observability only.
///
/// `member_reports` carries the freshest in-TTL heartbeat each member emitted,
/// projected from the node-side `MigrationStatusCache` (Task 6c.8) by the
/// caller that holds it (the admin route, Task 6c.10, via
/// `NodeClient::migration_status_reports`). `ContextManager` itself cannot
/// reach the node-side cache, so the cache snapshot is threaded in here rather
/// than read inside the handler. A member absent from this map resolves to
/// `unknown`; an empty map yields an all-`unknown` rollup (never a false
/// green).
#[derive(Clone, Debug)]
pub struct GetMigrationStatusRequest {
    pub namespace_id: ContextGroupId,
    /// Freshest per-member heartbeat reports (peer → report), snapshotted from
    /// the node-side TTL cache by the caller.
    pub member_reports: BTreeMap<PublicKey, MemberMigrationReport>,
}

impl Message for GetMigrationStatusRequest {
    type Result = eyre::Result<MigrationStatus>;
}

/// One entry in the cascade-status response: upgrade info for a single group
/// in the namespace subtree, plus the sticky HLC fence that the atomic
/// `CascadeUpgrade` op stamped on it.
#[derive(Clone, Debug)]
pub struct CascadeStatusEntry {
    pub group_id: ContextGroupId,
    /// Full upgrade snapshot for the group (mirrors what `get_group_upgrade_status`
    /// returns per-group), serialised here for every group in the subtree.
    pub upgrade: GroupUpgradeInfo,
    /// Sticky HLC fence written by `CascadeUpgrade`, if the group was part of
    /// an atomic cascade. `None` for non-cascade upgrades and groups that have
    /// no upgrade record.
    pub cascade_hlc: Option<HybridTimestamp>,
}

impl From<calimero_store::key::GroupUpgradeValue> for GroupUpgradeInfo {
    fn from(v: calimero_store::key::GroupUpgradeValue) -> Self {
        Self {
            from_version: v.from_version,
            to_version: v.to_version,
            migration: v.migration,
            initiated_at: v.initiated_at,
            initiated_by: v.initiated_by,
            status: v.status.into(),
        }
    }
}

// ---------------------------------------------------------------------------
// Migration status rollup (PR-6c task 6c.9)
// ---------------------------------------------------------------------------

/// Per-member reported migration facts, as observed from the freshest
/// in-TTL heartbeat that member emitted (Task 6c.8's `MigrationStatusCache`).
///
/// This is the cache-agnostic projection the rollup consumes: the node maps a
/// fresh `CacheEntry` into one of these, and a member with no fresh heartbeat
/// maps to `None` (→ [`MemberMigrationState::Unknown`]). Keeping the rollup
/// over this plain struct rather than the node-only cache type lets the
/// rollup live in the shared types crate, so it carries no `calimero-node`
/// dependency and stays unit-testable.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MemberMigrationReport {
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
}

/// The migration state the rollup assigns a pinned-cohort member.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MemberMigrationState {
    /// Reported `schema_version >= target` with both residue counts at 0.
    Migrated,
    /// Reported a fresh heartbeat but is still behind the target version or
    /// carrying residue.
    InProgress,
    /// No fresh heartbeat within the TTL. Treated as *not migrated* — it
    /// keeps `all_migrated == false` rather than being silently dropped, so a
    /// member that stopped reporting can never produce a false green.
    Unknown,
}

impl MemberMigrationState {
    /// Stable JSON discriminant used by the admin API (Task 6c.10).
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Migrated => "migrated",
            Self::InProgress => "in_progress",
            Self::Unknown => "unknown",
        }
    }
}

/// One row in the migration-status response: a single pinned-cohort member
/// and the state derived from its freshest in-TTL heartbeat (if any).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MemberMigrationStatus {
    pub peer: PublicKey,
    /// The member's reported facts, or `None` when it has no fresh heartbeat
    /// (in which case `state == Unknown`).
    pub report: Option<MemberMigrationReport>,
    pub state: MemberMigrationState,
}

/// Rollup counters across the pinned cohort.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MigrationStatusRollup {
    pub migrated: usize,
    pub in_progress: usize,
    pub unknown: usize,
    pub total: usize,
    /// `true` iff **every** pinned-cohort member reported
    /// `schema_version >= target && residue_auto == 0 && residue_identity == 0`.
    /// Any `unknown` (or any member still in progress) keeps this `false`.
    pub all_migrated: bool,
}

/// Full migration-status answer for a namespace: the pinned cohort, the
/// per-member rows, and the rollup. Observability only — never gates a
/// write/apply. Shape matches spec §8 exactly.
#[derive(Clone, Debug)]
pub struct MigrationStatus {
    pub target_version: u32,
    /// Size of the pinned cohort (the inherited-membership closure, minus any
    /// member excluded by the expand-entry HLC pin). Members excluded by the
    /// pin are NOT counted here and do not gate `all_migrated`.
    pub expected_members: usize,
    /// The governance HLC the cohort was pinned at (migration expand-entry).
    pub cohort_pinned_at_hlc: Option<HybridTimestamp>,
    pub rollup: MigrationStatusRollup,
    pub members: Vec<MemberMigrationStatus>,
}

/// Compute the migration-status rollup over a namespace's inherited-membership
/// closure, pinned at the migration expand-entry HLC.
///
/// `closure` is the current inherited-membership closure for the namespace
/// subtree (the `list ∪ enumerate_inherited` set, computed by the caller).
/// `cohort_pinned_at_seq` is the migration's expand-entry governance position
/// (`NamespaceGovHead.sequence` captured when the cascade was stamped): a member
/// whose freshest heartbeat proves it had not yet synced through the pin
/// (`synced_up_to_hlc < cohort_pinned_at_seq`) was not part of the converged
/// state the migration pinned and is **excluded** from the cohort (this is the
/// `synced_up_to_hlc` overlay that realizes pinning — a post-expand joiner that
/// reports a sync position behind the pin is dropped rather than counted). The
/// pin and the report are BOTH `NamespaceGovHead.sequence` values, so the
/// comparison is like-for-like; `cohort_pinned_at_hlc` is the replicated NTP64
/// HLC fence surfaced for display only and is NEVER used as the overlay pin (it
/// lives in a different, physical-time number space). A member with no report is
/// kept in the cohort as `unknown` (its membership is not contradicted), so a
/// silent member never produces a false green. `report_for` resolves each member
/// to its freshest in-TTL heartbeat report, or `None` if the member has no fresh
/// heartbeat (→ `unknown`).
///
/// Pure and side-effect free — this is observability only and never gates
/// correctness or progress. `all_migrated` is `true` IFF every pinned-cohort
/// member reported `schema_version >= target_version` with both residue counts
/// at 0; a single `unknown` or in-progress member keeps it `false`.
pub fn compute_migration_status_rollup(
    target_version: u32,
    cohort_pinned_at_hlc: Option<HybridTimestamp>,
    cohort_pinned_at_seq: Option<u64>,
    closure: &[PublicKey],
    mut report_for: impl FnMut(&PublicKey) -> Option<MemberMigrationReport>,
) -> MigrationStatus {
    let mut members = Vec::with_capacity(closure.len());
    let mut migrated = 0usize;
    let mut in_progress = 0usize;
    let mut unknown = 0usize;

    for peer in closure {
        let report = report_for(peer);

        // Pin overlay: a member whose freshest heartbeat proves it synced only
        // up to a governance position strictly before the expand-entry position
        // was not part of the converged pinned state — exclude it from the
        // cohort entirely (not even `unknown`). A member with no report is NOT
        // excluded (we cannot prove it joined late), so it stays in-cohort as
        // `unknown`.
        //
        // The pin is a `NamespaceGovHead.sequence` (the migration's expand-entry
        // governance position), compared like-for-like against the heartbeat's
        // `synced_up_to_hlc` — which is ALSO a `NamespaceGovHead.sequence`
        // (`MigrationEmitter::refresh_hlc` sets it from `head.sequence`), NOT an
        // HLC physical time. `cohort_pinned_at_hlc` is the replicated HLC fence
        // surfaced for display only; it lives in NTP64 physical-time space and
        // must never be compared against the sequence-space sync position.
        if let (Some(pin), Some(r)) = (cohort_pinned_at_seq, report) {
            if r.synced_up_to_hlc < pin {
                continue;
            }
        }

        let state = match report {
            None => {
                unknown += 1;
                MemberMigrationState::Unknown
            }
            Some(r) => {
                if r.schema_version >= target_version
                    && r.residue_auto == 0
                    && r.residue_identity == 0
                {
                    migrated += 1;
                    MemberMigrationState::Migrated
                } else {
                    in_progress += 1;
                    MemberMigrationState::InProgress
                }
            }
        };
        members.push(MemberMigrationStatus {
            peer: *peer,
            report,
            state,
        });
    }

    let total = members.len();
    // all_migrated only when the cohort is non-empty and every member is
    // `Migrated` (no unknown, no in-progress). An empty cohort is NOT green.
    let all_migrated = total > 0 && migrated == total;

    MigrationStatus {
        target_version,
        expected_members: total,
        cohort_pinned_at_hlc,
        rollup: MigrationStatusRollup {
            migrated,
            in_progress,
            unknown,
            total,
            all_migrated,
        },
        members,
    }
}

#[cfg(test)]
mod migration_status_tests {
    use std::collections::BTreeMap;

    use calimero_primitives::identity::PublicKey;

    use calimero_storage::logical_clock::{HybridTimestamp, Timestamp, ID, NTP64};

    use super::{compute_migration_status_rollup, MemberMigrationReport, MemberMigrationState};

    fn pk(b: u8) -> PublicKey {
        PublicKey::from([b; 32])
    }

    /// A heartbeat report whose freshest sync position is well past any pin
    /// used in these tests, so the pin overlay keeps the member in-cohort.
    fn report(
        schema_version: u32,
        residue_auto: u64,
        residue_identity: u64,
    ) -> MemberMigrationReport {
        report_synced(schema_version, residue_auto, residue_identity, u64::MAX)
    }

    /// Build a report whose `synced_up_to_hlc` is a `NamespaceGovHead.sequence`
    /// — the exact value the emitter publishes (`MigrationEmitter::refresh_hlc`
    /// sets `synced_up_to_hlc = head.sequence`). Models the producing space so
    /// the pin overlay is exercised against real units, not a fabricated HLC.
    fn report_synced(
        schema_version: u32,
        residue_auto: u64,
        residue_identity: u64,
        synced_up_to_seq: u64,
    ) -> MemberMigrationReport {
        MemberMigrationReport {
            schema_version,
            residue_auto,
            residue_identity,
            synced_up_to_hlc: synced_up_to_seq,
            reported_at: 0,
        }
    }

    /// Build the DISPLAY-only HLC fence (`cascade_hlc`) the way the initiator
    /// stamps it: an NTP64 physical-time `HybridTimestamp` produced by the
    /// storage HLC. Its raw `as_u64()` is on the order of ~7.6e18 — a number
    /// space the sequence-based `synced_up_to_hlc` must NEVER be compared
    /// against. Used here to prove the rollup does NOT use it as the overlay pin.
    fn cascade_hlc_at(t: u64) -> Option<HybridTimestamp> {
        let id = ID::from(std::num::NonZeroU128::new(1).unwrap());
        Some(HybridTimestamp::new(Timestamp::new(NTP64(t), id)))
    }

    /// Cohort {A,B,C}; A,B report v2+residue0; C never reports.
    /// C must be `unknown` and keep `all_migrated == false`.
    #[test]
    fn unknown_member_blocks_all_migrated() {
        let (a, b, c) = (pk(0xA), pk(0xB), pk(0xC));
        let mut reports = BTreeMap::new();
        let _ = reports.insert(a, report(2, 0, 0));
        let _ = reports.insert(b, report(2, 0, 0));
        // C absent — no fresh heartbeat.

        let st = compute_migration_status_rollup(2, None, None, &[a, b, c], |peer| {
            reports.get(peer).copied()
        });

        assert_eq!(st.rollup.unknown, 1);
        assert_eq!(st.rollup.migrated, 2);
        assert!(
            !st.rollup.all_migrated,
            "an unknown member must keep all_migrated false"
        );
        let c_row = st.members.iter().find(|m| m.peer == c).expect("C present");
        assert_eq!(c_row.state, MemberMigrationState::Unknown);
        assert!(c_row.report.is_none());
    }

    /// A member whose freshest heartbeat proves it had not synced through the
    /// expand-entry governance position (`synced_up_to_hlc < cohort_pinned_at_seq`)
    /// is a post-expand joiner from the migration's perspective: the pin overlay
    /// EXCLUDES it from the cohort entirely (it is not even surfaced in
    /// `members`) and it does not flip `all_migrated`.
    ///
    /// Both the pin and the reports here are `NamespaceGovHead.sequence` values
    /// — the SAME space the producing code emits (`head.sequence` on both the
    /// `cascade_seq` stamp and the heartbeat's `synced_up_to_hlc`). A separate
    /// HLC `cascade_hlc` (the display fence) is supplied too, to prove it is NOT
    /// the value the overlay compares against.
    #[test]
    fn cohort_pinned_ignores_post_expand_joiner() {
        let (a, b, c, d) = (pk(0xA), pk(0xB), pk(0xC), pk(0xD));
        // Realistic governance-head sequence values (tens of ops), as the
        // governance store produces — NOT NTP64 physical time.
        let pin_seq = 10u64;
        let mut reports = BTreeMap::new();
        // A,B,C synced at/after the pinned expand-entry sequence -> in-cohort.
        let _ = reports.insert(a, report_synced(2, 0, 0, pin_seq + 2));
        let _ = reports.insert(b, report_synced(2, 0, 0, pin_seq + 5));
        let _ = reports.insert(c, report_synced(2, 0, 0, pin_seq));
        // D's freshest sync position (head sequence 9) is BELOW the pinned
        // expand-entry sequence (10) -> a joiner whose governance head trails
        // the migration cut, excluded by the overlay even though it carries
        // residue and is behind on version.
        let _ = reports.insert(d, report_synced(1, 5, 5, pin_seq - 1));

        // Display HLC fence in its own (physical-time) space — must not affect
        // the overlay, which keys on `pin_seq`.
        let st = compute_migration_status_rollup(
            2,
            cascade_hlc_at(7_600_000_000_000_000_000),
            Some(pin_seq),
            &[a, b, c, d],
            |peer| reports.get(peer).copied(),
        );

        assert_eq!(
            st.expected_members, 3,
            "D's head sequence trails the pinned expand-entry sequence; excluded"
        );
        assert_eq!(st.rollup.total, 3);
        assert!(
            st.rollup.all_migrated,
            "post-pin joiner must not flip all_migrated"
        );
        assert!(
            st.members.iter().all(|m| m.peer != d),
            "excluded member is not surfaced in members[]"
        );
    }

    /// Regression guard for the unit/number-space mismatch: the pin is the
    /// expand-entry `NamespaceGovHead.sequence` (a small counter) while the
    /// DISPLAY `cascade_hlc` is an NTP64 physical-time HLC (~7.6e18). If the
    /// overlay (incorrectly) compared `synced_up_to_hlc` against
    /// `cascade_hlc.get_time().as_u64()`, EVERY reporting member's tiny head
    /// sequence (e.g. 12) would be `< 7.6e18` and get excluded, collapsing the
    /// cohort to only unknown members so `all_migrated` could never be true.
    /// With the like-for-like sequence comparison, members synced past the
    /// expand-entry sequence stay in-cohort and `all_migrated` holds.
    #[test]
    fn overlay_uses_sequence_pin_not_display_hlc() {
        let (a, b) = (pk(0xA), pk(0xB));
        let mut reports = BTreeMap::new();
        // Real gov-head sequences, both at/after the expand-entry sequence (10).
        let _ = reports.insert(a, report_synced(2, 0, 0, 12));
        let _ = reports.insert(b, report_synced(2, 0, 0, 11));

        // A large display HLC (NTP64 physical time) alongside a small sequence
        // pin — the exact mismatch the old `.get_time().as_u64()` comparison hit.
        let st = compute_migration_status_rollup(
            2,
            cascade_hlc_at(7_600_000_000_000_000_000),
            Some(10),
            &[a, b],
            |peer| reports.get(peer).copied(),
        );

        assert_eq!(
            st.expected_members, 2,
            "members synced past the expand-entry SEQUENCE must stay in-cohort; \
             the display HLC must not be used as the pin"
        );
        assert_eq!(st.rollup.migrated, 2);
        assert!(
            st.rollup.all_migrated,
            "with a like-for-like sequence pin every reporting member is migrated"
        );
    }

    /// A member with NO report is never excluded by the pin (we cannot prove it
    /// joined late) — it stays in-cohort as `unknown` and keeps `all_migrated`
    /// false, so the overlay can never silently drop a member into a false
    /// green.
    #[test]
    fn pin_does_not_exclude_unreported_member() {
        let (a, b) = (pk(0xA), pk(0xB));
        let mut reports = BTreeMap::new();
        // A reports a head sequence past the pin.
        let _ = reports.insert(a, report_synced(2, 0, 0, 20));
        // B absent.

        let st = compute_migration_status_rollup(2, None, Some(10), &[a, b], |peer| {
            reports.get(peer).copied()
        });

        assert_eq!(st.expected_members, 2, "unreported member is not excluded");
        assert_eq!(st.rollup.unknown, 1);
        assert!(!st.rollup.all_migrated);
    }

    /// `all_migrated` is true only when every pinned member reports
    /// `v >= target` with both residue counts at 0.
    #[test]
    fn all_migrated_true_only_when_every_pinned_member_v2_residue0() {
        let (a, b, c) = (pk(0xA), pk(0xB), pk(0xC));

        // All migrated -> green.
        let all_ok =
            compute_migration_status_rollup(2, None, None, &[a, b, c], |_| Some(report(2, 0, 0)));
        assert!(all_ok.rollup.all_migrated);
        assert_eq!(all_ok.rollup.migrated, 3);

        // One member still carries identity residue -> in_progress, not green.
        let mut reports = BTreeMap::new();
        let _ = reports.insert(a, report(2, 0, 0));
        let _ = reports.insert(b, report(2, 0, 0));
        let _ = reports.insert(c, report(2, 0, 1));
        let with_residue = compute_migration_status_rollup(2, None, None, &[a, b, c], |peer| {
            reports.get(peer).copied()
        });
        assert!(!with_residue.rollup.all_migrated);
        assert_eq!(with_residue.rollup.in_progress, 1);
        let c_row = with_residue
            .members
            .iter()
            .find(|m| m.peer == c)
            .expect("C present");
        assert_eq!(c_row.state, MemberMigrationState::InProgress);

        // One member behind target version -> in_progress, not green.
        let mut behind = BTreeMap::new();
        let _ = behind.insert(a, report(2, 0, 0));
        let _ = behind.insert(b, report(1, 0, 0));
        let _ = behind.insert(c, report(2, 0, 0));
        let behind_status = compute_migration_status_rollup(2, None, None, &[a, b, c], |peer| {
            behind.get(peer).copied()
        });
        assert!(!behind_status.rollup.all_migrated);
        assert_eq!(behind_status.rollup.in_progress, 1);

        // An empty cohort is never green.
        let empty = compute_migration_status_rollup(2, None, None, &[], |_| None);
        assert!(!empty.rollup.all_migrated);
        assert_eq!(empty.rollup.total, 0);
    }
}
