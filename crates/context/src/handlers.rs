use actix::Handler;
use calimero_context_primitives::messages::ContextMessage;
use calimero_utils_actix::adapters::ActorExt;

use crate::ContextManager;

pub mod add_group_members;
pub mod create_context;
pub mod create_group;
pub mod delete_context;
pub mod delete_group;
pub mod execute;
pub mod get_group_info;
pub mod join_context;
pub mod list_group_members;
pub mod remove_group_members;
pub mod sync;
pub mod update_application;
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
        }
    }
}
