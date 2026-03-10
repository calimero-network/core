use actix::Message;
use calimero_context_config::types::{AppKey, ContextGroupId};
use calimero_primitives::application::ApplicationId;
use calimero_primitives::context::{
    ContextId, GroupInvitationPayload, GroupMemberRole, UpgradePolicy,
};
use calimero_primitives::identity::PublicKey;

use crate::messages::MigrationParams;

/// State machine for a group-wide upgrade operation.
///
/// Transitions: `InProgress` → `Completed` once all contexts are upgraded.
/// If failures persist after retries, the status remains `InProgress` with a
/// non-zero `failed` count, allowing manual retry via the retry endpoint.
#[derive(Clone, Debug)]
pub enum GroupUpgradeStatus {
    /// Upgrade is actively being propagated across the group's contexts.
    InProgress {
        /// Total number of contexts in the group at upgrade start.
        total: u32,
        /// Number of contexts successfully upgraded so far.
        completed: u32,
        /// Number of contexts that failed in the current round.
        failed: u32,
    },
    /// All contexts in the group have been successfully upgraded.
    Completed {
        /// Unix timestamp (seconds) when the last context was upgraded, or
        /// `None` for `LazyOnAccess` upgrades where contexts upgrade individually
        /// on demand with no single completion event.
        completed_at: Option<u64>,
    },
}

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
}

#[derive(Debug)]
pub struct ListGroupMembersRequest {
    pub group_id: ContextGroupId,
    pub offset: usize,
    pub limit: usize,
}

impl Message for ListGroupMembersRequest {
    type Result = eyre::Result<Vec<GroupMemberEntry>>;
}

#[derive(Clone, Debug)]
pub struct GroupMemberEntry {
    pub identity: PublicKey,
    pub role: GroupMemberRole,
}

#[derive(Debug)]
pub struct ListGroupContextsRequest {
    pub group_id: ContextGroupId,
    pub offset: usize,
    pub limit: usize,
}

impl Message for ListGroupContextsRequest {
    type Result = eyre::Result<Vec<ContextId>>;
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
    pub invitee_identity: Option<PublicKey>,
    pub expiration: Option<u64>,
    /// On-chain block height after which the invitation commitment expires.
    /// Defaults to 999_999_999 when not provided (backward-compatible).
    pub expiration_block_height: Option<u64>,
}

impl Message for CreateGroupInvitationRequest {
    type Result = eyre::Result<CreateGroupInvitationResponse>;
}

#[derive(Debug)]
pub struct CreateGroupInvitationResponse {
    pub payload: GroupInvitationPayload,
}

#[derive(Debug)]
pub struct JoinGroupRequest {
    pub invitation_payload: GroupInvitationPayload,
}

impl Message for JoinGroupRequest {
    type Result = eyre::Result<JoinGroupResponse>;
}

#[derive(Clone, Debug)]
pub struct JoinGroupResponse {
    pub group_id: ContextGroupId,
    pub member_identity: PublicKey,
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
    /// Optional contract coordinates. If not provided, uses the node's
    /// configured "near" protocol params.
    pub protocol: Option<String>,
    pub network_id: Option<String>,
    pub contract_id: Option<String>,
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
pub struct JoinGroupContextRequest {
    pub group_id: ContextGroupId,
    pub context_id: ContextId,
}

impl Message for JoinGroupContextRequest {
    type Result = eyre::Result<JoinGroupContextResponse>;
}

#[derive(Clone, Debug)]
pub struct JoinGroupContextResponse {
    pub context_id: ContextId,
    pub member_public_key: PublicKey,
}

impl From<calimero_store::key::GroupUpgradeStatus> for GroupUpgradeStatus {
    fn from(s: calimero_store::key::GroupUpgradeStatus) -> Self {
        match s {
            calimero_store::key::GroupUpgradeStatus::InProgress {
                total,
                completed,
                failed,
            } => Self::InProgress {
                total,
                completed,
                failed,
            },
            calimero_store::key::GroupUpgradeStatus::Completed { completed_at } => {
                Self::Completed { completed_at }
            }
        }
    }
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
pub struct SetContextVisibilityRequest {
    pub group_id: ContextGroupId,
    pub context_id: ContextId,
    pub mode: calimero_context_config::VisibilityMode,
    pub requester: Option<PublicKey>,
}

impl Message for SetContextVisibilityRequest {
    type Result = eyre::Result<()>;
}

#[derive(Debug)]
pub struct GetContextVisibilityRequest {
    pub group_id: ContextGroupId,
    pub context_id: ContextId,
}

impl Message for GetContextVisibilityRequest {
    type Result = eyre::Result<GetContextVisibilityResponse>;
}

#[derive(Clone, Debug)]
pub struct GetContextVisibilityResponse {
    pub mode: calimero_context_config::VisibilityMode,
    pub creator: PublicKey,
}

#[derive(Debug)]
pub struct ManageContextAllowlistRequest {
    pub group_id: ContextGroupId,
    pub context_id: ContextId,
    pub add: Vec<PublicKey>,
    pub remove: Vec<PublicKey>,
    pub requester: Option<PublicKey>,
}

impl Message for ManageContextAllowlistRequest {
    type Result = eyre::Result<()>;
}

#[derive(Debug)]
pub struct GetContextAllowlistRequest {
    pub group_id: ContextGroupId,
    pub context_id: ContextId,
}

impl Message for GetContextAllowlistRequest {
    type Result = eyre::Result<Vec<PublicKey>>;
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
pub struct SetDefaultVisibilityRequest {
    pub group_id: ContextGroupId,
    pub default_visibility: calimero_context_config::VisibilityMode,
    pub requester: Option<PublicKey>,
}

impl Message for SetDefaultVisibilityRequest {
    type Result = eyre::Result<()>;
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
