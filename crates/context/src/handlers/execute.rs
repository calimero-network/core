use actix::{Handler, Message, ResponseFuture};
use calimero_context_primitives::messages::execute::ExecuteRequest;

use crate::ContextManager;

impl Handler<ExecuteRequest> for ContextManager {
    type Result = ResponseFuture<<ExecuteRequest as Message>::Result>;

    fn handle(&mut self, msg: ExecuteRequest, ctx: &mut Self::Context) -> Self::Result {
        todo!("localize the functionalities of the runtime here instead")
    }
}
