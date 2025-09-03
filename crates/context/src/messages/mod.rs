//! Message types for context operations

pub mod create_context;
pub mod delete_context;
pub mod execute;
pub mod join_context;
pub mod sync;
pub mod update_application;

use actix::Message;
use tokio::sync::oneshot;

pub use create_context::{CreateContextRequest, CreateContextResponse};
pub use delete_context::{DeleteContextRequest, DeleteContextResponse};
pub use execute::{ExecuteRequest, ExecuteResponse, ExecuteError};
pub use join_context::{JoinContextRequest, JoinContextResponse};
pub use sync::SyncRequest;
pub use update_application::UpdateApplicationRequest;

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
