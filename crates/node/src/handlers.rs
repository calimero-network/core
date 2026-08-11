//! Event handlers for network and node messages.
//!
//! **Purpose**: Handles incoming events from network layer and processes node-level requests.
//! **Structure**: Each event type has its own focused file (SRP).

use actix::Handler;
use calimero_node_primitives::messages::NodeMessage;
use calimero_utils_actix::adapters::ActorExt;
use tracing::debug;

use crate::NodeManager;

// Each handler in its own focused file (SRP)
mod blob_protocol;
mod get_blob_bytes;
mod network_event;
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
                // Synchronous snapshot for the admin `get_migration_status`
                // route (Task 6c.10), assembled by the same function the
                // receive-path reaction uses so both answer from one map. Pure
                // observability read — a dropped receiver is fine to ignore.
                let _ = outcome.send(crate::migration_status::namespace_member_reports(
                    &self.migration_status_cache,
                    &self.datastore,
                    namespace_id,
                ));
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
            NodeMessage::ForwardNamespaceSubscribed { namespace_id } => {
                // Seed the readiness FSM's `subscribed_at` at subscribe time.
                // Same mount-window caveat as `ForwardNamespaceOpApplied`: a
                // signal dropped before the actor is wired is harmless — the
                // first applied op seeds the entry (just later).
                if let Some(addr) = &self.readiness_addr {
                    addr.do_send(crate::readiness::NamespaceSubscribed { namespace_id });
                } else {
                    debug!(
                        namespace_id = %hex::encode(namespace_id),
                        "ForwardNamespaceSubscribed received before ReadinessManager mounted; \
                         dropping (first applied op will seed the entry)"
                    );
                }
            }
            NodeMessage::ForwardPendingRepublish { namespace_id, op } => {
                // Same mount-window caveat as the two forwards above. A signal
                // dropped here costs the retry only; the op is already applied
                // locally and still reaches peers via namespace sync.
                if let Some(addr) = &self.readiness_addr {
                    addr.do_send(crate::readiness::PendingRepublish {
                        namespace_id,
                        op: *op,
                    });
                } else {
                    debug!(
                        namespace_id = %hex::encode(namespace_id),
                        "ForwardPendingRepublish received before ReadinessManager mounted; \
                         dropping (namespace sync remains the fallback)"
                    );
                }
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
