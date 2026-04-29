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
pub(crate) mod state_delta;
mod stream_opened;
pub(crate) mod tee_attestation_admission;

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
                    .pending_specialized_node_invites_handle()
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
                    .pending_specialized_node_invites_handle()
                    .remove(&request.nonce);

                debug!(
                    nonce = %hex::encode(request.nonce),
                    "Removed pending specialized node invite"
                );
            }
            NodeMessage::ForwardNamespaceOpApplied { namespace_id } => {
                // Forward the publisher-side signal to the readiness FSM.
                // Mirrors `addr.do_send(NamespaceOpApplied { namespace_id })`
                // in `handlers/network_event/namespace.rs` for the receive
                // path, so both paths land on the same `Handler<NamespaceOpApplied>`.
                //
                // `readiness_addr` is `None` only during the brief window
                // between `NodeManager::new` and `setup_readiness_manager`
                // running in `Actor::started`. A signal that arrives in
                // that window is dropped — the FSM will reconcile when
                // the next op or peer beacon arrives. This matches the
                // documented "drop the message" behavior on the receive
                // path (`crates/node/src/manager.rs:53`).
                if let Some(addr) = &self.readiness_addr {
                    addr.do_send(crate::readiness::NamespaceOpApplied { namespace_id });
                } else {
                    debug!(
                        namespace_id = %hex::encode(namespace_id),
                        "ForwardNamespaceOpApplied received before ReadinessManager mounted; \
                         dropping (FSM will reconcile via next op or peer beacon)"
                    );
                }
            }
        }
    }
}
