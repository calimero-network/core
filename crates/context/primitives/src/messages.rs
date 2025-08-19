use actix::Message;
use tokio::sync::oneshot;

pub mod create_context;
pub mod delete_context;
pub mod execute;
pub mod join_context;
pub mod sync;
pub mod update_application;

use create_context::CreateContextRequest;
use delete_context::DeleteContextRequest;
use execute::ExecuteRequest;
use join_context::JoinContextRequest;
use sync::SyncRequest;
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
        outcome: oneshot::Sender<<DeleteContextRequest as Message>::Result>,
    },
    JoinContext {
        request: JoinContextRequest,
        outcome: oneshot::Sender<<JoinContextRequest as Message>::Result>,
    },
    UpdateApplication {
        request: UpdateApplicationRequest,
        outcome: oneshot::Sender<<UpdateApplicationRequest as Message>::Result>,
    },
    Sync {
        request: SyncRequest,
        outcome: oneshot::Sender<<SyncRequest as Message>::Result>,
    },
}
