use actix::Message;
use calimero_context_config::types::{AppKey, ContextGroupId, SignedGroupOpenInvitation};
use calimero_primitives::application::ApplicationId;
use calimero_primitives::context::{ContextId, GroupMemberRole, UpgradePolicy};
use calimero_primitives::identity::PublicKey;

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
    pub alias: Option<String>,
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
    pub default_visibility: String,
    pub alias: Option<String>,
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
    pub alias: Option<String>,
}

#[derive(Clone, Debug)]
pub struct GroupContextEntry {
    pub context_id: ContextId,
    pub alias: Option<String>,
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
/// For user-initiated changes, use [`GroupOp::ContextAliasSet`](crate::local_governance::GroupOp::ContextAliasSet)
/// via `sign_apply_and_publish` (governance op, replicated via gossip).
#[derive(Debug)]
pub struct StoreContextAliasRequest {
    pub group_id: ContextGroupId,
    pub context_id: ContextId,
    pub alias: String,
}

impl Message for StoreContextAliasRequest {
    type Result = eyre::Result<()>;
}

#[derive(Debug, Clone)]
pub struct UpgradeGroupRequest {
    pub group_id: ContextGroupId,
    pub target_application_id: ApplicationId,
    pub requester: Option<PublicKey>,
    pub migration: Option<MigrationParams>,
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
    pub group_alias: Option<String>,
}

#[derive(Debug)]
pub struct JoinGroupRequest {
    pub invitation: SignedGroupOpenInvitation,
    pub group_alias: Option<String>,
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
    pub alias: Option<String>,
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
pub struct SetDefaultVisibilityRequest {
    pub group_id: ContextGroupId,
    pub default_visibility: calimero_context_config::VisibilityMode,
    pub requester: Option<PublicKey>,
}

impl Message for SetDefaultVisibilityRequest {
    type Result = eyre::Result<()>;
}

/// Direct local persist — used when applying replicated governance ops.
/// For user-initiated changes, use [`SetDefaultVisibilityRequest`] instead, which
/// goes through `sign_apply_and_publish` (governance op, replicated via gossip).
#[derive(Debug)]
pub struct StoreDefaultVisibilityRequest {
    pub group_id: ContextGroupId,
    pub mode: u8,
}

impl Message for StoreDefaultVisibilityRequest {
    type Result = eyre::Result<()>;
}

#[derive(Debug)]
pub struct BroadcastGroupAliasesRequest {
    pub group_id: ContextGroupId,
}

impl Message for BroadcastGroupAliasesRequest {
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
pub struct SetMemberAliasRequest {
    pub group_id: ContextGroupId,
    pub member: PublicKey,
    pub alias: String,
    pub requester: Option<PublicKey>,
}

impl Message for SetMemberAliasRequest {
    type Result = eyre::Result<()>;
}

/// Direct local persist — used when applying replicated governance ops.
/// For user-initiated changes, use [`SetMemberAliasRequest`] instead, which
/// goes through `sign_apply_and_publish` (governance op, replicated via gossip).
#[derive(Debug)]
pub struct StoreMemberAliasRequest {
    pub group_id: ContextGroupId,
    pub member: PublicKey,
    pub alias: String,
}

impl Message for StoreMemberAliasRequest {
    type Result = eyre::Result<()>;
}

#[derive(Debug)]
pub struct SetGroupAliasRequest {
    pub group_id: ContextGroupId,
    pub alias: String,
    pub requester: Option<PublicKey>,
}

impl Message for SetGroupAliasRequest {
    type Result = eyre::Result<()>;
}

/// Direct local persist — used when applying replicated governance ops.
/// For user-initiated changes, use [`SetGroupAliasRequest`] instead, which
/// goes through `sign_apply_and_publish` (governance op, replicated via gossip).
#[derive(Debug)]
pub struct StoreGroupAliasRequest {
    pub group_id: ContextGroupId,
    pub alias: String,
}

impl Message for StoreGroupAliasRequest {
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
    pub alias: Option<String>,
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
