use actix::{Context, Handler, Message};
use calimero_network_primitives::messages::Publish;
use libp2p::gossipsub::{MessageId, PublishError};

use crate::NetworkManager;

impl Handler<Publish> for NetworkManager {
    type Result = <Publish as Message>::Result;

    fn handle(
        &mut self,
        Publish { topic, data }: Publish,
        _ctx: &mut Context<Self>,
    ) -> Self::Result {
        // The cold-start outbox is scoped to **namespace governance
        // topics** (`ns/<hex>`) and nothing else.
        //
        // Governance ops (ContextRegistered, MemberJoined, group
        // metadata, ...) have no receiver-side pull-recovery path: a
        // publish that lands on an unformed mesh is lost permanently
        // until the next governance op happens to repair it. Queuing
        // those for replay on the next `Subscribed` event is the fix
        // PR #2369 exists for.
        //
        // Context topics (raw base58 context-id) carry state deltas
        // and heartbeats. Those MUST NOT be queued: a state delta is a
        // point-in-time `parent_ids` / `root_hash` /
        // `governance_position` snapshot. Replaying it from the outbox
        // after the receiver's own DAG has advanced makes the storage
        // layer reject the stale application — `Cannot change
        // StorageType`, WASM CRDT-merge divergence — which regressed
        // the `group-multi-service` / `group-metadata` rust-apps e2e
        // tests on PR #2369's earlier pushes. State deltas already
        // have a real recovery path (HashComparison sync-pull driven
        // by heartbeats), so a cold-mesh publish failure is safe to
        // surface as `Err` exactly like `master` does.
        //
        // `gossipsub.publish` consumes `data` and can still return
        // `NoPeersSubscribedToTopic` *after* the consume, so the
        // payload is pre-cloned — but only for queueable topics, which
        // also keeps the common state-delta path allocation-free
        // (addresses Cursor Bugbot #1 on PR #2369).
        let queueable = topic.as_str().starts_with("ns/");
        let outbox_copy = if queueable { data.clone() } else { Vec::new() };
        match self
            .swarm
            .behaviour_mut()
            .gossipsub
            .publish(topic.clone(), data)
        {
            Ok(id) => Ok(id),
            // Queueable topic, no subscribers yet: stash for replay on
            // the next `Subscribed` event. Synthesise an empty
            // `MessageId` so the queued path is indistinguishable to
            // callers from a direct accept — every caller already
            // treats publish as eventually-consistent.
            Err(PublishError::NoPeersSubscribedToTopic) if queueable => {
                self.publish_outbox.enqueue(topic, outbox_copy);
                Ok(MessageId::new(&[]))
            }
            // Non-queueable topic (state delta / heartbeat) or any
            // other publish error: propagate exactly as `master` does.
            Err(e) => Err(e.into()),
        }
    }
}
