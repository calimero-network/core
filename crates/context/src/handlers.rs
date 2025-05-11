use actix::Handler;
use calimero_context_primitives::messages::ContextMessage;
use calimero_utils_actix::adapters::ActorExt;

use crate::ContextManager;

pub mod create_context;
pub mod delete_context;
pub mod execute;
pub mod join_context;
pub mod update_application;

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
        }
    }
}
