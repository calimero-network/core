//! Event handlers for network and node messages.
//!
//! **Purpose**: Handles incoming events from network layer and processes node-level requests.
//! **Structure**: Each event type has its own focused file (SRP).

use actix::Handler;
use calimero_node_primitives::messages::NodeMessage;
use calimero_utils_actix::adapters::ActorExt;

use crate::NodeManager;

// Each handler in its own focused file (SRP)
mod blob_protocol;
mod get_blob_bytes;
mod network_event;
mod state_delta;
mod stream_opened;

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
