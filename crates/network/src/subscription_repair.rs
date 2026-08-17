//! Repair for a mesh whose two ends disagree about who subscribes to what.
//!
//! gossipsub announces our subscriptions to a peer exactly once, on the
//! **first** connection to it — `on_connection_established` returns early when
//! `other_established > 0`. A node that restarts with an unchanged `PeerId` and
//! reconnects before its peers have reaped the dead connection therefore never
//! hears their subscriptions: from their side the reconnect is "another
//! connection to a peer we already know", so nothing is re-announced, while the
//! returning node — for whom they are new — announces its own. The two views
//! come apart, and neither end's mesh maintenance can tell:
//!
//! * They still hold us in their mesh from before the restart, so they keep
//!   forwarding to us and we **receive** gossip and beacons as if healthy. Their
//!   heartbeat sees a populated mesh entry and has nothing to fix.
//! * Our subscriber table for them is empty, so we can neither publish
//!   (`NoPeersSubscribedToTopic`) nor choose a sync partner, and our own
//!   mesh-low heartbeat has no candidate to graft — it logs a mesh of 0 once a
//!   second, forever.
//!
//! One-way is worse than disconnected, because everything that would recover a
//! disconnected node is gated on having a peer to ask, and that is exactly what
//! this state denies. A governance op that arrives needing a parent it does not
//! have stays buffered permanently: the delivery works, the pull that would
//! complete it never finds anyone.
//!
//! A delivery is the missing evidence. A message reaching us on a topic proves
//! its forwarder follows that topic — it could not have forwarded it otherwise.
//! When our own table disagrees, the table is wrong, and re-announcing our
//! subscription makes the peer rebuild the mesh from its side: it drops us on
//! the `UNSUBSCRIBE`, re-adds us on the `SUBSCRIBE`, and grafts us back — and
//! that GRAFT is what finally teaches us the subscription we were missing,
//! because a peer that grafts us must be subscribed to the topic it grafted on.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use libp2p::gossipsub::{Behaviour as Gossipsub, IdentTopic, TopicHash};
use libp2p::PeerId;
use tracing::{error, info};

/// Minimum gap between two re-announcements of the same topic.
///
/// **This must stay well above how long one repair takes to land**, and that is
/// longer than it looks. Our `UNSUBSCRIBE` makes the peer drop us from its mesh,
/// and dropping a mesh peer records a `prune_backoff` against us — so the
/// graft-on-subscribe path is suppressed and the repairing GRAFT only arrives
/// from one of the peer's later mesh-low heartbeats. The reproduction test
/// measures ~8s end to end on loopback.
///
/// Deliveries on a live namespace arrive every couple of seconds, so a short
/// gap here would re-announce on top of a cycle still in flight — each attempt
/// resetting the backoff and starving the very heartbeat that completes the
/// previous one, turning a self-healing state into a livelock. The cost of
/// erring long is only that a node stuck one-way stays stuck a few more
/// seconds, against a failure that otherwise never resolves at all.
const READVERTISE_COOLDOWN: Duration = Duration::from_secs(30);

/// Per-topic rate limiter for [`SubscriptionRepair::on_delivery`].
#[derive(Debug, Default)]
pub(crate) struct SubscriptionRepair {
    /// When each topic was last re-announced. Entries are dropped once they
    /// age past the cooldown, so this stays bounded by the number of topics
    /// repaired within one window rather than growing with every topic ever
    /// seen.
    last_readvertised: HashMap<TopicHash, Instant>,
}

impl SubscriptionRepair {
    /// A message just arrived from `source` on `topic`. Re-announce our
    /// subscription when `source` is absent from our subscriber table for it,
    /// which can only mean the table is stale.
    ///
    /// Returns whether a re-announcement was issued.
    pub(crate) fn on_delivery(
        &mut self,
        gossipsub: &mut Gossipsub,
        source: &PeerId,
        topic: &TopicHash,
        now: Instant,
    ) -> bool {
        // Only our own subscriptions are ours to re-announce. A delivery on a
        // topic we do not follow says nothing about a table we could repair.
        if !gossipsub.topics().any(|ours| ours == topic) {
            return false;
        }

        // The ordinary case, on every healthy delivery: the table already
        // agrees with what just arrived.
        if gossipsub
            .all_peers()
            .any(|(peer, topics)| peer == source && topics.contains(&topic))
        {
            return false;
        }

        if let Some(last) = self.last_readvertised.get(topic) {
            if now.duration_since(*last) < READVERTISE_COOLDOWN {
                return false;
            }
        }

        // The identity hasher makes a topic's hash its own name, so this
        // reconstructs the very topic the delivery arrived on rather than a
        // lookalike.
        let ident = IdentTopic::new(topic.as_str());
        let _was_subscribed: bool = gossipsub.unsubscribe(&ident);
        if let Err(err) = gossipsub.subscribe(&ident) {
            // Only a subscription filter refuses a subscribe, and this
            // behaviour runs the allow-all filter — so this is unreachable
            // rather than recoverable. It still has to be loud: we have already
            // unsubscribed, and staying that way would take the topic down
            // altogether.
            error!(
                %err,
                topic = %topic,
                "re-announcing a subscription left the topic unsubscribed",
            );
            return false;
        }

        self.last_readvertised
            .retain(|_, at| now.duration_since(*at) < READVERTISE_COOLDOWN);
        let _previous: Option<Instant> = self.last_readvertised.insert(topic.clone(), now);

        info!(
            %source,
            topic = %topic,
            "peer forwarded a message on a topic our subscriber table says it does not \
             follow — the table is stale, re-announcing to rebuild the mesh",
        );
        true
    }
}

#[cfg(test)]
mod tests {
    use core::time::Duration as CoreDuration;

    use calimero_network_primitives::config::{
        AutonatConfig, BootstrapConfig, BootstrapNodes, DiscoveryConfig, NetworkConfig,
        RelayConfig, RendezvousConfig, SwarmConfig,
    };
    use futures_util::StreamExt;
    use libp2p::gossipsub::{Config, MessageAuthenticity, PublishError};
    use libp2p::identity::Keypair;
    use libp2p::{Multiaddr, Swarm};
    use tokio::time::timeout;

    use super::*;
    use crate::behaviour::Behaviour;

    fn topic_of(seed: u8) -> (IdentTopic, TopicHash) {
        let ident = IdentTopic::new(format!("ns/{}", hex::encode([seed; 32])));
        let hash = ident.hash();
        (ident, hash)
    }

    /// A bare behaviour, with no connections. Every peer is therefore
    /// "unrecorded", which is the precondition the repair keys off.
    fn bare_gossipsub() -> Gossipsub {
        Gossipsub::new(
            MessageAuthenticity::Signed(Keypair::generate_ed25519()),
            Config::default(),
        )
        .expect("gossipsub behaviour")
    }

    #[test]
    fn a_delivery_on_a_topic_we_do_not_follow_is_not_ours_to_repair() {
        let mut gossipsub = bare_gossipsub();
        let (ours, _) = topic_of(0x01);
        let (_, theirs) = topic_of(0x02);
        let _subscribed: bool = gossipsub.subscribe(&ours).expect("subscribe");

        assert!(
            !SubscriptionRepair::default().on_delivery(
                &mut gossipsub,
                &PeerId::random(),
                &theirs,
                Instant::now(),
            ),
            "a topic this node does not follow has no subscription of ours to re-announce",
        );
    }

    #[test]
    fn an_unrecorded_forwarder_triggers_one_re_announce_per_cooldown() {
        let mut gossipsub = bare_gossipsub();
        let (ident, hash) = topic_of(0x03);
        let _subscribed: bool = gossipsub.subscribe(&ident).expect("subscribe");

        let mut repair = SubscriptionRepair::default();
        let source = PeerId::random();
        let t0 = Instant::now();

        assert!(
            repair.on_delivery(&mut gossipsub, &source, &hash, t0),
            "a forwarder absent from the subscriber table means the table is stale",
        );
        assert!(
            gossipsub.topics().any(|t| *t == hash),
            "the re-announce must leave us subscribed — unsubscribing without \
             re-subscribing would take the topic down entirely",
        );

        // A live namespace delivers faster than the peer can graft us back, and
        // re-announcing on top of a cycle in flight resets the very backoff the
        // repairing GRAFT is waiting out.
        assert!(
            !repair.on_delivery(&mut gossipsub, &source, &hash, t0 + Duration::from_secs(1)),
            "a second delivery inside the cooldown must not re-announce again",
        );
        assert!(
            repair.on_delivery(
                &mut gossipsub,
                &source,
                &hash,
                t0 + READVERTISE_COOLDOWN + Duration::from_millis(1),
            ),
            "past the cooldown it must retry — a repair can be lost, see the \
             module docs on gossipsub's per-peer send queue",
        );
    }

    #[test]
    fn stale_cooldown_entries_do_not_accumulate() {
        let mut gossipsub = bare_gossipsub();
        let mut repair = SubscriptionRepair::default();
        let source = PeerId::random();
        let t0 = Instant::now();

        for seed in 0..8u8 {
            let (ident, hash) = topic_of(seed);
            let _subscribed: bool = gossipsub.subscribe(&ident).expect("subscribe");
            // Each topic is stamped a full cooldown apart, so by the last one
            // every earlier entry has aged out.
            let at = t0 + READVERTISE_COOLDOWN * u32::from(seed);
            assert!(repair.on_delivery(&mut gossipsub, &source, &hash, at));
        }

        assert_eq!(
            repair.last_readvertised.len(),
            1,
            "only entries inside the cooldown window are worth keeping",
        );
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
                AutonatConfig::new(5, CoreDuration::from_secs(10)),
            ),
        )
    }

    async fn free_addr() -> Multiaddr {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        format!("/ip4/127.0.0.1/tcp/{port}").parse().unwrap()
    }

    /// Peers of `swarm` that it believes follow `topic` — the same read
    /// `SubscribedPeers` (and therefore sync peer-selection) performs.
    fn subscribers(swarm: &Swarm<Behaviour>, topic: &TopicHash) -> Vec<PeerId> {
        swarm
            .behaviour()
            .gossipsub
            .all_peers()
            .filter_map(|(peer, topics)| topics.contains(&topic).then_some(*peer))
            .collect()
    }

    /// Poll both swarms until `done`, or give up.
    ///
    /// The 200ms tick is not padding: this loop only re-checks `done` after a
    /// swarm event, and a mesh repair completes on the remote peer's heartbeat
    /// timer with nothing to deliver here in between. Production has no such
    /// gap — actix polls the swarm continuously.
    async fn drive_until(
        a: &mut Swarm<Behaviour>,
        b: &mut Swarm<Behaviour>,
        label: &str,
        mut done: impl FnMut(&Swarm<Behaviour>, &Swarm<Behaviour>) -> bool,
    ) {
        timeout(CoreDuration::from_secs(60), async {
            loop {
                if done(a, b) {
                    return;
                }
                tokio::select! {
                    _ = a.select_next_some() => {}
                    _ = b.select_next_some() => {}
                    _ = tokio::time::sleep(CoreDuration::from_millis(200)) => {}
                }
            }
        })
        .await
        .unwrap_or_else(|_| panic!("timed out waiting for: {label}"));
    }

    async fn wait_for_listener(swarm: &mut Swarm<Behaviour>) -> Multiaddr {
        timeout(CoreDuration::from_secs(10), async {
            loop {
                if let Some(addr) = swarm.listeners().next().cloned() {
                    return addr;
                }
                drop(swarm.next().await.expect("swarm event"));
            }
        })
        .await
        .expect("swarm never started listening")
    }

    /// The whole failure this module exists for, over two real swarms.
    ///
    /// A node restarts with an unchanged `PeerId` and reconnects while its peer
    /// still holds the pre-restart connection. The peer skips announcing its
    /// subscriptions (not a first connection), the returning node announces its
    /// own, and the result is one-way: the returning node receives but can
    /// neither publish nor name a peer to sync with, and no heartbeat on either
    /// side can notice.
    #[tokio::test]
    async fn a_returning_peer_repairs_its_one_way_subscriber_table() {
        let (ident, topic) = topic_of(0x5a);

        let mut peer_a = Behaviour::build_swarm(&network_config(
            Keypair::generate_ed25519(),
            free_addr().await,
        ))
        .expect("swarm a");
        let _subscribed: bool = peer_a
            .behaviour_mut()
            .gossipsub
            .subscribe(&ident)
            .expect("a subscribes");
        let addr_a = wait_for_listener(&mut peer_a).await;
        let id_a = *peer_a.local_peer_id();

        // The identity that survives the restart. Both incarnations present the
        // same `PeerId` to `peer_a`, which is what makes the reconnect look like
        // an extra connection rather than a new peer.
        let returning = Keypair::generate_ed25519();
        let id_returning = returning.public().to_peer_id();

        let mut before =
            Behaviour::build_swarm(&network_config(returning.clone(), free_addr().await))
                .expect("swarm before restart");
        let _subscribed: bool = before
            .behaviour_mut()
            .gossipsub
            .subscribe(&ident)
            .expect("pre-restart subscribes");
        before.dial(addr_a.clone()).expect("dial a");

        drive_until(
            &mut peer_a,
            &mut before,
            "both ends record each other",
            |a, b| {
                subscribers(a, &topic).contains(&id_returning)
                    && subscribers(b, &topic).contains(&id_a)
            },
        )
        .await;

        // The restart. A fresh swarm on the same identity, and `peer_a` is never
        // told the old one is gone.
        let mut after = Behaviour::build_swarm(&network_config(returning, free_addr().await))
            .expect("swarm after restart");
        let _subscribed: bool = after
            .behaviour_mut()
            .gossipsub
            .subscribe(&ident)
            .expect("post-restart subscribes");
        after.dial(addr_a.clone()).expect("redial a");

        // Gate on the PEER holding both connections at once, not merely on the
        // returning node believing it is connected. The whole failure hinges on
        // the reconnect arriving while the stale connection is still there — if
        // the peer processes the close first, the reconnect is a first
        // connection, it announces normally, and there is nothing to reproduce.
        drive_until(
            &mut peer_a,
            &mut after,
            "the peer holds the stale and the fresh connection at once",
            |a, _| a.network_info().connection_counters().num_established() == 2,
        )
        .await;

        // Retire the pre-restart connection now that the new one is up, so
        // exactly one connection carries the peer — the state a real node
        // reaches once the dead socket is finally reaped, and what makes the
        // rest of this test deterministic rather than a race between two
        // connections sharing gossipsub's per-peer send queue.
        drop(before);
        drive_until(
            &mut peer_a,
            &mut after,
            "the pre-restart connection is retired",
            |a, _| a.network_info().connection_counters().num_established() == 1,
        )
        .await;

        // The one-way state, from both sides at once.
        assert!(
            subscribers(&peer_a, &topic).contains(&id_returning),
            "precondition: the peer still holds the returning node as a subscriber, \
             which is why it keeps delivering to it",
        );
        assert!(
            subscribers(&after, &topic).is_empty(),
            "the returning node was never told what its peer follows, so it can \
             name nobody to publish to or sync with",
        );
        assert!(
            matches!(
                after
                    .behaviour_mut()
                    .gossipsub
                    .publish(topic.clone(), b"blocked".to_vec()),
                Err(PublishError::NoPeersSubscribedToTopic),
            ),
            "and that is not cosmetic: it cannot publish at all — the exact error \
             a stranded node logs when its own heartbeat is dropped",
        );

        // A delivery is the evidence. Standing in for one here rather than
        // publishing from `peer_a`, so the test pins the repair itself and not
        // gossipsub's forwarding timing.
        assert!(
            SubscriptionRepair::default().on_delivery(
                &mut after.behaviour_mut().gossipsub,
                &id_a,
                &topic,
                Instant::now(),
            ),
            "a forwarder missing from the table must trigger the re-announce",
        );

        drive_until(
            &mut peer_a,
            &mut after,
            "the returning node learns the subscription",
            |_, b| subscribers(b, &topic).contains(&id_a),
        )
        .await;

        assert!(
            after
                .behaviour_mut()
                .gossipsub
                .publish(topic, b"unblocked".to_vec())
                .is_ok(),
            "with the table repaired it can publish again",
        );
    }
}
