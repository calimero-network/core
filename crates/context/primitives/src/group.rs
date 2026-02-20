use actix::Message;
use calimero_context_config::types::{AppKey, ContextGroupId};
use calimero_primitives::application::ApplicationId;
use calimero_primitives::context::UpgradePolicy;
use calimero_primitives::identity::PublicKey;

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
