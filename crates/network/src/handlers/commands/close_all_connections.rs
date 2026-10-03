//! Close every connection at shutdown, while the runtime can still say so.
//!
//! When a node stops without closing its connections, a QUIC peer is never
//! told. A QUIC connection ends on the remote side only when this side sends a
//! close packet. The task that would send it runs on the same arbiter as the
//! network actor, so stopping the actix system cancels it, and the peer keeps
//! the dead connection until its idle timeout (10s). TCP does not have this
//! problem, because the kernel ends the socket when the process exits.
//!
//! The dead connection matters when the node restarts inside that window,
//! which is the normal case for a restart. Its first new connection reaches a
//! peer that still counts the old one, so gossipsub treats it as a second
//! connection to a known peer and does not send its subscriptions. The
//! restarted node sends its own, so the peer knows what the restarted node
//! follows but not the reverse. The restarted node can then find nobody to sync
//! with or fetch a blob from on any topic they share, until
//! [`subscription_repair`](crate::subscription_repair) notices it, which needs
//! a delivery on each topic.
//!
//! Closing every connection here, before the system is stopped, makes the peer
//! drop us at once. The restart then reaches it as a new peer, and gossipsub
//! sends subscriptions both ways.

use actix::{Context, Handler, Message, Response};
use calimero_network_primitives::messages::CloseAllConnections;
use libp2p::connection_limits::ConnectionLimits;
use libp2p::PeerId;
use tokio::sync::oneshot;
use tracing::info;

use crate::NetworkManager;

impl NetworkManager {
    /// Refuse every new connection and start closing the ones that are open.
    ///
    /// Refusing comes first. A peer whose connection we close dials us back
    /// straight away (its `ConnectionClosed` handler re-dials direct
    /// addresses), and our own recovery would dial it too. Either connection
    /// would be just as stale once the process exits.
    pub(crate) fn begin_closing_all_connections(&mut self) {
        if self.closing_all_connections {
            return;
        }
        self.closing_all_connections = true;

        *self.swarm.behaviour_mut().connection_limits.limits_mut() = ConnectionLimits::default()
            .with_max_pending_incoming(Some(0))
            .with_max_pending_outgoing(Some(0))
            .with_max_established(Some(0));

        let peers: Vec<PeerId> = self.swarm.connected_peers().copied().collect();
        info!(
            peers = peers.len(),
            "closing every peer connection before shutdown"
        );
        for peer_id in peers {
            // `Err` only means the peer is no longer connected, which is the
            // goal anyway.
            let _closing: Result<(), ()> = self.swarm.disconnect_peer_id(peer_id);
        }
    }

    /// Answer every `CloseAllConnections` caller once no connection is left.
    pub(crate) fn resolve_close_all_if_done(&mut self) {
        if self.swarm.network_info().num_peers() > 0 {
            return;
        }
        for waiter in self.pending_close_all.drain(..) {
            let _caller_gone: Result<(), ()> = waiter.send(());
        }
    }
}

impl Handler<CloseAllConnections> for NetworkManager {
    type Result = Response<<CloseAllConnections as Message>::Result>;

    fn handle(&mut self, _msg: CloseAllConnections, _ctx: &mut Context<Self>) -> Self::Result {
        self.begin_closing_all_connections();

        if self.swarm.network_info().num_peers() == 0 {
            return Response::reply(());
        }

        let (sender, receiver) = oneshot::channel();
        self.pending_close_all.push(sender);

        Response::fut(async move {
            // A dropped sender means the actor itself went away, so it holds
            // no connections either.
            let _closed: Result<(), _> = receiver.await;
        })
    }
}

#[cfg(test)]
mod tests {
    use core::time::Duration;
    use std::collections::BTreeSet;
    use std::net::UdpSocket;
    use std::sync::Arc;
    use std::thread;

    use actix::{Actor, Addr, System};
    use calimero_network_primitives::config::{
        AutonatConfig, BootstrapConfig, BootstrapNodes, DiscoveryConfig, NetworkConfig,
        RelayConfig, RendezvousConfig, SwarmConfig,
    };
    use calimero_network_primitives::messages::{
        NetworkEvent, NetworkEventDispatcher, SubscribedPeers,
    };
    use futures_util::StreamExt;
    use libp2p::gossipsub::{IdentTopic, TopicHash};
    use libp2p::identity::Keypair;
    use libp2p::{Multiaddr, Swarm};
    use prometheus_client::registry::Registry;
    use tokio::time::{sleep, timeout, Instant};

    use super::*;
    use crate::behaviour::Behaviour;

    struct NoopDispatcher;

    impl NetworkEventDispatcher for NoopDispatcher {
        fn dispatch(&self, _event: NetworkEvent) -> bool {
            true
        }
    }

    /// QUIC only. TCP would hide the failure: the kernel sends a FIN when the
    /// socket is dropped, so the peer learns of a TCP close however the node
    /// stops. A QUIC close is a packet, and only a running runtime sends it.
    fn quic_addr(port: u16) -> Multiaddr {
        format!("/ip4/127.0.0.1/udp/{port}/quic-v1")
            .parse()
            .expect("quic multiaddr")
    }

    fn free_udp_port() -> u16 {
        UdpSocket::bind("127.0.0.1:0")
            .and_then(|socket| socket.local_addr())
            .expect("free udp port")
            .port()
    }

    fn network_config(keypair: Keypair, listen: Multiaddr) -> NetworkConfig {
        NetworkConfig::new(
            keypair,
            SwarmConfig::new(vec![listen]),
            BootstrapConfig::new(BootstrapNodes::new(vec![])),
            DiscoveryConfig::new(
                false,
                false,
                Vec::new(),
                RendezvousConfig::default(),
                RelayConfig::default(),
                AutonatConfig::new(5, Duration::from_secs(10)),
            ),
        )
    }

    fn subscribers(swarm: &Swarm<Behaviour>, topic: &TopicHash) -> Vec<PeerId> {
        swarm
            .behaviour()
            .gossipsub
            .all_peers()
            .filter_map(|(peer, topics)| topics.contains(&topic).then_some(*peer))
            .collect()
    }

    /// Runs `NetworkManager` as an actor in its own actix system, as merod
    /// does, so stopping that system cancels the runtime the connections run
    /// on, which is how a node stops for real.
    fn start_manager_in_own_system(
        keypair: Keypair,
        listen: Multiaddr,
        dial: Multiaddr,
        topic: IdentTopic,
    ) -> (Addr<NetworkManager>, System, thread::JoinHandle<()>) {
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        let handle = thread::spawn(move || {
            let system = System::new();
            system.block_on(async move {
                let mut registry = Registry::default();
                let mut manager = NetworkManager::new(
                    &network_config(keypair, listen),
                    Arc::new(NoopDispatcher),
                    &mut registry,
                    BTreeSet::new(),
                    None,
                )
                .await
                .expect("manager builds");
                let _subscribed: bool = manager
                    .swarm
                    .behaviour_mut()
                    .gossipsub
                    .subscribe(&topic)
                    .expect("manager subscribes");
                manager.swarm.dial(dial).expect("manager dials the peer");
                ready_tx
                    .send((manager.start(), System::current()))
                    .expect("test still waiting");
            });
            system.run().expect("actix system runs");
        });
        let (addr, system) = ready_rx.recv().expect("manager started");
        (addr, system, handle)
    }

    /// A restart where the node reconnects before its peer has dropped the old
    /// connection, which is what a restart does unless the old connection was
    /// closed.
    ///
    /// The peer then sees the new connection as a second connection to a peer
    /// it already knows, and gossipsub does not send it the peer's
    /// subscriptions. The restarted node has nobody to sync with or fetch a
    /// blob from on the topic they share. This is what a node restarted in CI
    /// showed: "connected to peers but none subscribe to the context or
    /// namespace topic", and blob fetches that found no peer.
    #[tokio::test]
    async fn a_restarted_node_learns_its_peers_subscriptions() {
        let topic = IdentTopic::new(format!("ns/{}", hex::encode([0x42; 32])));
        let topic_hash = topic.hash();

        let mut peer = Behaviour::build_swarm(&network_config(
            Keypair::generate_ed25519(),
            quic_addr(free_udp_port()),
        ))
        .expect("peer swarm");
        let _subscribed: bool = peer
            .behaviour_mut()
            .gossipsub
            .subscribe(&topic)
            .expect("peer subscribes");
        let peer_id = *peer.local_peer_id();
        let peer_addr = timeout(Duration::from_secs(10), async {
            loop {
                if let Some(addr) = peer.listeners().next().cloned() {
                    return addr;
                }
                drop(peer.select_next_some().await);
            }
        })
        .await
        .expect("peer listens");

        // The identity and port that survive the restart, as on a real node.
        let keypair = Keypair::generate_ed25519();
        let node_id = keypair.public().to_peer_id();
        let node_addr = quic_addr(free_udp_port());

        let (manager, system, handle) = start_manager_in_own_system(
            keypair.clone(),
            node_addr.clone(),
            peer_addr.clone(),
            topic.clone(),
        );

        // Before the restart, each side knows the other follows the topic.
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            let node_sees_peer = manager
                .send(SubscribedPeers(topic_hash.clone()))
                .await
                .expect("manager answers")
                .contains(&peer_id);
            if node_sees_peer && subscribers(&peer, &topic_hash).contains(&node_id) {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "the two nodes never learned each other's subscriptions"
            );
            tokio::select! {
                _ = peer.select_next_some() => {}
                () = sleep(Duration::from_millis(100)) => {}
            }
        }

        // The node shuts down as merod does: close connections, then stop
        // the system.
        timeout(Duration::from_secs(5), manager.send(CloseAllConnections))
            .await
            .expect("closing connections finishes")
            .expect("manager answers");
        system.stop();
        tokio::task::spawn_blocking(move || handle.join())
            .await
            .expect("join task")
            .expect("manager thread exits cleanly");

        // The restart: same identity, same port, and it dials the peer at
        // once, as merod's peer cache does on startup.
        let mut restarted =
            Behaviour::build_swarm(&network_config(keypair, node_addr)).expect("restarted swarm");
        let _subscribed: bool = restarted
            .behaviour_mut()
            .gossipsub
            .subscribe(&topic)
            .expect("restarted node subscribes");
        restarted.dial(peer_addr).expect("restarted node dials");

        // Well inside QUIC's 10s idle timeout, so a stale connection on the
        // peer is still there for the whole wait.
        let deadline = Instant::now() + Duration::from_secs(6);
        while !subscribers(&restarted, &topic_hash).contains(&peer_id) {
            assert!(
                Instant::now() < deadline,
                "the restarted node never learned that its peer follows the topic: the peer \
                 still held the pre-restart connection ({} established), took the new one as \
                 a second connection, and did not send its subscriptions",
                peer.network_info().connection_counters().num_established(),
            );
            tokio::select! {
                _ = peer.select_next_some() => {}
                _ = restarted.select_next_some() => {}
                () = sleep(Duration::from_millis(100)) => {}
            }
        }

        assert_eq!(
            peer.network_info().connection_counters().num_established(),
            1,
            "only the restarted node's new connection is left on the peer",
        );
    }
}
