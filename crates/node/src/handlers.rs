//! Event handlers for network and node messages.
//!
//! **Purpose**: Handles incoming events from network layer and processes node-level requests.
//! **Structure**: Each event type has its own focused file (SRP).

use crate::specialized_node_invite_state::{
    PendingSpecializedNodeInvite, SpecializedNodeInviteAction,
};
use actix::Handler;
use calimero_node_primitives::messages::NodeMessage;
use calimero_utils_actix::adapters::ActorExt;
use tracing::debug;

use crate::NodeManager;

// Each handler in its own focused file (SRP)
mod blob_protocol;
mod get_blob_bytes;
mod network_event;
mod specialized_node_invite;
pub mod state_delta;
mod stream_opened;

impl Handler<NodeMessage> for NodeManager {
    type Result = ();

    fn handle(&mut self, msg: NodeMessage, ctx: &mut Self::Context) -> Self::Result {
        match msg {
            NodeMessage::GetBlobBytes { request, outcome } => {
                self.forward_handler(ctx, request, outcome)
            }
            NodeMessage::RegisterPendingSpecializedNodeInvite { request } => {
                let action = SpecializedNodeInviteAction::HandleContextInvite {
                    context_id: request.context_id,
                    inviter_id: request.inviter_id,
                };
                self.state
                    .pending_specialized_node_invites
                    .insert(request.nonce, PendingSpecializedNodeInvite::new(action));

                debug!(
                    context_id = %request.context_id,
                    inviter_id = %request.inviter_id,
                    nonce = %hex::encode(request.nonce),
                    "Registered pending specialized node invite"
                );
            }
            NodeMessage::RemovePendingSpecializedNodeInvite { request } => {
                self.state
                    .pending_specialized_node_invites
                    .remove(&request.nonce);

                debug!(
                    nonce = %hex::encode(request.nonce),
                    "Removed pending specialized node invite"
                );
            }
        }
    }
}
