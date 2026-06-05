use core::time::Duration;

use calimero_network_primitives::config::{
    NetworkConfig, GOSSIPSUB_MESH_N, GOSSIPSUB_MESH_N_HIGH, GOSSIPSUB_MESH_N_LOW,
    GOSSIPSUB_MESH_OUTBOUND_MIN,
};
use calimero_network_primitives::specialized_node_invite::{
    SpecializedNodeInviteCodec, CALIMERO_SPECIALIZED_NODE_INVITE_PROTOCOL,
};
use eyre::WrapErr;
use libp2p::request_response::{self, ProtocolSupport};
use libp2p::swarm::behaviour::toggle::Toggle;
use libp2p::swarm::{NetworkBehaviour, Swarm};
use libp2p::{
    dcutr, gossipsub, identify, kad, mdns, noise, ping, relay, rendezvous, tcp, tls, yamux,
    StreamProtocol, SwarmBuilder,
};
use multiaddr::Protocol;
use tracing::warn;

use crate::autonat;

const PROTOCOL_VERSION: &str = concat!("/", env!("CARGO_PKG_NAME"), "/", env!("CARGO_PKG_VERSION"));
const CALIMERO_KAD_PROTO_NAME: StreamProtocol = StreamProtocol::new("/calimero/kad/1.0.0");

#[expect(
    missing_debug_implementations,
    reason = "Swarm behaviours don't implement Debug"
)]
#[derive(NetworkBehaviour)]
pub struct Behaviour {
    pub autonat: autonat::Behaviour,
    pub dcutr: dcutr::Behaviour,
    pub gossipsub: gossipsub::Behaviour,
    pub identify: identify::Behaviour,
    pub kad: kad::Behaviour<kad::store::MemoryStore>,
    pub mdns: Toggle<mdns::tokio::Behaviour>,
    pub ping: ping::Behaviour,
    pub relay: relay::client::Behaviour,
    pub rendezvous: rendezvous::client::Behaviour,
    pub stream: libp2p_stream::Behaviour,
    pub specialized_node_invite: request_response::Behaviour<SpecializedNodeInviteCodec>,
}

impl Behaviour {
    pub fn build_swarm(config: &NetworkConfig) -> eyre::Result<Swarm<Self>> {
        let peer_id = config.identity.public().to_peer_id();

        let bootstrap_peers = {
            let mut peers = vec![];

            for mut addr in config.bootstrap.nodes.list.iter().cloned() {
                let Some(Protocol::P2p(peer_id)) = addr.pop() else {
                    eyre::bail!("Failed to parse peer id from addr {:?}", addr);
                };

                peers.push((peer_id, addr));
            }

            peers
        };

        let mut swarm = SwarmBuilder::with_existing_identity(config.identity.clone())
            .with_tokio()
            .with_tcp(
                tcp::Config::default(),
                (tls::Config::new, noise::Config::new),
                yamux::Config::default,
            )?
            .with_quic()
            .with_relay_client(noise::Config::new, yamux::Config::default)?
            .with_behaviour(|key, relay_behaviour| {
                let mut behaviour = Self {
                    autonat: {
                        autonat::Behaviour::new(
                            autonat::Config::default()
                                .with_max_candidates(config.discovery.autonat.max_candidates)
                                .with_probe_interval(config.discovery.autonat.probe_interval),
                        )
                    },
                    dcutr: dcutr::Behaviour::new(peer_id),
                    identify: identify::Behaviour::new(
                        identify::Config::new(PROTOCOL_VERSION.to_owned(), key.public())
                            .with_push_listen_addr_updates(true),
                    ),
                    mdns: config
                        .discovery
                        .mdns
                        .then_some(())
                        .map(|()| mdns::Behaviour::new(mdns::Config::default(), peer_id))
                        .transpose()?
                        .into(),
                    kad: {
                        let kad_config = kad::Config::new(CALIMERO_KAD_PROTO_NAME);

                        let mut kad = kad::Behaviour::with_config(
                            peer_id,
                            kad::store::MemoryStore::new(peer_id),
                            kad_config,
                        );

                        for (peer_id, addr) in bootstrap_peers {
                            let _ = kad.add_address(&peer_id, addr);
                        }

                        if let Err(err) = kad.bootstrap() {
                            warn!(%err, "Failed to bootstrap Kademlia");
                        }

                        kad
                    },
                    gossipsub: gossipsub::Behaviour::new(
                        gossipsub::MessageAuthenticity::Signed(key.clone()),
                        // Defaults assume larger swarms. Match the water
                        // marks to Calimero's 2–20 peer clusters so a 3-node
                        // deployment sits at a stable mesh size instead of
                        // the heartbeat path logging `Mesh low` every second
                        // and re-running `get_random_peers` for no
                        // candidates. Topic admission is gated by namespace
                        // membership at the governance layer, so the
                        // permissionless backoff defaults are not needed.
                        //
                        // `flood_publish` fans `publish()` out to every
                        // subscribed peer (not just mesh peers). For
                        // Calimero's small (dozens-of-members) topics this
                        // is cheap and removes the cold-start window where
                        // the mesh isn't formed yet and `broadcast()` would
                        // otherwise drop the delta (issues #2122, #2236).
                        //
                        // Security note: bypassing the mesh also bypasses
                        // gossipsub's per-peer scoring and prune-backoff
                        // on the publish path. That is acceptable here
                        // because every Calimero topic is admission-gated
                        // by signed governance membership — a non-member
                        // peer can subscribe at the transport but their
                        // forwarded/published messages are rejected at the
                        // governance/cryptographic layer (`state_delta`
                        // and `governance_broadcast` validators) before
                        // they can influence application state. Scoring-
                        // based abuse mitigation is therefore not load-
                        // bearing on this code path; the governance layer
                        // is.
                        //
                        // Note on metadata exposure to passive
                        // subscribers: `flood_publish` does not change
                        // what envelope fields a non-member subscriber
                        // can observe. State-delta artifacts are
                        // encrypted with the namespace SharedKey
                        // (`NodeClient::broadcast` in
                        // `crates/node/primitives/src/client.rs`), but
                        // the surrounding `BroadcastMessage` envelope
                        // (context_id, author_id, dag_heads, root_hash,
                        // governance metadata) is plaintext borsh. A
                        // mesh-forwarded message has the same envelope
                        // properties; `flood_publish` just makes
                        // delivery deterministic rather than mesh-
                        // timing-dependent. If the envelope-metadata
                        // exposure ever becomes a threat, the fix is
                        // either envelope encryption or admission-gated
                        // subscription — neither is in scope here.
                        gossipsub::ConfigBuilder::default()
                            .mesh_n_low(GOSSIPSUB_MESH_N_LOW)
                            .mesh_n(GOSSIPSUB_MESH_N)
                            .mesh_n_high(GOSSIPSUB_MESH_N_HIGH)
                            .mesh_outbound_min(GOSSIPSUB_MESH_OUTBOUND_MIN)
                            .flood_publish(true)
                            .unsubscribe_backoff(1)
                            .prune_backoff(Duration::from_secs(1))
                            .build()
                            .map_err(|e| eyre::eyre!("invalid gossipsub config: {e}"))?,
                    )?,
                    ping: ping::Behaviour::default(),
                    rendezvous: rendezvous::client::Behaviour::new(key.clone()),
                    relay: relay_behaviour,
                    stream: libp2p_stream::Behaviour::new(),
                    specialized_node_invite: request_response::Behaviour::new(
                        [(
                            CALIMERO_SPECIALIZED_NODE_INVITE_PROTOCOL,
                            ProtocolSupport::Full,
                        )],
                        request_response::Config::default(),
                    ),
                };

                // Enable gossipsub application-specific peer scoring
                // (#2513). The score is derived solely from our verified
                // membership knowledge — the node pushes a positive
                // app-specific score for peers it has authenticated as
                // namespace/group members (anchors highest), pushed from
                // `observe_peer_identity`. Mesh maintenance (graft/prune,
                // opportunistic grafting) then prefers verified members
                // for mesh slots and squeezes out an unverified
                // non-forwarder.
                //
                // All non-app weights are zeroed (see
                // `membership_peer_score_config`), so an unknown peer
                // sits at exactly 0 — above every gating threshold — and
                // is never graylisted for being unverified. That's the
                // cold-start-safe form: boost the verified, never punish
                // the unknown, and keep traffic-dependent penalties off.
                let (score_params, score_thresholds) = membership_peer_score_config();
                behaviour
                    .gossipsub
                    .with_peer_score(score_params, score_thresholds)
                    .map_err(|e| eyre::eyre!("failed to enable gossipsub peer scoring: {e}"))?;

                Ok(behaviour)
            })?
            .with_swarm_config(|cfg| cfg.with_idle_connection_timeout(Duration::from_secs(30)))
            .build();

        for addr in &config.swarm.listen {
            let _ignored = swarm
                .listen_on(addr.clone())
                .wrap_err_with(|| format!("failed to listen on '{addr}'"))?;
        }

        Ok(swarm)
    }
}

/// Peer-score configuration for membership-biased mesh maintenance
/// (#2513). Only the application-specific score contributes: every
/// traffic- and behaviour-dependent weight is zeroed, so the score
/// reflects *our verified-membership knowledge* and nothing else. In
/// particular per-topic `mesh_message_deliveries` scoring stays off (no
/// topic params), which avoids false-penalizing honest peers on quiet /
/// low-traffic context topics.
///
/// Thresholds are the gossipsub defaults — every gating cutoff is ≤ 0
/// (gossip −10, publish −50, graylist −80) — so an unknown peer at score
/// 0 sits above all of them and is never suppressed for being unverified.
/// The node only ever pushes non-negative app scores in this
/// configuration (boost members, leave unknown at 0), so peer scoring can
/// only *prefer*, never exclude.
fn membership_peer_score_config() -> (gossipsub::PeerScoreParams, gossipsub::PeerScoreThresholds) {
    let params = gossipsub::PeerScoreParams {
        // The pushed app score is used as-is (the node tiers it: anchors
        // highest, plain members positive, unknown left at 0).
        app_specific_weight: 1.0,
        // Disable every non-membership signal.
        topics: std::collections::HashMap::new(),
        ip_colocation_factor_weight: 0.0,
        behaviour_penalty_weight: 0.0,
        slow_peer_weight: 0.0,
        ..Default::default()
    };
    (params, gossipsub::PeerScoreThresholds::default())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn membership_score_config_is_valid_and_cold_start_safe() {
        let (params, thresholds) = membership_peer_score_config();
        // `with_peer_score` rejects invalid params/thresholds at build —
        // assert directly so a bad edit fails here, not deep in swarm init.
        params.validate().expect("score params must validate");
        thresholds
            .validate()
            .expect("score thresholds must validate");

        // The load-bearing invariant: an unknown peer (no app score
        // pushed) sits at exactly 0, which must be at or above every
        // gating cutoff so it is never gossip-/publish-suppressed or
        // graylisted for merely being unverified.
        assert!(0.0 >= thresholds.gossip_threshold);
        assert!(0.0 >= thresholds.publish_threshold);
        assert!(0.0 >= thresholds.graylist_threshold);

        // Only the app-specific signal contributes — no per-topic traffic
        // scoring and no behaviour/IP/slow penalties that could
        // false-penalize an honest peer on a quiet topic.
        assert!(params.topics.is_empty());
        assert_eq!(params.ip_colocation_factor_weight, 0.0);
        assert_eq!(params.behaviour_penalty_weight, 0.0);
        assert_eq!(params.slow_peer_weight, 0.0);
    }
}
