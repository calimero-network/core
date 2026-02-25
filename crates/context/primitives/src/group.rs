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
        /// Unix timestamp (seconds) when the last context was upgraded.
        completed_at: u64,
    },
}

/// Snapshot of an in-progress or completed group upgrade, returned by the API.
///
/// Contains the full context of the upgrade operation including source/target
/// revisions, optional migration method, and current progress status.
#[derive(Clone, Debug)]
pub struct GroupUpgradeInfo {
    /// Application revision the group was at before this upgrade.
    pub from_revision: u64,
    /// Application revision the group is being upgraded to.
    pub to_revision: u64,
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
    pub app_key: AppKey,
    pub application_id: ApplicationId,
    pub upgrade_policy: UpgradePolicy,
    pub admin_identity: PublicKey,
}

impl Message for CreateGroupRequest {
    type Result = eyre::Result<CreateGroupResponse>;
}

#[derive(Copy, Clone, Debug)]
pub struct CreateGroupResponse {
    pub group_id: ContextGroupId,
}

#[derive(Copy, Clone, Debug)]
pub struct DeleteGroupRequest {
    pub group_id: ContextGroupId,
    pub requester: PublicKey,
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
    pub requester: PublicKey,
}

impl Message for AddGroupMembersRequest {
    type Result = eyre::Result<()>;
}

#[derive(Debug)]
pub struct RemoveGroupMembersRequest {
    pub group_id: ContextGroupId,
    pub members: Vec<PublicKey>,
    pub requester: PublicKey,
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
    pub requester: PublicKey,
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
    pub requester: PublicKey,
}

impl Message for RetryGroupUpgradeRequest {
    type Result = eyre::Result<UpgradeGroupResponse>;
}

#[derive(Debug)]
pub struct CreateGroupInvitationRequest {
    pub group_id: ContextGroupId,
    pub requester: PublicKey,
    pub invitee_identity: Option<PublicKey>,
    pub expiration: Option<u64>,
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
    pub joiner_identity: PublicKey,
}

impl Message for JoinGroupRequest {
    type Result = eyre::Result<JoinGroupResponse>;
}

#[derive(Copy, Clone, Debug)]
pub struct JoinGroupResponse {
    pub group_id: ContextGroupId,
    pub member_identity: PublicKey,
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

impl From<calimero_store::key::GroupUpgradeValue> for GroupUpgradeInfo {
    fn from(v: calimero_store::key::GroupUpgradeValue) -> Self {
        Self {
            from_revision: v.from_revision,
            to_revision: v.to_revision,
            migration: v.migration,
            initiated_at: v.initiated_at,
            initiated_by: v.initiated_by,
            status: v.status.into(),
        }
    }
}
