//! Event handlers for network and node messages.
//!
//! **Purpose**: Handles incoming events from network layer and processes node-level requests.
//! **Structure**: Each event type has its own focused file (SRP).

use actix::{AsyncContext, Handler, WrapFuture};
use calimero_node_primitives::messages::NodeMessage;
use calimero_utils_actix::adapters::ActorExt;
use tracing::debug;

use crate::NodeManager;

// Each handler in its own focused file (SRP)
mod blob_protocol;
pub mod ephemeral;
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
                // is fine to ignore - this is a pure observability query.
                let snapshot = self.state.sync_status_snapshot(&context_id);
                let _ = outcome.send(snapshot);
            }
            NodeMessage::GetMigrationStatusReports {
                namespace_id,
                outcome,
            } => {
                // Synchronous snapshot for the admin `get_migration_status`
                // route, assembled by the same function the receive-path
                // reaction uses so both answer from one map. Pure observability
                // read - a dropped receiver is fine to ignore.
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
                // that window is dropped - the FSM will reconcile when
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
                // The same local-progress signal drives the migration-heartbeat
                // emitter. A governance apply may have advanced the group's
                // target schema or drained residue, so recompute and post the
                // node's facts - this both edge-triggers an on-change heartbeat
                // and seeds the namespace into the emitter so its periodic
                // keep-alive tick goes live.
                self.notify_migration_facts(namespace_id);
            }
            NodeMessage::ForwardNamespaceSubscribed { namespace_id } => {
                // Seed the readiness FSM's `subscribed_at` at subscribe time.
                // Same mount-window caveat as `ForwardNamespaceOpApplied`: a
                // signal dropped before the actor is wired is harmless - the
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
            NodeMessage::ApplyLocalNamespaceOp { op } => {
                // The publisher path wrote this op to the live store and the
                // persisted governance log but never to the in-memory DAG or the
                // unified-op projection; hand it to the same actor entry a peer's
                // op uses so both origins converge on one apply path. Applying it
                // again is safe and deliberate: the live apply short-circuits on
                // finding the op already in the local op-log, so what this call
                // actually does is insert the DAG node and feed the projection,
                // while the op is still live's head.
                let context_client = self.clients.context.clone();
                let work = async move {
                    match context_client.apply_signed_namespace_op(*op).await {
                        Ok(outcome) => {
                            debug!(?outcome, "fed locally-published governance op to own DAG")
                        }
                        // Not fatal: peers still carry the op, and a peer echo
                        // re-offers it to this same handler later.
                        Err(err) => {
                            debug!(%err, "failed to feed locally-published governance op to own DAG")
                        }
                    }
                };
                let _spawn_handle = ctx.spawn(work.into_actor(self));
            }
            NodeMessage::RefreshMigrationFacts { namespace_id } => {
                // Edge-trigger a fact recompute + emit-on-change for this
                // namespace (resync-heal path). Same seam the governance-apply
                // signal uses, without the readiness side-effect - a resync
                // applies no governance op.
                self.notify_migration_facts(namespace_id);
            }
            NodeMessage::SetLocalEphemeral {
                context_id,
                author,
                slice,
                outcome,
            } => {
                let result = crate::handlers::ephemeral::outbound::set_local_ephemeral(
                    self, ctx, context_id, author, slice,
                );
                // A dropped receiver means the caller gave up; ignore.
                let _ = outcome.send(result);
            }
            NodeMessage::GetEphemeralSnapshot {
                context_id,
                outcome,
            } => {
                // Wall clock, the same helper the inbound apply and the TTL
                // sweep use, so the ages reported here are computed against the
                // reading the entries were stamped with.
                let now_ms = crate::handlers::ephemeral::now_ms();
                let entries = self.awareness_store.snapshot(context_id, now_ms);
                let _ = outcome.send(entries);
            }
        }
    }
}
