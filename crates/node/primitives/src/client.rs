use actix::Message;
use calimero_primitives::{context::ContextId, identity::PublicKey};
use calimero_runtime::logic::Outcome;
use calimero_utils_actix::LazyRecipient;
use tokio::sync::oneshot;

use crate::messages::{CallError, ExecutionRequest};

#[derive(Clone, Debug)]
pub struct NodeClient {
    node_manager: LazyRecipient<NodeMessage>,
}

#[derive(Message)]
#[rtype("()")]
pub enum NodeMessage {
    Execute {
        request: ExecutionRequest,
        outcome: oneshot::Sender<Result<Outcome, CallError>>,
    },
}

impl NodeClient {
    pub fn new(node_manager: LazyRecipient<NodeMessage>) -> Self {
        Self { node_manager }
    }

    pub async fn execute(
        &self,
        context_id: ContextId,
        method: String,
        payload: Vec<u8>,
        executor_public_key: PublicKey,
    ) -> Result<Outcome, CallError> {
        let (tx, rx) = oneshot::channel();

        self.node_manager
            .send(NodeMessage::Execute {
                request: ExecutionRequest {
                    context_id,
                    method,
                    payload,
                    executor_public_key,
                },
                outcome: tx,
            })
            .await
            .expect("Mailbox to not be dropped");

        rx.await.expect("Mailbox to not be dropped")
    }
}
