use actix::Message;
use create_context::CreateContextRequest;
use tokio::sync::oneshot;

pub mod create_context;
pub mod execute;
pub mod update_application;

use execute::ExecuteRequest;
use update_application::UpdateApplicationRequest;

#[derive(Debug, Message)]
#[rtype("()")]
pub enum ContextMessage {
    Execute {
        request: ExecuteRequest,
        outcome: oneshot::Sender<<ExecuteRequest as Message>::Result>,
    },
    CreateContext {
        request: CreateContextRequest,
        outcome: oneshot::Sender<<CreateContextRequest as Message>::Result>,
    },
    UpdateApplication {
        request: UpdateApplicationRequest,
        outcome: oneshot::Sender<<UpdateApplicationRequest as Message>::Result>,
    },
}
