use std::time::Duration;

use libp2p::identity::Keypair;
use libp2p::kad::store::MemoryStore;
use libp2p::swarm::{NetworkBehaviour, SwarmEvent};
use libp2p::{identify, kad, Swarm};
use libp2p_swarm_test::SwarmExt;

use calimero_network::autonat;
use TestBehaviourEvent::*;

#[derive(NetworkBehaviour)]
struct TestBehaviour {
    autonat: autonat::Behaviour,
    identify: identify::Behaviour,
    kad: kad::Behaviour<MemoryStore>,
}

impl TestBehaviour {
    fn new(k: Keypair) -> Self {
        let local_peer_id = k.public().to_peer_id();

        Self {
            autonat: autonat::Behaviour::new(
                autonat::Config::default()
                    .with_max_candidates(5)
                    .with_probe_interval(Duration::from_millis(100)),
            ),
            identify: identify::Behaviour::new(identify::Config::new(
                "/test/1.0.0".to_owned(),
                k.public(),
            )),
            kad: kad::Behaviour::with_config(
                local_peer_id,
                MemoryStore::new(local_peer_id),
                kad::Config::new(kad::PROTOCOL_NAME),
            ),
        }
    }
}

#[tokio::test]
async fn upgrade_to_kad_server_after_autonat_probe() {
    // Create client and server test swarms
    let mut client = Swarm::new_ephemeral_tokio(TestBehaviour::new);
    let _res = client.listen().await;

    // And make server listen with confirmed external address
    let mut server = Swarm::new_ephemeral_tokio(TestBehaviour::new);
    let _res = server.listen().with_tcp_addr_external().await;
    server.behaviour_mut().autonat.enable_server().unwrap();

    client.connect(&mut server).await;

    let _handle = tokio::spawn(server.loop_on_next());

    // Wait for AutoNAT to confirm external address
    client
        .wait(|e| match e {
            SwarmEvent::ExternalAddrConfirmed { .. } => Some(()),
            _ => None,
        })
        .await;

    // Confirmed external address event should change KAD Mode
    let client_kad_mode = client
        .wait(|e| match e {
            SwarmEvent::Behaviour(Kad(kad::Event::ModeChanged { new_mode })) => Some(new_mode),
            _ => None,
        })
        .await;

    // Check if the client swarm is now in KAD server mode
    assert_eq!(client_kad_mode, kad::Mode::Server);
}
