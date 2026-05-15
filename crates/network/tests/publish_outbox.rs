//! Integration smoke for the publisher-side cold-start outbox.
//!
//! Two `Behaviour` swarms; the publisher side queues when no
//! subscriber is known and we observe via swarm events that the
//! payload still reaches the subscriber once it joins the topic. This
//! complements the unit tests in `crates/network/src/publish_outbox.rs`,
//! which cover the queue's enqueue / drain / TTL / cap semantics in
//! isolation.

use core::time::Duration;

use calimero_network::behaviour::{Behaviour, BehaviourEvent};
use calimero_network::publish_outbox::{PublishOutbox, OUTBOX_MAX_PER_TOPIC, OUTBOX_TTL};
use calimero_network_primitives::config::{
    AutonatConfig, BootstrapConfig, BootstrapNodes, DiscoveryConfig, NetworkConfig, RelayConfig,
    RendezvousConfig, SwarmConfig,
};
use futures_util::StreamExt;
use libp2p::gossipsub::{Event as GossipsubEvent, IdentTopic, PublishError, TopicHash};
use libp2p::identity::Keypair;
use libp2p::swarm::SwarmEvent;
use libp2p::{Multiaddr, Swarm};
use libp2p_swarm_test::SwarmExt;
use tokio::time::timeout;

fn network_config(keypair: Keypair, listen: Multiaddr) -> NetworkConfig {
    NetworkConfig::new(
        keypair,
        SwarmConfig::new(vec![listen]),
        BootstrapConfig::new(BootstrapNodes::new(vec![])),
        DiscoveryConfig::new(
            false,
            false,
            RendezvousConfig::default(),
            RelayConfig::default(),
            AutonatConfig::new(5, Duration::from_secs(10)),
        ),
    )
}

async fn prime_listen_external(swarm: &mut Swarm<Behaviour>) {
    loop {
        let listeners: Vec<Multiaddr> = swarm.listeners().cloned().collect();
        if let Some(addr) = listeners.into_iter().next() {
            swarm.add_external_address(addr);
            return;
        }
        swarm.next().await.expect("swarm event");
    }
}

async fn allocate_listen() -> Multiaddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let port = listener.local_addr().expect("local_addr").port();
    drop(listener);
    format!("/ip4/127.0.0.1/tcp/{port}").parse().expect("parse")
}

/// End-to-end: publisher attempts to publish before any subscriber
/// is known (NoPeersSubscribedToTopic), the outbox queues the
/// payload, the subscriber arrives, and the drained re-publish
/// reaches it.
#[tokio::test]
async fn outbox_drains_payload_after_subscribed_event() {
    let topic = IdentTopic::new("publish-outbox-test/group");
    let topic_hash: TopicHash = topic.hash();
    let payload = b"cold-start-payload".to_vec();

    let addr_a = allocate_listen().await;
    let addr_b = allocate_listen().await;

    let kp_a = Keypair::generate_ed25519();
    let kp_b = Keypair::generate_ed25519();

    let mut swarm_a = Behaviour::build_swarm(&network_config(kp_a, addr_a)).expect("swarm A");
    let mut swarm_b = Behaviour::build_swarm(&network_config(kp_b, addr_b)).expect("swarm B");

    prime_listen_external(&mut swarm_a).await;
    prime_listen_external(&mut swarm_b).await;
    swarm_a.connect(&mut swarm_b).await;

    let mut outbox = PublishOutbox::new();

    // Drive both swarms briefly and try to publish. With no subscribers,
    // gossipsub returns NoPeersSubscribedToTopic — exactly the
    // cold-start case the outbox catches.
    let drained_payload = match swarm_a
        .behaviour_mut()
        .gossipsub
        .publish(topic_hash.clone(), payload.clone())
    {
        Err(PublishError::NoPeersSubscribedToTopic) => {
            outbox.enqueue(topic_hash.clone(), payload.clone());
            payload.clone()
        }
        Ok(_) => panic!("publish unexpectedly succeeded before subscriber"),
        Err(e) => panic!("unexpected publish error: {e:?}"),
    };

    // B subscribes; on A's side this produces an Event::Subscribed
    // for `topic_hash`. The production handler calls
    // `drain_publish_outbox` here — we mirror that by taking the
    // queue and re-publishing once we see the Subscribed event.
    swarm_b
        .behaviour_mut()
        .gossipsub
        .subscribe(&topic)
        .expect("subscribe B");

    // Drive A's swarm until we see Subscribed for our topic; then
    // re-publish the drained payload. Wrap in a short timeout so
    // the test fails fast on a regression.
    let drain_topic = topic_hash.clone();
    let drain_then_publish = tokio::spawn(async move {
        loop {
            let event = swarm_a.next().await.expect("swarm A event");
            if let SwarmEvent::Behaviour(BehaviourEvent::Gossipsub(GossipsubEvent::Subscribed {
                topic: subscribed_topic,
                ..
            })) = &event
            {
                if subscribed_topic == &drain_topic {
                    let entries = outbox.take_drainable(&drain_topic);
                    for entry in entries {
                        assert_eq!(entry.data, drained_payload);
                        swarm_a
                            .behaviour_mut()
                            .gossipsub
                            .publish(drain_topic.clone(), entry.data.clone())
                            .expect("re-publish after subscribed should succeed");
                    }
                    break;
                }
            }
        }
        swarm_a.loop_on_next().await;
    });

    let received = timeout(
        Duration::from_secs(15),
        swarm_b.wait(|e| {
            if let SwarmEvent::Behaviour(BehaviourEvent::Gossipsub(GossipsubEvent::Message {
                message,
                ..
            })) = e
            {
                if message.topic == topic_hash {
                    return Some(message.data);
                }
            }
            None
        }),
    )
    .await
    .expect("timed out waiting for queued payload");

    drain_then_publish.abort();
    assert_eq!(received, b"cold-start-payload");
}

/// Smoke that the public PublishOutbox API enforces the per-topic cap
/// the production handler relies on. The unit tests in
/// `publish_outbox::tests` cover this internally; this one asserts the
/// behaviour through the crate's public surface so a refactor that
/// changes the cap silently is caught.
#[test]
fn cap_visible_through_public_api() {
    let mut outbox = PublishOutbox::new();
    let t = TopicHash::from_raw("cap-test");
    for i in 0..(OUTBOX_MAX_PER_TOPIC + 5) {
        outbox.enqueue(t.clone(), vec![i as u8]);
    }
    let drained = outbox.take_drainable(&t);
    assert_eq!(drained.len(), OUTBOX_MAX_PER_TOPIC);
    assert!(OUTBOX_TTL >= Duration::from_secs(1));
}
