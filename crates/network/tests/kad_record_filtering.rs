//! Locks the Kademlia record-filtering contract the blob-provider DHT flow
//! depends on.
//!
//! Production configures `StoreInserts::FilterBoth` (see `behaviour.rs`), so a
//! record another peer replicates to us is **not** written to our store blind:
//! it surfaces as an `InboundRequest::PutRecord` carrying the record, and the
//! kad handler validates its shape before storing it explicitly. This test
//! reproduces that end-to-end with two real swarms — the replicated record
//! stays absent from the receiver's store until the handler stores it. A
//! future libp2p change that silently reverted to auto-storing, or dropped the
//! record from the event, would fail here instead of quietly breaking blob
//! discovery in production.

use std::time::Duration;

use libp2p::identity::Keypair;
use libp2p::kad::store::{MemoryStore, RecordStore};
use libp2p::kad::{self, InboundRequest, Mode, Quorum, Record, RecordKey, StoreInserts};
use libp2p::swarm::{NetworkBehaviour, SwarmEvent};
use libp2p::{PeerId, Swarm};
use libp2p_swarm_test::SwarmExt;

#[derive(NetworkBehaviour)]
struct KadOnly {
    kad: kad::Behaviour<MemoryStore>,
}

impl KadOnly {
    fn new(k: Keypair) -> Self {
        let peer_id = k.public().to_peer_id();
        let mut config = kad::Config::new(kad::PROTOCOL_NAME);
        // Mirror production: never auto-store an inbound record.
        let _ = config.set_record_filtering(StoreInserts::FilterBoth);
        Self {
            kad: kad::Behaviour::with_config(peer_id, MemoryStore::new(peer_id), config),
        }
    }
}

/// A record shaped like the blob announcement `AnnounceBlob` produces: a
/// 64-byte key (context id + blob id) and a value of a peer id + 8-byte size.
fn blob_record() -> Record {
    let value = [
        PeerId::random().to_bytes().as_slice(),
        &4096_u64.to_le_bytes(),
    ]
    .concat();
    Record::new(RecordKey::new(&vec![9_u8; 64]), value)
}

#[tokio::test]
async fn filtered_inbound_record_is_not_stored_until_handler_stores_it() {
    let mut announcer = Swarm::new_ephemeral_tokio(KadOnly::new);
    let (announcer_addr, _) = announcer.listen().with_memory_addr_external().await;
    let announcer_id = *announcer.local_peer_id();

    let mut receiver = Swarm::new_ephemeral_tokio(KadOnly::new);
    let (receiver_addr, _) = receiver.listen().with_memory_addr_external().await;
    let receiver_id = *receiver.local_peer_id();

    // Server mode so the receiver accepts inbound records; seed each side's
    // routing table so the put actually routes between the two peers.
    announcer.behaviour_mut().kad.set_mode(Some(Mode::Server));
    receiver.behaviour_mut().kad.set_mode(Some(Mode::Server));
    let _ = announcer
        .behaviour_mut()
        .kad
        .add_address(&receiver_id, receiver_addr);
    let _ = receiver
        .behaviour_mut()
        .kad
        .add_address(&announcer_id, announcer_addr);

    announcer.connect(&mut receiver).await;

    let record = blob_record();
    let key = record.key.clone();
    let _query = announcer
        .behaviour_mut()
        .kad
        .put_record(record, Quorum::One)
        .expect("put_record should be accepted");

    // Drive both swarms until the receiver surfaces the replicated record.
    // Polling the announcer keeps its side of the connection making progress.
    // Bounded so a routing regression fails loudly instead of hanging forever.
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            let event = tokio::select! {
                _ = announcer.next_swarm_event() => continue,
                event = receiver.next_swarm_event() => event,
            };

            if let SwarmEvent::Behaviour(KadOnlyEvent::Kad(kad::Event::InboundRequest {
                request:
                    InboundRequest::PutRecord {
                        record: Some(inbound),
                        ..
                    },
            })) = event
            {
                assert_eq!(inbound.key, key, "received the announced record");
                // FilterBoth must have suppressed the blind auto-store.
                assert!(
                    receiver.behaviour_mut().kad.store_mut().get(&key).is_none(),
                    "record must not be stored before the handler validates it",
                );
                // The handler's explicit store is what makes it retrievable.
                receiver
                    .behaviour_mut()
                    .kad
                    .store_mut()
                    .put(inbound)
                    .expect("store put should succeed");
                break;
            }
        }
    })
    .await
    .expect("timed out waiting for the receiver to surface the replicated record");

    assert!(
        receiver.behaviour_mut().kad.store_mut().get(&key).is_some(),
        "record is retrievable only after the handler stores it",
    );
}
