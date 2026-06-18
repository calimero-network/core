//! Event handlers for network and node messages.
//!
//! **Purpose**: Handles incoming events from network layer and processes node-level requests.
//! **Structure**: Each event type has its own focused file (SRP).

use crate::migration_status::DEFAULT_HEARTBEAT_TTL;
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
            NodeMessage::GetSyncStatus {
                context_id,
                outcome,
            } => {
                // Synchronous read off the lock-free `sync_status` map; reply
                // directly on the oneshot. A dropped receiver (caller gave up)
                // is fine to ignore — this is a pure observability query.
                let snapshot = self.state.sync_status_snapshot(&context_id);
                let _ = outcome.send(snapshot);
            }
            NodeMessage::GetMigrationStatusReports {
                namespace_id,
                outcome,
            } => {
                // Synchronous snapshot of the in-memory migration-heartbeat TTL
                // cache (Task 6c.8) for the admin `get_migration_status` route
                // (Task 6c.10). Pure observability read — a dropped receiver is
                // fine to ignore. Stale entries are filtered by the cache's
                // per-call TTL; a member with no fresh entry is simply absent,
                // which the rollup resolves to `unknown`.
                let mut reports = self
                    .migration_status_cache
                    .migration_status_reports(namespace_id, DEFAULT_HEARTBEAT_TTL);
                // A node never receives its OWN gossiped heartbeat, so the cache
                // above never holds the local node. Inject its freshly-computed
                // facts (keyed by its namespace identity) so the local node —
                // frequently the admin running this very rollup — is not reported
                // as `unknown`, which would pin `all_migrated` false forever.
                let now_millis = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_millis() as u64)
                    .unwrap_or(0);
                if let Some((self_pk, self_report)) = crate::migration_status::self_migration_report(
                    &self.datastore,
                    namespace_id,
                    now_millis,
                ) {
                    let _ = reports.insert(self_pk, self_report);
                }
                let _ = outcome.send(reports);
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
                // PR-6c Task 6c.8: the same local-progress signal drives the
                // migration-heartbeat emitter. A governance apply may have
                // advanced the group's target schema or drained residue, so
                // recompute and post the node's facts — this both edge-triggers
                // an on-change heartbeat and seeds the namespace into the
                // emitter so its periodic keep-alive tick goes live.
                self.notify_migration_facts(namespace_id);
            }
            NodeMessage::RefreshMigrationFacts { namespace_id } => {
                // Edge-trigger a fact recompute + emit-on-change for this
                // namespace (resync-heal path). Same seam the governance-apply
                // signal uses, without the readiness side-effect — a resync
                // applies no governance op.
                self.notify_migration_facts(namespace_id);
            }
        }
    }
}
