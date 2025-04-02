use actix::Message;
use tokio::sync::oneshot;

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
    UpdateApplication {
        request: UpdateApplicationRequest,
        outcome: oneshot::Sender<<UpdateApplicationRequest as Message>::Result>,
    },
}
