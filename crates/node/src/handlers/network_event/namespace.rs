use std::collections::HashSet;

use actix::{AsyncContext, WrapFuture};
use calimero_context::governance_broadcast::sign_ack;
use calimero_context::group_store::NamespaceRepository;
use calimero_context_client::local_governance::{
    hash_scoped_namespace, NamespaceTopicMsg, SignedNamespaceOp,
};
use calimero_context_client::messages::NamespaceApplyOutcome;
use calimero_context_config::types::ContextGroupId;
use calimero_network_primitives::client::NetworkClient;
use calimero_node_primitives::sync::{BroadcastMessage, MAX_SIGNED_GROUP_OP_PAYLOAD_BYTES};
use calimero_primitives::identity::PrivateKey;
use libp2p::gossipsub::TopicHash;
use tracing::{debug, info, warn};
use zeroize::Zeroize;

use crate::sync::parent_pull::{NextPeer, ParentPullBudget};
use crate::NodeManager;

pub(super) fn handle_namespace_governance_delta(
    this: &mut NodeManager,
    ctx: &mut actix::Context<NodeManager>,
    source: libp2p::PeerId,
    namespace_id: [u8; 32],
    payload: Vec<u8>,
) {
    if payload.len() > MAX_SIGNED_GROUP_OP_PAYLOAD_BYTES {
        warn!(
            len = payload.len(),
            "oversized NamespaceGovernanceDelta payload"
        );
        return;
    }

    let msg: NamespaceTopicMsg = match borsh::from_slice(&payload) {
        Ok(msg) => msg,
        Err(err) => {
            warn!(%err, "failed to decode NamespaceTopicMsg payload");
            return;
        }
    };

    let op = match msg {
        NamespaceTopicMsg::Op(op) => op,
        NamespaceTopicMsg::Ack(ack) => {
            // Phase 4: route the ack to whatever in-flight
            // `publish_and_await_ack` caller is waiting on this op_hash.
            // `route` returns false if no subscriber registered — fine,
            // it just means the publish completed already (or wasn't ours).
            let _ = this.clients.context.ack_router().route(ack);
            return;
        }
        // Phase 7.3: forward readiness variants to the dedicated
        // submodule. Beacon receive verifies + inserts into the cache;
        // probe receive forwards to the rate-limited
        // `EmitOutOfCycleBeacon` handler on `ReadinessManager`.
        //
        // Cross-check `inner.namespace_id == namespace_id` (the topic's
        // namespace) BEFORE forwarding — without this, a peer could
        // publish a beacon claiming `beacon.namespace_id = X` on the
        // gossipsub topic for namespace Y, polluting namespace X's
        // cache from a Y subscription. Mirrors the existing `Op` arm
        // check below.
        NamespaceTopicMsg::ReadinessBeacon(beacon) => {
            if beacon.namespace_id != namespace_id.into() {
                warn!("ReadinessBeacon namespace_id mismatch with topic; dropping");
                return;
            }
            super::readiness::handle_readiness_beacon(this, ctx, source, beacon);
            return;
        }
        NamespaceTopicMsg::ReadinessProbe(probe) => {
            if probe.namespace_id != namespace_id.into() {
                warn!("ReadinessProbe namespace_id mismatch with topic; dropping");
                return;
            }
            super::readiness::handle_readiness_probe(this, ctx, source, probe);
            return;
        }
        // PR-6c Task 6c.8: ingest a signed migration heartbeat into the
        // per-(namespace, peer) TTL cache. Cross-check the heartbeat's
        // namespace against the topic's namespace FIRST (same guard as the
        // `ReadinessBeacon` arm) so a peer can't pollute namespace X's cache
        // from a namespace-Y subscription. Then verify the Ed25519 signature
        // AND cohort membership via `verify_migration_heartbeat` before
        // inserting — an unsigned or non-member heartbeat must never enter a
        // rollup. The heartbeat is ephemeral telemetry, not governance state,
        // so there is no apply / ack / backfill — just the cache upsert.
        NamespaceTopicMsg::MigrationHeartbeat(heartbeat) => {
            if heartbeat.namespace_id != namespace_id.into() {
                warn!("MigrationHeartbeat namespace_id mismatch with topic; dropping");
                return;
            }
            if !calimero_context::governance_broadcast::verify_migration_heartbeat(
                &this.datastore,
                &heartbeat,
            ) {
                debug!(
                    namespace_id = %hex::encode(heartbeat.namespace_id.as_bytes()),
                    "MigrationHeartbeat failed verification (sig/membership); dropping"
                );
                return;
            }
            this.migration_status_cache.insert(&heartbeat);
            debug!(
                namespace_id = %hex::encode(heartbeat.namespace_id.as_bytes()),
                peer = %heartbeat.peer_pubkey,
                schema_version = heartbeat.schema_version,
                residue_auto = heartbeat.residue_auto,
                residue_identity = heartbeat.residue_identity,
                "migration heartbeat cached"
            );
            return;
        }
    };

    if op.namespace_id != namespace_id.into() {
        warn!("NamespaceGovernanceDelta namespace_id mismatch with topic");
        return;
    }

    if let Err(err) = op.verify_signature() {
        warn!(%err, "NamespaceGovernanceDelta signature verification failed");
        return;
    }

    let context_client = this.clients.context.clone();
    let node_client = this.clients.node.clone();
    let node_state = this.state.clone();
    let network_client = this.managers.sync.network_client.clone();
    // Clone of the sync manager used to fire reconcile-via-anchor on
    // a `MemberRemoved` / `MemberLeft` apply that reports state-hash
    // divergence. Cloning is cheap (Arc-wrapped fields) and the
    // spawned task needs an owned handle.
    let sync_manager = this.managers.sync.clone();
    let sync_timeout = this.managers.sync.sync_config.timeout;
    let pull_budget_max_peers = this.managers.sync.sync_config.parent_pull_additional_peers;
    let pull_budget_duration = this.managers.sync.sync_config.parent_pull_budget;
    let readiness_addr = this.readiness_addr.clone();
    // PR-6c Task 6c.8: capture the migration-heartbeat emitter address + a
    // datastore handle so the apply path can drive an on-change heartbeat once
    // the governance op applies (mirrors the `readiness_addr` capture). Cloning
    // is cheap (Arc-wrapped) and the spawned future needs owned handles.
    let migration_emitter_addr = this.migration_emitter_addr.clone();
    let migration_datastore = this.datastore.clone();

    let op_for_ack = op.clone();
    // Capture the signer for the peer-identity cache before `op` is
    // consumed by `apply_signed_namespace_op` below. Signature was
    // already verified above, so the identity is real; the recording
    // itself is gated on `Applied` below to skip relayed duplicates and
    // pending ops where the delivering peer didn't necessarily originate
    // the message.
    let signer = op.signer;
    let _ignored = ctx.spawn(
        async move {
            let outcome = match context_client.apply_signed_namespace_op(op).await {
                Ok(outcome) => outcome,
                Err(err) => {
                    warn!(?err, %source, "failed to apply namespace governance delta");
                    return;
                }
            };

            // Notify the ReadinessManager FSM that we've made local
            // progress on this namespace. Without this signal,
            // `state_per_namespace` stays empty forever, no beacons emit,
            // and the readiness subsystem is inert (#2269 cursor[bot]
            // HIGH-severity finding). `Pending` and `Duplicate` outcomes
            // do NOT advance our applied count — Pending is waiting on
            // parents (no real progress yet) and Duplicate is a re-deliver.
            if let NamespaceApplyOutcome::Applied { divergence } = &outcome {
                if let Some(addr) = &readiness_addr {
                    addr.do_send(crate::readiness::NamespaceOpApplied { namespace_id });
                }

                // PR-6c Task 6c.8: drive the migration-heartbeat emitter on the
                // same applied edge. Recompute the node's facts from the (now
                // updated) local governance state and post them — this seeds the
                // namespace into the emitter (making its periodic keep-alive
                // tick live) and edge-triggers an on-change heartbeat when the
                // target schema / residue changed. Best-effort: a `None` address
                // (emitter not yet mounted) drops the signal; the next applied
                // op re-drives it.
                if let Some(addr) = &migration_emitter_addr {
                    let facts = crate::migration_status::compute_namespace_migration_facts(
                        &migration_datastore,
                        namespace_id,
                    );
                    addr.do_send(crate::migration_status::MigrationFactsUpdate {
                        namespace_id,
                        facts,
                    });
                }

                // Record the (peer, identity) pair now that the
                // signature verified, the nonce was monotonic, and the
                // op applied successfully. Consumed by anchor-preferred
                // sync peer selection. See `NodeState::peer_identities`.
                //
                // `None`: this path doesn't carry the signer's group +
                // role cheaply, so the observation updates only the
                // in-memory reverse view. The durable cache is populated
                // from the state-delta path (which has both) — the deltas
                // that follow a member's governance op re-observe them
                // there, so cold-start coverage is unaffected.
                node_state.observe_peer_identity(source, signer, None);

                // Governance-pending active drain: a governance op that just applied may
                // unblock state deltas previously buffered as `Unknown`.
                // Without this hook the lazy on-state-delta drain
                // deadlocks when the only state delta in flight is one
                // waiting for that very governance op (e2e 3-node test
                // reproduced this).
                let drain_input = crate::handlers::state_delta::StateDeltaContext {
                    node_clients: crate::state::NodeClients {
                        context: context_client.clone(),
                        node: node_client.clone(),
                    },
                    node_state: node_state.clone(),
                    network_client: network_client.clone(),
                    sync_timeout,
                };
                crate::handlers::state_delta::drain_all_governance_pending(&drain_input).await;
                // PR-6b Task 6b.5: a cascade-upgrade governance op that just
                // applied bumps the group's target schema and is the catalyst
                // for this node's lazy binary advance. Drain any absorbed
                // straggler deltas the now-loaded reader can read (verbatim
                // replay); no-op for contexts that haven't advanced.
                crate::handlers::state_delta::drain_all_absorbed(&drain_input).await;

                // Reconcile-via-anchor: if `MemberRemoved` / `MemberLeft`
                // apply reported state-hash divergence from the signed
                // claims, pull canonical state from a trusted anchor.
                // The sync manager method picks an anchor peer (from
                // the group's trusted-anchor set, filtered through the
                // verified peer-identity cache) and verifies the
                // post-adoption hash against the signed expected.
                // Best-effort: no anchor connected → logged and
                // dropped; re-attempted on the next signed op or
                // sync tick.
                //
                // Detached via `actix::spawn` so a multi-second
                // Snapshot / HashComparison sync doesn't block the
                // governance-apply task — the actor's mailbox needs
                // to keep draining (other signed ops, acks, and the
                // proactive backfill for `Pending`). `actix::spawn`
                // is the right primitive here: the inner future
                // touches non-`Send` types via the sync manager, so
                // `tokio::spawn` (which requires `Send`) can't take
                // it; `actix::spawn` schedules on the current arbiter
                // and stays single-threaded with the actor. The
                // arbiter's task queue runs the detached future
                // concurrently with the actor's mailbox drain.
                // Errors are already logged inside
                // `reconcile_after_divergence`; the spawned task has
                // no return value to consume.
                if let Some(report) = divergence {
                    let sm = sync_manager.clone();
                    let report = report.clone();
                    drop(actix::spawn(async move {
                        sm.reconcile_after_divergence(report).await;
                    }));
                }

                // Prompt key recovery on receipt (#2613). A gossiped Group
                // op we couldn't decrypt is now buffered awaiting its key.
                // Trigger a direct key pull immediately rather than waiting
                // for a sync tick — crucially, this is the ONLY recovery
                // trigger for a namespace/subgroup member that holds no
                // local context (e.g. a Restricted-subgroup member just
                // added via `add_group_members`): the per-context interval
                // recovery never runs for it, and without a prompt pull it
                // never decrypts the subgroup's ops (membership,
                // `ContextRegistered`) and so can't see or join the
                // subgroup. Cheap when nothing is awaiting (one op-log
                // scan); tries multiple peers. Detached for the same
                // non-`Send` reason as the reconcile above.
                {
                    let sm = sync_manager.clone();
                    drop(actix::spawn(async move {
                        sm.recover_missing_group_keys(namespace_id, None).await;
                    }));
                }
            }

            // Phase 4: emit a `SignedAck` on the same topic when we've
            // newly applied the op. `Pending` (waiting on parents) and
            // `Duplicate` (we already had it — likely already acked
            // earlier) deliberately don't ack: Pending would lie about
            // application, and Duplicate would just inflate gossip with
            // no observable change to the publisher's dedup-by-signer
            // counting.
            if matches!(outcome, NamespaceApplyOutcome::Applied { .. }) {
                emit_namespace_ack(&context_client, &network_client, namespace_id, &op_for_ack)
                    .await;
            }

            // Proactive backfill (#2198) fires ONLY for `Pending` — the
            // DAG accepted the op but can't apply it until missing parents
            // arrive. `Applied` is the steady-state happy path; `Duplicate`
            // means we already have the op (very common on gossip, since
            // every mesh peer rebroadcasts), and triggering a backfill for
            // it would open a stream and request the full namespace state
            // for nothing.
            //
            // NOTE: we MUST ask `source` first before handing off to
            // `resolve_namespace_pending`. That helper seeds its
            // `ParentPullBudget` with the initial peer marked as already
            // tried, so passing `source` to it directly without a prior
            // fetch means `source` never actually gets queried — which in
            // a 2-node mesh (where no other peers exist) silently does
            // nothing. Empty `delta_ids` means "give me everything for
            // this namespace" on the responder side.
            if matches!(outcome, NamespaceApplyOutcome::Pending) {
                debug!(
                    %source,
                    namespace_id = %hex::encode(namespace_id),
                    "gossip governance op is pending; triggering proactive backfill"
                );
                fetch_and_apply_namespace_backfill(
                    &context_client,
                    &node_client,
                    &network_client,
                    &sync_manager,
                    source,
                    namespace_id,
                    Vec::new(),
                    sync_timeout,
                    &node_state,
                )
                .await;
                resolve_namespace_pending(
                    &context_client,
                    &node_client,
                    &network_client,
                    &sync_manager,
                    source,
                    namespace_id,
                    sync_timeout,
                    pull_budget_max_peers,
                    pull_budget_duration,
                    &node_state,
                )
                .await;
            }

            // Group-key delivery to a new joiner is no longer pushed from
            // here onto the namespace governance DAG. The joiner pulls the
            // key directly from a sync peer when it next syncs the
            // namespace and finds it lacks the key (see
            // `SyncManager::recover_missing_group_keys`), which gives it a
            // durable retry instead of the old single fire-and-forget shot.
        }
        .into_actor(this),
    );
}

pub(super) fn handle_namespace_state_heartbeat(
    this: &mut NodeManager,
    ctx: &mut actix::Context<NodeManager>,
    source: libp2p::PeerId,
    namespace_id: [u8; 32],
    peer_heads: Vec<[u8; 32]>,
) {
    // Cap peer-supplied heads to prevent DoS via oversized heartbeat.
    const MAX_PEER_HEADS: usize = 256;
    if peer_heads.len() > MAX_PEER_HEADS {
        warn!(
            %source,
            heads = peer_heads.len(),
            "Namespace heartbeat exceeds max peer heads, ignoring"
        );
        return;
    }

    // Phase 11.2 (#2237): the heartbeat is now liveness-only. The
    // active catch-up arms (republishing ops the peer is missing,
    // backfilling ops we are missing, and the cross-peer
    // `resolve_namespace_pending` fan-out) are gone. With the
    // three-phase contract in place (Phase 5+6+7+8):
    //
    //   * `publish_and_await_ack_namespace` blocks the publisher until
    //     at least one mesh peer applies + acks, so a freshly-applied
    //     op is already known to be on at least one receiver before the
    //     publisher returns.
    //   * `parent_pull` runs on every gossip op whose parents are
    //     missing locally — that's the on-receive recovery path.
    //   * `ReadinessBeacon` carries `applied_through` and `dag_head`,
    //     so peers detect divergence and pick a sync partner via
    //     `pick_sync_partner` without needing the heartbeat to
    //     advertise heads.
    //
    // The heartbeat-driven catch-up was the fallback before any of
    // those existed. Keeping it now duplicates work the new path
    // already performs, and makes the system noisier without making it
    // more correct (a genuinely stuck DAG is recovered by the join /
    // probe / beacon flow, not by the heartbeat). We still log the
    // divergence detection at debug for observability — operators
    // chasing a wedged namespace can grep for the `peer_heads` count
    // mismatch — but no remediation runs from here.
    let context_client = this.clients.context.clone();

    let _ignored = ctx.spawn(
        async move {
            let store = context_client.datastore_handle().into_inner();
            let ns_head_key = calimero_store::key::NamespaceGovHead::new(namespace_id);
            let handle = store.handle();
            let local_heads: HashSet<[u8; 32]> = match handle.get(&ns_head_key) {
                Ok(Some(h)) => h.dag_heads.into_iter().collect(),
                _ => HashSet::new(),
            };
            drop(handle);

            let we_missing = peer_heads
                .iter()
                .filter(|h| !local_heads.contains(*h))
                .count();
            let peer_head_set: HashSet<[u8; 32]> = peer_heads.iter().copied().collect();
            let peer_missing = local_heads
                .iter()
                .filter(|h| !peer_head_set.contains(*h))
                .count();

            if we_missing == 0 && peer_missing == 0 {
                return;
            }
            tracing::debug!(
                namespace_id = %hex::encode(namespace_id),
                %source,
                we_missing,
                peer_missing,
                "namespace heartbeat: divergence detected (liveness-only — recovery via \
                 publish_and_await_ack / parent_pull / readiness beacon)"
            );
        }
        .into_actor(this),
    );
}

/// Iterate other namespace-mesh peers asking for backfill until the local
/// governance DAG has no more pending ops for this namespace, or the retry
/// budget is exhausted.
///
/// Uses empty-body `NamespaceBackfillRequest` (semantics: "give me everything
/// for this namespace", per `handle_namespace_backfill_request`) because
/// callers don't know which specific ancestor ids are still missing — the
/// pending chain can be arbitrarily deep, and the responder caps at
/// `MAX_BACKFILL_OPS` per response anyway.
#[allow(
    clippy::too_many_arguments,
    reason = "orthogonal args on a consensus sync/handler path; no cohesive grouping"
)]
async fn resolve_namespace_pending(
    context_client: &calimero_context_client::client::ContextClient,
    node_client: &calimero_node_primitives::client::NodeClient,
    network_client: &NetworkClient,
    sync_manager: &crate::sync::SyncManager,
    initial_peer: libp2p::PeerId,
    namespace_id: [u8; 32],
    sync_timeout: tokio::time::Duration,
    max_additional_peers: usize,
    budget: tokio::time::Duration,
    node_state: &crate::NodeState,
) {
    let topic = libp2p::gossipsub::TopicHash::from_raw(format!("ns/{}", hex::encode(namespace_id)));
    let mut mesh_peers = network_client.mesh_peers(topic.clone()).await;
    let mut scheduler = ParentPullBudget::new(initial_peer, max_additional_peers, budget);

    loop {
        match namespace_has_pending(context_client, namespace_id).await {
            Ok(false) => break,
            Ok(true) => {}
            Err(err) => {
                // Fail loud rather than pretend convergence: a query error is
                // unknown state, not "zero pending". Spinning on the same
                // error is pointless, so we exit the retry loop; the next
                // heartbeat-triggered `resolve_namespace_pending` pass will
                // retry the check naturally.
                warn!(
                    ?err,
                    namespace_id = %hex::encode(namespace_id),
                    "namespace_pending_op_count failed; aborting cross-peer retry"
                );
                break;
            }
        }

        let next_peer = match scheduler.next(&mesh_peers) {
            NextPeer::Peer(p) => p,
            NextPeer::RefetchMesh => {
                mesh_peers = network_client.mesh_peers(topic.clone()).await;
                scheduler.record_refetch();
                match scheduler.next(&mesh_peers) {
                    NextPeer::Peer(p) => p,
                    other => {
                        debug!(
                            namespace_id = %hex::encode(namespace_id),
                            ?other,
                            "no additional ns mesh peers for parent pull"
                        );
                        break;
                    }
                }
            }
            NextPeer::BudgetExhausted => {
                warn!(
                    namespace_id = %hex::encode(namespace_id),
                    "namespace parent-pull budget exhausted"
                );
                break;
            }
            NextPeer::MaxPeersReached | NextPeer::NoMorePeers => break,
        };

        scheduler.record_attempt(next_peer);
        info!(
            namespace_id = %hex::encode(namespace_id),
            ?next_peer,
            attempt = scheduler.attempts(),
            "retrying namespace backfill against additional mesh peer"
        );

        fetch_and_apply_namespace_backfill(
            context_client,
            node_client,
            network_client,
            sync_manager,
            next_peer,
            namespace_id,
            Vec::new(),
            sync_timeout,
            node_state,
        )
        .await;
    }
}

#[allow(
    clippy::too_many_arguments,
    reason = "orthogonal args on a consensus sync/handler path; no cohesive grouping"
)]
async fn fetch_and_apply_namespace_backfill(
    context_client: &calimero_context_client::client::ContextClient,
    node_client: &calimero_node_primitives::client::NodeClient,
    network_client: &NetworkClient,
    sync_manager: &crate::sync::SyncManager,
    peer: libp2p::PeerId,
    namespace_id: [u8; 32],
    delta_ids: Vec<[u8; 32]>,
    sync_timeout: tokio::time::Duration,
    node_state: &crate::NodeState,
) {
    let Ok(mut stream) = network_client.open_stream(peer).await else {
        debug!(
            %peer,
            "failed to open stream for namespace backfill"
        );
        return;
    };

    let msg = calimero_node_primitives::sync::StreamMessage::Init {
        context_id: calimero_primitives::context::ContextId::from([0u8; 32]),
        party_id: calimero_primitives::identity::PublicKey::from([0u8; 32]),
        payload: calimero_node_primitives::sync::InitPayload::NamespaceBackfillRequest {
            namespace_id,
            delta_ids,
        },
        next_nonce: {
            use rand::Rng;
            rand::thread_rng().gen()
        },
    };

    if let Err(err) = crate::sync::stream::send(&mut stream, &msg, None).await {
        debug!(%err, "failed to send NamespaceBackfillRequest");
        return;
    }

    match crate::sync::stream::recv(&mut stream, None, sync_timeout).await {
        Ok(Some(calimero_node_primitives::sync::StreamMessage::Message {
            payload:
                calimero_node_primitives::sync::MessagePayload::NamespaceBackfillResponse { deltas },
            ..
        })) => {
            let mut any_applied = false;
            // Collect divergence reports from `MemberRemoved` /
            // `MemberLeft` ops applying via the backfill path. Same
            // reasoning as the gossip-receive path: once the DAG
            // marks the op `Applied`, any later gossipsub delivery of
            // the same op becomes `Duplicate` and the apply work —
            // including the post-apply hash check — is skipped. If
            // divergence surfaces here and we drop it, no later path
            // will re-emit it. Defer firing until after the batch
            // so the apply loop is contiguous.
            let mut pending_divergences: Vec<calimero_context_client::messages::DivergenceReport> =
                Vec::new();
            for (delta_id, op_bytes) in deltas {
                if let Ok(op) = borsh::from_slice::<SignedNamespaceOp>(&op_bytes) {
                    match context_client.apply_signed_namespace_op(op).await {
                        Ok(NamespaceApplyOutcome::Applied { divergence }) => {
                            any_applied = true;
                            if let Some(report) = divergence {
                                pending_divergences.push(report);
                            }
                        }
                        Ok(_) => {}
                        Err(err) => {
                            warn!(
                                %peer,
                                namespace_id = %hex::encode(namespace_id),
                                delta_id = %hex::encode(delta_id),
                                ?err,
                                "failed to apply namespace backfill op"
                            );
                        }
                    }
                }
            }
            // Route any divergences surfaced by the apply loop to the
            // reconcile-via-anchor path. The helper itself logs and
            // backs off internally; no error to propagate here.
            for report in pending_divergences {
                sync_manager.reconcile_after_divergence(report).await;
            }
            if any_applied {
                // FSM notify after the batch — same rationale as the
                // gossip-receive path (line 120).
                node_client.notify_namespace_op_applied(namespace_id);

                // Governance-pending active drain: backfilled governance ops may unblock
                // state deltas previously buffered as `Unknown`. Same
                // hook as the gossip-apply path.
                let drain_input = crate::handlers::state_delta::StateDeltaContext {
                    node_clients: crate::state::NodeClients {
                        context: context_client.clone(),
                        node: node_client.clone(),
                    },
                    node_state: node_state.clone(),
                    network_client: network_client.clone(),
                    sync_timeout,
                };
                crate::handlers::state_delta::drain_all_governance_pending(&drain_input).await;
                // PR-6b Task 6b.5: backfilled cascade-upgrade ops may have
                // advanced this node's target schema and triggered a lazy
                // binary advance — drain absorbed straggler deltas the loaded
                // reader can now read (verbatim replay). Same hook as the
                // gossip-apply path.
                crate::handlers::state_delta::drain_all_absorbed(&drain_input).await;
            }
        }
        _ => {
            debug!("unexpected response to NamespaceBackfillRequest");
        }
    }
}

/// Returns `Ok(true)` if this node's governance DAG has ops whose parents
/// are not yet local (the pending queue is non-empty).
///
/// Surfaces query errors to the caller rather than swallowing them with
/// `unwrap_or(0)` — an error here is *not* the same signal as "zero pending
/// ops", and collapsing the two caused the cross-peer retry loop to exit as
/// if the DAG were fully resolved when the real state was unknown. Mirrors
/// the data-delta path's `get_missing_parents()`, where a query failure is
/// equally observable rather than silenced.
async fn namespace_has_pending(
    context_client: &calimero_context_client::client::ContextClient,
    namespace_id: [u8; 32],
) -> eyre::Result<bool> {
    Ok(context_client
        .namespace_pending_op_count(namespace_id)
        .await?
        > 0)
}

/// Sign and publish a `SignedAck` for an applied namespace op.
///
/// Best-effort: every failure path (no namespace identity yet, ack
/// signing error, gossipsub publish error) logs at debug/warn and
/// returns. The publisher will time out and retry the op rather than
/// being told falsely that it was acked.
async fn emit_namespace_ack(
    context_client: &calimero_context_client::client::ContextClient,
    network_client: &NetworkClient,
    namespace_id: [u8; 32],
    op: &SignedNamespaceOp,
) {
    let topic_str = format!("ns/{}", hex::encode(namespace_id));
    let topic = TopicHash::from_raw(topic_str.clone());
    let op_hash = match hash_scoped_namespace(topic_str.as_bytes(), op) {
        Ok(h) => h,
        Err(err) => {
            warn!(%err, "ack: failed to hash op for ack signing; skipping");
            return;
        }
    };

    let store = context_client.datastore();
    let ns_group = ContextGroupId::from(namespace_id);
    let mut identity = match NamespaceRepository::new(store).identity(&ns_group) {
        Ok(Some(t)) => t,
        Ok(None) => {
            debug!(
                namespace_id = %hex::encode(namespace_id),
                "ack: no namespace identity yet; skipping"
            );
            return;
        }
        Err(err) => {
            warn!(%err, "ack: namespace identity lookup failed; skipping");
            return;
        }
    };
    // `PrivateKey` zeroizes its inner buffer on drop, but the `[u8; 32]`
    // returned by `get_namespace_identity` is `Copy` — constructing the
    // `PrivateKey` leaves the original tuple field intact on the stack
    // until the function returns. Zeroize the leftover bytes explicitly
    // so the only remaining copy is inside the `PrivateKey`. (Systemic
    // fix lives at `get_namespace_identity` returning a `PrivateKey`
    // directly — out of scope for Phase 4.)
    let signer_sk = PrivateKey::from(identity.1);
    identity.1.zeroize();
    identity.2.zeroize();

    let ack = match sign_ack(&signer_sk, op_hash) {
        Ok(ack) => ack,
        Err(err) => {
            warn!(%err, "ack: signing failed; skipping");
            return;
        }
    };
    let inner = match borsh::to_vec(&NamespaceTopicMsg::Ack(ack)) {
        Ok(p) => p,
        Err(err) => {
            warn!(%err, "ack: borsh encode failed; skipping");
            return;
        }
    };
    // Receiver decodes the gossipsub frame as `BroadcastMessage` first
    // (see `network_event.rs` and `client.rs::publish_signed_namespace_op`),
    // then unwraps `payload` as `NamespaceTopicMsg`. Publishing the inner
    // `NamespaceTopicMsg` raw would deserialize-fail at the receiver and
    // be silently dropped, defeating the ack. `delta_id`/`parent_ids` are
    // not DAG-bound for an Ack — they're discarded by the receive path.
    let envelope = BroadcastMessage::NamespaceGovernanceDelta {
        namespace_id,
        delta_id: [0u8; 32],
        parent_ids: vec![],
        payload: inner,
    };
    let bytes = match borsh::to_vec(&envelope) {
        Ok(b) => b,
        Err(err) => {
            warn!(%err, "ack: envelope encode failed; skipping");
            return;
        }
    };
    if let Err(err) = network_client.publish(topic, bytes).await {
        // Non-fatal — ack is fire-and-forget; sender will time out and retry.
        debug!(%err, "ack: publish failed; sender will retry on timeout");
    }
}
