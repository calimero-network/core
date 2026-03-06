use actix::Handler;
use calimero_context_primitives::messages::ContextMessage;
use calimero_utils_actix::adapters::ActorExt;

use crate::ContextManager;

pub mod add_group_members;
pub mod create_context;
pub mod create_group;
pub mod create_group_invitation;
pub mod delete_context;
pub mod delete_group;
pub mod detach_context_from_group;
pub mod execute;
pub mod get_group_for_context;
pub mod get_group_info;
pub mod get_group_upgrade_status;
pub mod join_context;
pub mod join_group;
pub mod join_group_context;
pub mod list_all_groups;
pub mod list_group_contexts;
pub mod list_group_members;
pub mod remove_group_members;
pub mod retry_group_upgrade;
pub mod sync;
pub mod sync_group;
pub mod update_application;
pub mod update_group_settings;
pub mod update_member_role;
pub mod upgrade_group;
mod utils;

impl Handler<ContextMessage> for ContextManager {
    type Result = ();

    fn handle(&mut self, msg: ContextMessage, ctx: &mut Self::Context) -> Self::Result {
        match msg {
            ContextMessage::Execute { request, outcome } => {
                self.forward_handler(ctx, request, outcome)
            }
            ContextMessage::UpdateApplication { request, outcome } => {
                self.forward_handler(ctx, request, outcome)
            }
            ContextMessage::CreateContext { request, outcome } => {
                self.forward_handler(ctx, request, outcome)
            }
            ContextMessage::DeleteContext { request, outcome } => {
                self.forward_handler(ctx, request, outcome)
            }
            ContextMessage::JoinContext { request, outcome } => {
                self.forward_handler(ctx, request, outcome)
            }
            ContextMessage::JoinContextOpenInvitation {
                request: _,
                outcome: _,
            } => {
                //TODO(identity): do we need that here? I don't think so.
                //self.forward_handler(ctx, request, outcome)
            }
            ContextMessage::Sync { request, outcome } => {
                self.forward_handler(ctx, request, outcome)
            }
            ContextMessage::CreateGroup { request, outcome } => {
                self.forward_handler(ctx, request, outcome)
            }
            ContextMessage::DeleteGroup { request, outcome } => {
                self.forward_handler(ctx, request, outcome)
            }
            ContextMessage::AddGroupMembers { request, outcome } => {
                self.forward_handler(ctx, request, outcome)
            }
            ContextMessage::RemoveGroupMembers { request, outcome } => {
                self.forward_handler(ctx, request, outcome)
            }
            ContextMessage::GetGroupInfo { request, outcome } => {
                self.forward_handler(ctx, request, outcome)
            }
            ContextMessage::ListGroupMembers { request, outcome } => {
                self.forward_handler(ctx, request, outcome)
            }
            ContextMessage::ListGroupContexts { request, outcome } => {
                self.forward_handler(ctx, request, outcome)
            }
            ContextMessage::UpgradeGroup { request, outcome } => {
                self.forward_handler(ctx, request, outcome)
            }
            ContextMessage::GetGroupUpgradeStatus { request, outcome } => {
                self.forward_handler(ctx, request, outcome)
            }
            ContextMessage::RetryGroupUpgrade { request, outcome } => {
                self.forward_handler(ctx, request, outcome)
            }
            ContextMessage::CreateGroupInvitation { request, outcome } => {
                self.forward_handler(ctx, request, outcome)
            }
            ContextMessage::JoinGroup { request, outcome } => {
                self.forward_handler(ctx, request, outcome)
            }
            ContextMessage::ListAllGroups { request, outcome } => {
                self.forward_handler(ctx, request, outcome)
            }
            ContextMessage::UpdateGroupSettings { request, outcome } => {
                self.forward_handler(ctx, request, outcome)
            }
            ContextMessage::UpdateMemberRole { request, outcome } => {
                self.forward_handler(ctx, request, outcome)
            }
            ContextMessage::DetachContextFromGroup { request, outcome } => {
                self.forward_handler(ctx, request, outcome)
            }
            ContextMessage::GetGroupForContext { request, outcome } => {
                self.forward_handler(ctx, request, outcome)
            }
            ContextMessage::SyncGroup { request, outcome } => {
                self.forward_handler(ctx, request, outcome)
            }
            ContextMessage::JoinGroupContext { request, outcome } => {
                self.forward_handler(ctx, request, outcome)
            }
        }
    }
}
