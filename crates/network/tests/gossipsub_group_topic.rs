//! Gossipsub delivery between two Calimero swarms on a `group/<hex>` topic (same shape as local group governance).
//!
//! Exercises real libp2p + production [`calimero_network::behaviour::Behaviour`]. End-to-end store convergence for
//! `SignedGroupOpV1` remains in `calimero-context` (`tests/local_group_governance_convergence.rs`).

use core::time::Duration;

use calimero_network::behaviour::{Behaviour, BehaviourEvent};
use calimero_network_primitives::config::{
    AutonatConfig, BootstrapConfig, BootstrapNodes, DiscoveryConfig, NetworkConfig, RelayConfig,
    RendezvousConfig, SwarmConfig,
};
use libp2p::gossipsub::{Event as GossipsubEvent, IdentTopic, PublishError};
use libp2p::identity::Keypair;
use libp2p::swarm::SwarmEvent;
use futures_util::StreamExt;
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

#[tokio::test]
async fn two_swarms_deliver_payload_on_group_topic() {
    let group_id = [0x42u8; 32];
    let topic = IdentTopic::new(format!("group/{}", hex::encode(group_id)));

    let listener_a = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port_a = listener_a.local_addr().unwrap().port();
    drop(listener_a);

    let listener_b = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port_b = listener_b.local_addr().unwrap().port();
    drop(listener_b);

    let addr_a: Multiaddr = format!("/ip4/127.0.0.1/tcp/{port_a}").parse().unwrap();
    let addr_b: Multiaddr = format!("/ip4/127.0.0.1/tcp/{port_b}").parse().unwrap();

    let kp_a = Keypair::generate_ed25519();
    let kp_b = Keypair::generate_ed25519();

    let mut swarm_a = Behaviour::build_swarm(&network_config(kp_a, addr_a)).unwrap();
    let mut swarm_b = Behaviour::build_swarm(&network_config(kp_b, addr_b)).unwrap();

    swarm_a
        .behaviour_mut()
        .gossipsub
        .subscribe(&topic)
        .expect("subscribe A");
    swarm_b
        .behaviour_mut()
        .gossipsub
        .subscribe(&topic)
        .expect("subscribe B");

    // `SwarmExt::connect` dials using `other.external_addresses()`; those are empty unless we
    // register our listen addresses (see libp2p-swarm-test docs).
    async fn prime_listen_external(swarm: &mut Swarm<Behaviour>) {
        loop {
            let first: Vec<Multiaddr> = swarm.listeners().cloned().collect();
            if let Some(addr) = first.into_iter().next() {
                swarm.add_external_address(addr);
                return;
            }
            swarm.next().await.expect("swarm event");
        }
    }
    prime_listen_external(&mut swarm_a).await;
    prime_listen_external(&mut swarm_b).await;

    swarm_a.connect(&mut swarm_b).await;

    let topic_hash = topic.hash();
    let topic_hash_cmp = topic_hash.clone();
    let payload = b"hello-signed-group-op-payload".to_vec();

    // Mesh membership is asynchronous; we must **drive** the swarm between publish attempts so
    // gossipsub/identify can add the remote peer (sleep alone is not enough).
    let driver_a = tokio::spawn(async move {
        for attempt in 0..200u32 {
            match swarm_a
                .behaviour_mut()
                .gossipsub
                .publish(topic_hash.clone(), payload.clone())
            {
                Ok(_) => break,
                Err(PublishError::NoPeersSubscribedToTopic) => {
                    swarm_a.next().await.expect("swarm_a event");
                    if attempt == 199 {
                        panic!("gossipsub mesh never became ready for publish");
                    }
                }
                Err(e) => panic!("publish from A: {e:?}"),
            }
        }
        swarm_a.loop_on_next().await;
    });

    let data = timeout(
        Duration::from_secs(30),
        swarm_b.wait(|e| {
            if let SwarmEvent::Behaviour(BehaviourEvent::Gossipsub(GossipsubEvent::Message {
                message,
                ..
            })) = e
            {
                if message.topic == topic_hash_cmp {
                    return Some(message.data.clone());
                }
            }
            None
        }),
    )
    .await
    .expect("test timed out waiting for gossip message");

    driver_a.abort();

    assert_eq!(data, b"hello-signed-group-op-payload");
}
