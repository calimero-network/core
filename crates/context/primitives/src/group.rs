use actix::Message;
use calimero_context_config::types::{AppKey, ContextGroupId};
use calimero_primitives::application::ApplicationId;
use calimero_primitives::context::{
    ContextId, GroupInvitationPayload, GroupMemberRole, UpgradePolicy,
};
use calimero_primitives::identity::PublicKey;
use calimero_store::key::{GroupUpgradeStatus, GroupUpgradeValue};

use crate::messages::MigrationParams;

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
    pub active_upgrade: Option<GroupUpgradeValue>,
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
    type Result = eyre::Result<Option<GroupUpgradeValue>>;
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
