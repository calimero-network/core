use actix::{Handler, Message};
use calimero_context_primitives::messages::ContextMessage;
use calimero_utils_actix::forward_handler;

use crate::ContextManager;

pub mod execute;
pub mod update_application;
// pub mod create_context;
// pub mod list_contexts;
// pub mod delete_context;

impl Handler<ContextMessage> for ContextManager {
    type Result = ();

    fn handle(&mut self, msg: ContextMessage, ctx: &mut Self::Context) -> Self::Result {
        match msg {
            ContextMessage::Execute { request, outcome } => {
                forward_handler(self, ctx, request, outcome)
            }
            ContextMessage::UpdateApplication { request, outcome } => {
                forward_handler(self, ctx, request, outcome)
            }
        }
    }
}
