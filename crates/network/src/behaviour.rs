use core::num::{NonZeroU8, NonZeroUsize};
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
    connection_limits, dcutr, gossipsub, identify, kad, mdns, noise, ping, relay, rendezvous, tcp,
    tls, yamux, StreamProtocol, SwarmBuilder,
};
use multiaddr::Protocol;
use tracing::warn;

use crate::autonat;

const PROTOCOL_VERSION: &str = concat!("/", env!("CARGO_PKG_NAME"), "/", env!("CARGO_PKG_VERSION"));
const CALIMERO_KAD_PROTO_NAME: StreamProtocol = StreamProtocol::new("/calimero/kad/1.0.0");

// Connection-count ceilings, enforced by `connection_limits::Behaviour`.
//
// These are a resource-exhaustion backstop, not a tuning knob: Calimero
// clusters are small (2–20 collaborating peers) plus a handful of
// bootstrap/rendezvous/relay links, so the ceilings sit far above any
// healthy steady state and only bite under abuse — e.g. a poisoned peer
// cache trying to dial thousands of stale addresses at startup, or an inbound
// handshake flood. Without them a single bad input can exhaust file
// descriptors and take the node down.
//
// `max_pending_outgoing` is the dial-storm cap: excess concurrent dials are
// denied (not queued) rather than opening an unbounded number of sockets. The
// startup cache redial also caps how many it issues at the source.
const MAX_PENDING_INCOMING: u32 = 128;
const MAX_PENDING_OUTGOING: u32 = 128;
// A peer legitimately holds a few simultaneous connections (TCP + QUIC, plus a
// relayed circuit upgrading to a direct one via DCUtR), so allow headroom
// while still bounding a single misbehaving peer.
const MAX_ESTABLISHED_PER_PEER: u32 = 8;
// Total established ceiling — generous headroom over any real cluster while
// still bounding total open sockets/FDs.
const MAX_ESTABLISHED_TOTAL: u32 = 1024;

// Kademlia record lifetime and refresh.
//
// The only records Calimero puts on the DHT are tiny blob-provider entries
// (peer id + blob size, keyed by context+blob), announced on demand at
// upgrade/admit time — never on a periodic schedule from the app. libp2p's
// 48h default TTL therefore leaves forged or stale entries lingering far
// longer than any real blob handoff needs. Bound the TTL to 12h and have the
// original announcer re-publish every 6h, so a live blob's record is always
// refreshed well before it expires while a one-off (or malicious) entry ages
// out in half a day.
const KAD_RECORD_TTL: Duration = Duration::from_secs(12 * 60 * 60);
const KAD_RECORD_PUBLICATION_INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);
// Cap the wall-clock a single DHT query may burn before it is abandoned, so a
// blob lookup against unresponsive closest-peers can't hang a caller
// indefinitely.
const KAD_QUERY_TIMEOUT: Duration = Duration::from_secs(60);
// Records replicate to at most this many peers. Calimero clusters are small
// (2–20 peers); 10 still saturates a typical cluster with redundant copies of
// each record — a record survives unless all 10 holders drop — while leaving
// headroom below the libp2p default of 20 (K_VALUE). If the peer count
// transiently spikes (e.g. a churny bootstrap phase), replication fan-out and
// the write traffic it drives stay bounded at 10 instead of tracking the
// cluster size upward.
const KAD_REPLICATION_FACTOR: NonZeroUsize = match NonZeroUsize::new(10) {
    Some(factor) => factor,
    None => panic!("KAD_REPLICATION_FACTOR must be non-zero"),
};

// Kademlia in-memory store bounds — the resource ceiling for records this node
// will hold on behalf of the network. Provider records are unused (we announce
// via `put_record`, not `start_providing`), so the provider fields are kept
// minimal. `max_value_bytes` is the load-bearing anti-abuse bound: a valid
// blob-provider value is a peer id plus an 8-byte size (~50 bytes), so 256
// bytes rejects any oversized forged record cheaply. The inbound-record
// validator enforces the same ceiling directly (see the kad handler), so an
// oversized record is dropped in our own code; the store bound is the backstop.
const KAD_MAX_RECORDS: usize = 4096;
pub(crate) const KAD_MAX_VALUE_BYTES: usize = 256;
const KAD_MAX_PROVIDERS_PER_KEY: usize = 20;
const KAD_MAX_PROVIDED_KEYS: usize = 1024;

// Gossipsub message-size ceiling. libp2p's hidden default is 64 KiB, which is
// also the only inbound size guard on the receive path — and small enough that
// a legitimate large state-delta broadcast is silently dropped. Set it
// explicitly to 1 MiB: comfortably above real envelopes+deltas, matched to the
// specialized-node-invite cap, and far below the transport stream limit so it
// never conflicts with yamux framing. `flood_publish` fans each publish out to
// every subscriber, so this doubles as the amplification bound and is kept
// deliberately modest rather than pushed to the stream ceiling.
const GOSSIPSUB_MAX_TRANSMIT_SIZE: usize = 1024 * 1024;

// Inbound concurrency cap for the specialized-node-invite request-response
// protocol. Each request carries a TEE attestation quote whose verification is
// expensive; without a cap a peer can open many concurrent request streams and
// turn cheap bytes into disproportionate CPU. The message size is already
// bounded by the codec; this bounds how many verifications run at once.
const SPECIALIZED_NODE_INVITE_MAX_CONCURRENT_STREAMS: usize = 8;

// Addresses dialed in parallel per dial attempt (libp2p default is 8). Lowering
// it curbs the socket burst when dialing a peer that advertises many
// addresses — relevant at startup when redialing a large peer cache — while
// still trying enough candidates to connect promptly.
const DIAL_CONCURRENCY_FACTOR: u8 = 3;

// Per-connection substream ceiling (libp2p yamux default is 8192). A handful of
// concurrent substreams covers every Calimero protocol on one connection, so a
// far lower cap bounds how much a single peer can allocate against us without
// starving legitimate use.
const YAMUX_MAX_NUM_STREAMS: usize = 256;

// Liveness-probe cadence, pinned rather than left to the libp2p default. The
// ping-failure watchdog (see the ping handler) sizes its force-close threshold
// against a 15s interval / 20s timeout to land inside the sync-recovery budget;
// pinning the values here keeps that math correct even if the library default
// later changes.
const PING_INTERVAL: Duration = Duration::from_secs(15);
const PING_TIMEOUT: Duration = Duration::from_secs(20);

#[expect(
    missing_debug_implementations,
    reason = "Swarm behaviours don't implement Debug"
)]
#[derive(NetworkBehaviour)]
pub struct Behaviour {
    pub autonat: autonat::Behaviour,
    pub connection_limits: connection_limits::Behaviour,
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
                bounded_yamux_config,
            )?
            .with_quic()
            .with_relay_client(noise::Config::new, bounded_yamux_config)?
            .with_behaviour(|key, relay_behaviour| {
                let mut behaviour = Self {
                    autonat: {
                        autonat::Behaviour::new(
                            autonat::Config::default()
                                .with_max_candidates(config.discovery.autonat.max_candidates)
                                .with_probe_interval(config.discovery.autonat.probe_interval),
                        )
                    },
                    connection_limits: connection_limits::Behaviour::new(
                        connection_limits::ConnectionLimits::default()
                            .with_max_pending_incoming(Some(MAX_PENDING_INCOMING))
                            .with_max_pending_outgoing(Some(MAX_PENDING_OUTGOING))
                            .with_max_established_per_peer(Some(MAX_ESTABLISHED_PER_PEER))
                            .with_max_established(Some(MAX_ESTABLISHED_TOTAL)),
                    ),
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
                        let mut kad_config = kad::Config::new(CALIMERO_KAD_PROTO_NAME);
                        // Reject auto-storing inbound records. With filtering
                        // on, a replicated record from another peer is
                        // surfaced as an `InboundRequest::PutRecord` event
                        // instead of being written blind, so the kad handler
                        // can validate its shape and size before it enters our
                        // store. Without this any server-mode node would store
                        // arbitrary forged records for any (context, blob).
                        kad_config.set_record_filtering(kad::StoreInserts::FilterBoth);
                        kad_config.set_record_ttl(Some(KAD_RECORD_TTL));
                        kad_config.set_publication_interval(Some(KAD_RECORD_PUBLICATION_INTERVAL));
                        kad_config.set_query_timeout(KAD_QUERY_TIMEOUT);
                        kad_config.set_replication_factor(KAD_REPLICATION_FACTOR);

                        let store = kad::store::MemoryStore::with_config(
                            peer_id,
                            kad::store::MemoryStoreConfig {
                                max_records: KAD_MAX_RECORDS,
                                max_value_bytes: KAD_MAX_VALUE_BYTES,
                                max_providers_per_key: KAD_MAX_PROVIDERS_PER_KEY,
                                max_provided_keys: KAD_MAX_PROVIDED_KEYS,
                            },
                        );

                        let mut kad = kad::Behaviour::with_config(peer_id, store, kad_config);

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
                            // Set the transmit size explicitly rather than
                            // inheriting the hidden 64 KiB default, which also
                            // silently caps the inbound receive path. See
                            // `GOSSIPSUB_MAX_TRANSMIT_SIZE`.
                            .max_transmit_size(GOSSIPSUB_MAX_TRANSMIT_SIZE)
                            // Anti-churn backoffs (defaults 10s / 60s) are
                            // deliberately cut to the 1s floor. They exist to
                            // rate-limit GRAFT/PRUNE thrash between mutually
                            // untrusted peers on a large permissionless mesh;
                            // a long prune-backoff there stops a churning peer
                            // from re-grafting instantly. Calimero's mesh is
                            // small (2–20 peers) and admission-gated by signed
                            // governance membership, so a peer worth meshing
                            // with is already trusted and one that isn't is
                            // rejected at the application layer regardless of
                            // mesh state — the long backoff buys no protection
                            // here and only slows legitimate re-grafting after
                            // a transient drop (e.g. the ping-watchdog close),
                            // which on a small mesh directly delays broadcast
                            // recovery. The 1s floor keeps re-meshing prompt
                            // while still collapsing a tight graft/prune loop.
                            .unsubscribe_backoff(1)
                            .prune_backoff(Duration::from_secs(1))
                            .build()
                            .map_err(|e| eyre::eyre!("invalid gossipsub config: {e}"))?,
                    )?,
                    ping: ping::Behaviour::new(
                        ping::Config::new()
                            .with_interval(PING_INTERVAL)
                            .with_timeout(PING_TIMEOUT),
                    ),
                    rendezvous: rendezvous::client::Behaviour::new(key.clone()),
                    relay: relay_behaviour,
                    stream: libp2p_stream::Behaviour::new(),
                    specialized_node_invite: request_response::Behaviour::new(
                        [(
                            CALIMERO_SPECIALIZED_NODE_INVITE_PROTOCOL,
                            ProtocolSupport::Full,
                        )],
                        request_response::Config::default().with_max_concurrent_streams(
                            SPECIALIZED_NODE_INVITE_MAX_CONCURRENT_STREAMS,
                        ),
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
            .with_swarm_config(|cfg| {
                let cfg = cfg.with_idle_connection_timeout(Duration::from_secs(30));
                match NonZeroU8::new(DIAL_CONCURRENCY_FACTOR) {
                    Some(factor) => cfg.with_dial_concurrency_factor(factor),
                    None => cfg,
                }
            })
            .build();

        for addr in &config.swarm.listen {
            let _ignored = swarm
                .listen_on(addr.clone())
                .wrap_err_with(|| format!("failed to listen on '{addr}'"))?;
        }

        Ok(swarm)
    }
}

/// Yamux muxer config with a bounded per-connection substream ceiling.
///
/// Passed by name to both `with_tcp` and `with_relay_client` so every
/// connection — direct or relayed — inherits the same `YAMUX_MAX_NUM_STREAMS`
/// cap instead of the library's 8192 default.
fn bounded_yamux_config() -> yamux::Config {
    let mut cfg = yamux::Config::default();
    cfg.set_max_num_streams(YAMUX_MAX_NUM_STREAMS);
    cfg
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
