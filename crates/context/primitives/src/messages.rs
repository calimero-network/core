use actix::Message;
use create_context::CreateContextRequest;
use delete_context::{DeleteContextRequest, DeleteContextResponse};
use join_context::{JoinContextRequest, JoinContextResponse};
use tokio::sync::oneshot;

pub mod create_context;
pub mod delete_context;
pub mod execute;
pub mod join_context;
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
    DeleteContext {
        request: DeleteContextRequest,
        outcome: oneshot::Sender<<DeleteContextResponse as Message>::Result>,
    },
    JoinContext {
        request: JoinContextRequest,
        outcome: oneshot::Sender<<JoinContextResponse as Message>::Result>,
    },
    UpdateApplication {
        request: UpdateApplicationRequest,
        outcome: oneshot::Sender<<UpdateApplicationRequest as Message>::Result>,
    },
}
