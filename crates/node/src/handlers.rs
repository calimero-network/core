use actix::Handler;
use calimero_node_primitives::messages::NodeMessage;
use calimero_utils_actix::adapters::ActorExt;

use crate::NodeManager;

mod get_blob_bytes;
mod network_event;

impl Handler<NodeMessage> for NodeManager {
    type Result = ();

    fn handle(&mut self, msg: NodeMessage, ctx: &mut Self::Context) -> Self::Result {
        match msg {
            NodeMessage::GetBlobBytes { request, outcome } => {
                self.forward_handler(ctx, request, outcome)
            }
        }
    }
}
