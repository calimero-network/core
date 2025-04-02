use actix::dev::MessageResponse;
use actix::{Handler, Message};
use calimero_context_primitives::messages::execute::ExecuteRequest;
use calimero_context_primitives::messages::update_application::UpdateApplicationRequest;
use calimero_context_primitives::messages::ContextMessage;

use crate::ContextManager;

pub mod execute;
pub mod update_application;
// pub mod create_context;
// pub mod list_contexts;
// pub mod delete_context;

impl Handler<ContextMessage> for ContextManager {
    type Result = <ContextMessage as Message>::Result;

    fn handle(&mut self, msg: ContextMessage, ctx: &mut Self::Context) -> Self::Result {
        match msg {
            ContextMessage::Execute { request, outcome } => {
                MessageResponse::<Self, ExecuteRequest>::handle(
                    self.handle(request, ctx),
                    ctx,
                    Some(outcome),
                )
            }
            ContextMessage::UpdateApplication { request, outcome } => {
                MessageResponse::<Self, UpdateApplicationRequest>::handle(
                    self.handle(request, ctx),
                    ctx,
                    Some(outcome),
                )
            }
        }
    }
}
