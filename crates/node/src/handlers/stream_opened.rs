//! StreamOpened event handling
//!
//! **SRP**: This module has ONE job - route incoming streams to the correct handler

use std::collections::hash_map::Entry;
use std::collections::HashMap;
use std::sync::{LazyLock, Mutex, PoisonError};

use calimero_network_primitives::stream::Stream;
use libp2p::{PeerId, StreamProtocol};
use tokio::sync::{Semaphore, SemaphorePermit};
use tracing::{debug, info, warn};

use crate::handlers::blob_announce::handle_blob_announce_stream;
use crate::handlers::blob_protocol::handle_blob_protocol_stream;
use crate::sync_session_bridge::{SyncSessionJob, SyncSessionSendError};
use crate::NodeManager;

const MAX_INBOUND_BLOB_STREAMS: usize = 64; // transfer and announce streams handled at once
const MAX_INBOUND_BLOB_STREAMS_PER_PEER: usize = 16; // of those, held by any one peer

static BLOB_STREAM_SLOTS: Semaphore = Semaphore::const_new(MAX_INBOUND_BLOB_STREAMS);
static BLOB_STREAMS_PER_PEER: LazyLock<Mutex<HashMap<PeerId, usize>>> =
    LazyLock::new(Mutex::default);

/// One inbound blob stream's share of both limits, given back when its task ends.
struct BlobStreamPermit {
    _slot: SemaphorePermit<'static>,
    peer: PeerId,
}

impl BlobStreamPermit {
    /// `None` when the peer or the node is at its limit: the caller drops the
    /// stream rather than queueing work on a remote peer's say-so.
    fn try_acquire(peer: PeerId) -> Option<Self> {
        let mut per_peer = BLOB_STREAMS_PER_PEER
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let held = per_peer.get(&peer).copied().unwrap_or(0);
        if held >= MAX_INBOUND_BLOB_STREAMS_PER_PEER {
            return None;
        }
        let slot = BLOB_STREAM_SLOTS.try_acquire().ok()?;
        let _previous = per_peer.insert(peer, held + 1);
        Some(Self { _slot: slot, peer })
    }
}

impl Drop for BlobStreamPermit {
    fn drop(&mut self) {
        let mut per_peer = BLOB_STREAMS_PER_PEER
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if let Entry::Occupied(mut held) = per_peer.entry(self.peer) {
            *held.get_mut() -= 1;
            if *held.get() == 0 {
                let _count = held.remove();
            }
        }
    }
}

/// Handles StreamOpened event by routing to blob or sync protocol
///
/// Protocol routing:
/// - `CALIMERO_BLOB_PROTOCOL` → blob_protocol::handle_blob_protocol_stream
/// - `CALIMERO_BLOB_ANNOUNCE_PROTOCOL` → blob_announce::handle_blob_announce_stream
/// - All other protocols → SyncManager::handle_opened_stream
pub fn handle_stream_opened(
    node_manager: &mut NodeManager,
    _ctx: &mut <NodeManager as actix::Actor>::Context,
    peer_id: PeerId,
    stream: Box<Stream>,
    protocol: StreamProtocol,
) {
    let is_blob_protocol = protocol == calimero_network_primitives::stream::CALIMERO_BLOB_PROTOCOL
        || protocol == calimero_network_primitives::stream::CALIMERO_BLOB_ANNOUNCE_PROTOCOL;
    let permit = if is_blob_protocol {
        let Some(permit) = BlobStreamPermit::try_acquire(peer_id) else {
            debug!(%peer_id, %protocol, "Refusing inbound blob stream: stream limit reached");
            return;
        };
        Some(permit)
    } else {
        None
    };

    // Route streams based on protocol
    if protocol == calimero_network_primitives::stream::CALIMERO_BLOB_PROTOCOL {
        info!(%peer_id, "Routing to blob protocol handler");
        let node_client = node_manager.clients.node.clone();
        let context_client = node_manager.clients.context.clone();
        // Serve the blob on the global tokio runtime, NOT on the NodeManager
        // actor's arbiter (`ctx.spawn`). Blob serving can run up to
        // `BLOB_SERVE_TIMEOUT` (5 min); an `into_actor` future occupies the
        // actor's single-threaded arbiter and blocks NodeManager message
        // handling for that whole duration. Sync sessions were already moved off
        // this arbiter for the same reason; blob serving needs the same
        // treatment. `handle_blob_protocol_stream` only needs owned, `Send`
        // handles, so a detached `tokio::spawn` is sufficient.
        drop(tokio::spawn(async move {
            let _permit = permit;
            if let Err(err) =
                handle_blob_protocol_stream(node_client, context_client, peer_id, stream).await
            {
                debug!(%peer_id, error = %err, "Failed to handle blob protocol stream");
            }
        }));
    } else if protocol == calimero_network_primitives::stream::CALIMERO_BLOB_ANNOUNCE_PROTOCOL {
        debug!(%peer_id, "Routing to blob announce handler");
        let node_client = node_manager.clients.node.clone();
        let context_client = node_manager.clients.context.clone();
        // Detached like the blob-serving path above, and for the same reason:
        // a prefetch can run for as long as a transfer, which must not occupy
        // the NodeManager arbiter.
        drop(tokio::spawn(async move {
            let _permit = permit;
            if let Err(err) =
                handle_blob_announce_stream(node_client, context_client, peer_id, stream).await
            {
                debug!(%peer_id, error = %err, "Failed to handle blob announce stream");
            }
        }));
    } else {
        debug!(%peer_id, "Routing to sync protocol handler");
        // Route inbound sync streams onto the dedicated SyncSessionActor
        // arbiter (issue #2316). On Full/Closed we drop the stream and
        // rely on peer retry; the bounded mailbox is the whole point of
        // moving sync sessions off this actor's arbiter.
        match node_manager
            .sync_session_tx
            .try_send(SyncSessionJob::Responder { peer_id, stream })
        {
            Ok(()) => {}
            Err(SyncSessionSendError::Full) => {
                warn!(
                    %peer_id,
                    "SyncSession actor mailbox full — dropping inbound sync stream (#2316); peer will retry"
                );
            }
            Err(SyncSessionSendError::Closed) => {
                warn!(
                    %peer_id,
                    "SyncSession actor closed — dropping inbound sync stream"
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use calimero_network_primitives::blob_types::{BlobRequest, BlobResponse};
    use calimero_network_primitives::messages::NetworkEvent;
    use calimero_network_primitives::stream::{Message, Stream, CALIMERO_BLOB_PROTOCOL};
    use calimero_primitives::blobs::BlobId;
    use calimero_primitives::context::ContextId;
    use futures_util::{FutureExt, SinkExt, StreamExt};
    use libp2p::PeerId;
    use serial_test::serial;

    use super::*;
    use crate::handlers::blob_protocol::BLOB_REQUEST_READ_TIMEOUT;
    use crate::test_node_harness::{boot_test_node, TestNode};

    /// Upper bound on the node acting on a stream, beyond the read timeout.
    const SETTLE: Duration = Duration::from_secs(5);

    /// Opens a blob stream from `peer` to the node and returns the peer's end.
    async fn open(node: &TestNode, peer: PeerId) -> Stream {
        let (ours, theirs) = Stream::test_pair();
        node.node_addr
            .send(NetworkEvent::StreamOpened {
                peer_id: peer,
                stream: Box::new(ours),
                protocol: CALIMERO_BLOB_PROTOCOL,
            })
            .await
            .expect("deliver StreamOpened to the node actor");
        theirs
    }

    async fn closed_by_node(stream: &mut Stream, within: Duration) -> bool {
        matches!(
            tokio::time::timeout(within, stream.next()).await,
            Ok(None | Some(Err(_)))
        )
    }

    /// Peers that open blob streams and never send a request hold at most the
    /// per-peer and total share, lose them after the read timeout, and an
    /// honest request is served afterwards.
    #[tokio::test]
    #[serial(boot_test_node)]
    async fn silent_blob_streams_are_bounded_and_released() {
        let node = boot_test_node().await;
        let flooder = PeerId::random();

        let mut held = Vec::new();
        for _ in 0..MAX_INBOUND_BLOB_STREAMS_PER_PEER {
            held.push(open(&node, flooder).await);
        }
        let mut over_peer = open(&node, flooder).await;
        assert!(
            closed_by_node(&mut over_peer, SETTLE).await,
            "one peer holds no more than its share"
        );

        for _ in MAX_INBOUND_BLOB_STREAMS_PER_PEER..MAX_INBOUND_BLOB_STREAMS {
            held.push(open(&node, PeerId::random()).await);
        }
        let mut over_total = open(&node, PeerId::random()).await;
        assert!(
            closed_by_node(&mut over_total, SETTLE).await,
            "the node holds no more than its total"
        );
        assert!(
            held[0].next().now_or_never().is_none(),
            "admitted streams are still waiting for their request"
        );

        for stream in &mut held {
            assert!(
                closed_by_node(stream, BLOB_REQUEST_READ_TIMEOUT + SETTLE).await,
                "a silent stream is dropped after the read timeout"
            );
        }

        let mut honest = open(&node, flooder).await;
        let request = BlobRequest {
            blob_id: BlobId::from([1; 32]),
            context_id: ContextId::from([2; 32]),
            auth: None,
        };
        honest
            .send(Message::new(serde_json::to_vec(&request).expect("encode")))
            .await
            .expect("send request");
        let reply = tokio::time::timeout(SETTLE, honest.next())
            .await
            .expect("the node answers")
            .expect("a frame")
            .expect("a readable frame");
        let response: BlobResponse = serde_json::from_slice(&reply.data).expect("a response");
        assert!(!response.found, "an unknown context's blob is not served");
    }
}
