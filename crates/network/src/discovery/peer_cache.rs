//! Persistent address cache for relevant peers.
//!
//! Lets a node reconnect to the peers it actually collaborates with
//! immediately on restart — dialing their last-known addresses in
//! parallel with (and ahead of) rendezvous rediscovery — instead of
//! waiting a discovery round-trip with nothing to dial.
//!
//! Distinct from the discovery address book in [`super::state`]:
//!
//! - The discovery book holds **direct** addresses only (relayed
//!   multiaddrs are dropped, since they aren't direct-dial entries).
//!   That's correct for its job but useless for reconnecting to a NAT'd
//!   peer, which has no direct address.
//! - This cache deliberately keeps **both** direct and relayed-circuit
//!   addresses. A `/<relay>/p2p-circuit/p2p/<peer>` address stays
//!   re-dialable after a restart as long as the peer re-reserves on the
//!   same relay (the circuit is keyed by the peer's stable PeerId, not a
//!   per-session token), so caching it gives NAT'd peers a fast path too.
//!   When it doesn't work (peer moved relays, IP changed), the stale dial
//!   fails, the address is dropped, and rendezvous supplies the fresh one.
//!
//! **Wiring** (all live; see the `peer_cache_*` methods on the discovery
//! `NetworkManager` impl): recorded on `ConnectionEstablished`, persisted
//! to a node-local `Generic` datastore key on the rendezvous tick, and
//! loaded + dialed on `started()`.
//!
//! **Staleness / IP-change handling** is threefold:
//!   1. `record` overwrites a peer's entry with the freshest observed
//!      address on every (re)connection — identify pushes listen-addr
//!      updates, so an IP change is captured live, not on a timer.
//!   2. TTL: `load_fresh` / `snapshot_relevant_fresh` drop entries past a
//!      wall-clock TTL (24h), so a peer not seen recently ages out of the
//!      blob entirely — including a relayed-circuit address whose relay
//!      the peer has since left.
//!   3. A stale cached address that's dialed and fails is deduped at the
//!      swarm level (`DisconnectedAndNotDialing`) and re-supplied fresh by
//!      rendezvous; a direct address additionally hits the discovery
//!      book's failure-eviction threshold. The cache itself is a best-
//!      effort hint — it never blocks reconnection, it only accelerates it.

use std::collections::{BTreeSet, HashMap};

use libp2p::{Multiaddr, PeerId};
use serde::{Deserialize, Serialize};
use tracing::debug;

/// On-disk form of a cached peer. `PeerId`/`Multiaddr` are stored as
/// strings so the file is human-readable and we don't depend on libp2p's
/// optional serde features. Unparseable entries are skipped on load.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct PersistedPeer {
    pub(crate) peer_id: String,
    pub(crate) addrs: Vec<String>,
    pub(crate) last_seen_secs: u64,
}

/// Max addresses retained per peer, most-recent-first. A peer rarely
/// needs more than one good address; a small cap tolerates an in-flight
/// IP change (old + new both present briefly) without unbounded growth.
const MAX_ADDRS_PER_PEER: usize = 4;

/// Hard cap on the number of peer entries retained in the live map after
/// TTL pruning. The per-peer address list is already bounded by
/// [`MAX_ADDRS_PER_PEER`], but the *entry count* was not — a node on the
/// global rendezvous namespace can briefly connect to many transient (or
/// adversarial) peers, and without a cap the map grew one entry per
/// distinct `PeerId` ever seen for the process lifetime. The cap is
/// generous (a node rarely collaborates with this many distinct
/// co-members); eviction is least-recently-seen, so actively-connected
/// co-members — refreshed on every (re)connect — are kept, transients
/// drop out.
pub(crate) const MAX_PEER_CACHE_ENTRIES: usize = 4096;

/// One cached peer: its stable id, the addresses we last connected to it
/// on (most-recent-first, direct and/or relayed), and the wall-clock unix
/// second we last saw it. Wall-clock (not a monotonic `Instant`) so the
/// freshness survives a process restart — the whole point of the cache.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CachedPeer {
    pub(crate) peer_id: PeerId,
    pub(crate) addrs: Vec<Multiaddr>,
    pub(crate) last_seen_secs: u64,
}

/// In-memory address cache, snapshotted to disk by the node layer.
#[derive(Default, Debug)]
pub(crate) struct PeerAddrCache {
    peers: HashMap<PeerId, CachedPeer>,
}

impl PeerAddrCache {
    /// Record a freshly observed address for `peer_id`. Moves the address
    /// to the front (most-recent-first), de-duplicates, caps at
    /// [`MAX_ADDRS_PER_PEER`], and refreshes `last_seen_secs`.
    ///
    /// This is the IP-change capture path: on every (re)connection — or
    /// identify listen-addr push — the new address jumps to the front, so
    /// the cache always dials the latest first while keeping a couple of
    /// prior addresses as fallbacks until they fail.
    pub(crate) fn record(&mut self, peer_id: PeerId, addr: Multiaddr, now_secs: u64) {
        let entry = self.peers.entry(peer_id).or_insert_with(|| CachedPeer {
            peer_id,
            addrs: Vec::new(),
            last_seen_secs: now_secs,
        });
        entry.last_seen_secs = now_secs;
        entry.addrs.retain(|a| a != &addr);
        entry.addrs.insert(0, addr);
        entry.addrs.truncate(MAX_ADDRS_PER_PEER);
    }

    /// Serialize the relevant, still-fresh entries for persistence to
    /// disk. Filters by `relevant` (current overlay co-members) and TTL,
    /// then renders ids/addrs as strings.
    pub(crate) fn to_persisted(
        &self,
        relevant: &BTreeSet<PeerId>,
        now_secs: u64,
        ttl_secs: u64,
    ) -> Vec<PersistedPeer> {
        self.snapshot_relevant_fresh(relevant, now_secs, ttl_secs)
            .into_iter()
            .map(|p| PersistedPeer {
                peer_id: p.peer_id.to_base58(),
                addrs: p.addrs.iter().map(Multiaddr::to_string).collect(),
                last_seen_secs: p.last_seen_secs,
            })
            .collect()
    }

    /// Rebuild a cache from a loaded snapshot, parsing string ids/addrs
    /// and dropping any that are malformed or past `ttl_secs`. Malformed
    /// entries are logged at debug and skipped rather than failing the
    /// whole load — a corrupt line shouldn't lose the rest of the cache.
    pub(crate) fn from_persisted(
        records: Vec<PersistedPeer>,
        now_secs: u64,
        ttl_secs: u64,
    ) -> Self {
        let entries = records
            .into_iter()
            .filter_map(|r| {
                let peer_id = match r.peer_id.parse::<PeerId>() {
                    Ok(p) => p,
                    Err(err) => {
                        debug!(peer_id = %r.peer_id, ?err, "skipping unparseable cached peer id");
                        return None;
                    }
                };
                let addrs: Vec<Multiaddr> = r.addrs.iter().filter_map(|a| a.parse().ok()).collect();
                if addrs.is_empty() {
                    return None;
                }
                Some(CachedPeer {
                    peer_id,
                    addrs,
                    last_seen_secs: r.last_seen_secs,
                })
            })
            .collect();
        Self::load_fresh(entries, now_secs, ttl_secs)
    }

    /// Replace the cache contents from a loaded snapshot, keeping only
    /// entries seen within `ttl_secs` of `now_secs`. Used on startup so a
    /// long-dead peer doesn't get dialed after a long downtime.
    pub(crate) fn load_fresh(entries: Vec<CachedPeer>, now_secs: u64, ttl_secs: u64) -> Self {
        let peers = entries
            .into_iter()
            .filter(|p| is_fresh(p.last_seen_secs, now_secs, ttl_secs))
            .map(|p| (p.peer_id, p))
            .collect();
        Self { peers }
    }

    /// Fresh entries (seen within `ttl_secs`) restricted to `relevant`
    /// peers — the set the node layer persists. Filtering by relevance
    /// (current overlay co-members) keeps the file proportional to who we
    /// actually collaborate with rather than every peer ever seen.
    pub(crate) fn snapshot_relevant_fresh(
        &self,
        relevant: &std::collections::BTreeSet<PeerId>,
        now_secs: u64,
        ttl_secs: u64,
    ) -> Vec<CachedPeer> {
        let mut out: Vec<CachedPeer> = self
            .peers
            .values()
            .filter(|p| relevant.contains(&p.peer_id))
            .filter(|p| is_fresh(p.last_seen_secs, now_secs, ttl_secs))
            .cloned()
            .collect();
        // Deterministic order (HashMap iteration is randomised) so the
        // persisted file is stable across runs and tests are reproducible.
        out.sort_by(|a, b| a.peer_id.cmp(&b.peer_id));
        out
    }

    /// All currently-cached, still-fresh peers, sorted by `peer_id`. A
    /// test-only inspection helper for the cache contents; production dials the
    /// recency-capped [`startup_dial_set`](Self::startup_dial_set) instead.
    #[cfg(test)]
    pub(crate) fn dial_candidates(&self, now_secs: u64, ttl_secs: u64) -> Vec<CachedPeer> {
        let mut out: Vec<CachedPeer> = self
            .peers
            .values()
            .filter(|p| is_fresh(p.last_seen_secs, now_secs, ttl_secs))
            .cloned()
            .collect();
        out.sort_by(|a, b| a.peer_id.cmp(&b.peer_id));
        out
    }

    /// Count of currently-cached, still-fresh peers (the full reconnect set
    /// before the startup dial cap is applied). Used only for logging how many
    /// of the fresh candidates were dropped by the cap.
    pub(crate) fn fresh_count(&self, now_secs: u64, ttl_secs: u64) -> usize {
        self.peers
            .values()
            .filter(|p| is_fresh(p.last_seen_secs, now_secs, ttl_secs))
            .count()
    }

    /// The startup reconnect dial set: at most `max` fresh peers, ordered
    /// most-recently-seen first (ties broken by `peer_id` for determinism).
    ///
    /// Bounds the startup dial burst so a full or poisoned cache can't trigger
    /// a dial storm; the dropped peers are simply rediscovered via rendezvous.
    pub(crate) fn startup_dial_set(
        &self,
        now_secs: u64,
        ttl_secs: u64,
        max: usize,
    ) -> Vec<CachedPeer> {
        let mut out: Vec<CachedPeer> = self
            .peers
            .values()
            .filter(|p| is_fresh(p.last_seen_secs, now_secs, ttl_secs))
            .cloned()
            .collect();
        out.sort_by(|a, b| {
            b.last_seen_secs
                .cmp(&a.last_seen_secs)
                .then_with(|| a.peer_id.cmp(&b.peer_id))
        });
        out.truncate(max);
        out
    }

    /// Bound the live map: first drop entries older than `ttl_secs`, then,
    /// if more than `max_entries` remain, evict the least-recently-seen
    /// down to the cap. Until now TTL/relevance were applied only at
    /// read time (snapshot/dial/load), never to the resident map, so it
    /// accumulated one entry per `PeerId` ever connected to — slow
    /// growth / a churn-driven DoS vector. Called on the rendezvous tick
    /// (~15s) so the map stays proportional to recently-active peers.
    ///
    /// LRU eviction keeps actively-connected co-members (their
    /// `last_seen` is refreshed on every (re)connect) and sheds
    /// transient/attacker peers, so reconnect-on-restart for real
    /// co-members is unaffected.
    pub(crate) fn prune(&mut self, now_secs: u64, ttl_secs: u64, max_entries: usize) {
        self.peers
            .retain(|_, p| is_fresh(p.last_seen_secs, now_secs, ttl_secs));
        if self.peers.len() <= max_entries {
            return;
        }
        // Still over the cap after TTL pruning: keep the `max_entries`
        // most-recently-seen entries, evict the rest. Tie-break by PeerId
        // so eviction is deterministic.
        let mut by_recency: Vec<(PeerId, u64)> = self
            .peers
            .iter()
            .map(|(id, p)| (*id, p.last_seen_secs))
            .collect();
        by_recency.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        for (id, _) in by_recency.into_iter().skip(max_entries) {
            let _ = self.peers.remove(&id);
        }
    }

    /// Number of resident entries — for the prune metric / tests.
    pub(crate) fn len(&self) -> usize {
        self.peers.len()
    }
}

/// `last_seen_secs` is within `ttl_secs` of `now_secs`. A clock that
/// jumped backwards (`now < last_seen`) counts as fresh — we'd rather
/// keep a possibly-good entry than drop it on a clock glitch.
fn is_fresh(last_seen_secs: u64, now_secs: u64, ttl_secs: u64) -> bool {
    now_secs.saturating_sub(last_seen_secs) <= ttl_secs
}

#[cfg(test)]
mod tests {
    use super::*;

    fn peer(n: u8) -> PeerId {
        let kp = libp2p::identity::Keypair::ed25519_from_bytes([n; 32]).expect("seed");
        PeerId::from_public_key(&kp.public())
    }

    fn addr(s: &str) -> Multiaddr {
        s.parse().expect("multiaddr")
    }

    #[test]
    fn record_inserts_and_refreshes_last_seen() {
        let mut c = PeerAddrCache::default();
        c.record(peer(1), addr("/ip4/1.2.3.4/tcp/1"), 100);
        let snap = c.dial_candidates(100, 1000);
        assert_eq!(snap.len(), 1);
        assert_eq!(snap[0].last_seen_secs, 100);

        // Re-record later bumps last_seen.
        c.record(peer(1), addr("/ip4/1.2.3.4/tcp/1"), 250);
        assert_eq!(c.dial_candidates(250, 1000)[0].last_seen_secs, 250);
    }

    #[test]
    fn record_moves_new_address_to_front_for_ip_change() {
        // The IP-change case: a peer reconnects on a new address. The new
        // one must be dialed first, with the old kept as a fallback.
        let mut c = PeerAddrCache::default();
        c.record(peer(1), addr("/ip4/1.1.1.1/tcp/1"), 100);
        c.record(peer(1), addr("/ip4/2.2.2.2/tcp/1"), 200);
        let entry = &c.dial_candidates(200, 1000)[0];
        assert_eq!(
            entry.addrs,
            vec![addr("/ip4/2.2.2.2/tcp/1"), addr("/ip4/1.1.1.1/tcp/1")],
            "newest address first, prior kept as fallback"
        );
    }

    #[test]
    fn record_dedupes_and_caps_addresses() {
        let mut c = PeerAddrCache::default();
        for i in 0..(MAX_ADDRS_PER_PEER as u16 + 3) {
            c.record(
                peer(1),
                addr(&format!("/ip4/1.2.3.4/tcp/{i}")),
                100 + u64::from(i),
            );
        }
        // Re-recording an existing addr must not duplicate it.
        c.record(peer(1), addr("/ip4/1.2.3.4/tcp/0"), 999);
        let entry = &c.dial_candidates(999, 100_000)[0];
        assert!(entry.addrs.len() <= MAX_ADDRS_PER_PEER, "capped");
        assert_eq!(
            entry
                .addrs
                .iter()
                .filter(|a| **a == addr("/ip4/1.2.3.4/tcp/0"))
                .count(),
            1,
            "no duplicate addresses"
        );
        assert_eq!(
            entry.addrs[0],
            addr("/ip4/1.2.3.4/tcp/0"),
            "re-recorded addr moved to front"
        );
    }

    #[test]
    fn stale_entries_are_dropped_by_ttl() {
        let mut c = PeerAddrCache::default();
        c.record(peer(1), addr("/ip4/1.2.3.4/tcp/1"), 100);
        // now=2000, ttl=1000 → 1900 > 1000 → stale.
        assert!(
            c.dial_candidates(2000, 1000).is_empty(),
            "past-TTL entry dropped"
        );
        // within TTL → kept.
        assert_eq!(c.dial_candidates(900, 1000).len(), 1);
    }

    #[test]
    fn startup_dial_set_caps_to_most_recent_and_excludes_stale() {
        let mut c = PeerAddrCache::default();
        // Five fresh peers (last_seen 910..=950) and one stale (800).
        c.record(peer(1), addr("/ip4/1.0.0.1/tcp/1"), 910);
        c.record(peer(2), addr("/ip4/1.0.0.2/tcp/1"), 920);
        c.record(peer(3), addr("/ip4/1.0.0.3/tcp/1"), 930);
        c.record(peer(4), addr("/ip4/1.0.0.4/tcp/1"), 940);
        c.record(peer(5), addr("/ip4/1.0.0.5/tcp/1"), 950);
        c.record(peer(6), addr("/ip4/1.0.0.6/tcp/1"), 800);

        // now=1000, ttl=100 → fresh iff last_seen >= 900, so peer(6) is stale.
        let now = 1000;
        let ttl = 100;
        assert_eq!(c.fresh_count(now, ttl), 5, "stale peer excluded from count");

        // Cap of 3 → the three most-recently-seen, most-recent-first.
        let set = c.startup_dial_set(now, ttl, 3);
        let ids: Vec<PeerId> = set.iter().map(|p| p.peer_id).collect();
        assert_eq!(
            ids,
            vec![peer(5), peer(4), peer(3)],
            "most-recently-seen three, newest first"
        );
        // Ordering is strictly descending by last_seen.
        assert!(set
            .windows(2)
            .all(|w| w[0].last_seen_secs >= w[1].last_seen_secs));
        // The stale peer is never dialed regardless of the cap.
        assert!(
            !c.startup_dial_set(now, ttl, 100)
                .iter()
                .any(|p| p.peer_id == peer(6)),
            "stale peer must not be in the dial set even when the cap is not reached"
        );
    }

    #[test]
    fn load_fresh_keeps_only_unexpired() {
        let entries = vec![
            CachedPeer {
                peer_id: peer(1),
                addrs: vec![addr("/ip4/1.1.1.1/tcp/1")],
                last_seen_secs: 100,
            },
            CachedPeer {
                peer_id: peer(2),
                addrs: vec![addr("/ip4/2.2.2.2/tcp/1")],
                last_seen_secs: 1900,
            },
        ];
        let c = PeerAddrCache::load_fresh(entries, 2000, 1000);
        let got = c.dial_candidates(2000, 1000);
        assert_eq!(got.len(), 1, "only the within-TTL peer survives load");
        assert_eq!(got[0].peer_id, peer(2));
    }

    #[test]
    fn snapshot_relevant_fresh_filters_by_relevance() {
        let mut c = PeerAddrCache::default();
        c.record(peer(1), addr("/ip4/1.1.1.1/tcp/1"), 100);
        c.record(peer(2), addr("/ip4/2.2.2.2/tcp/1"), 100);
        let relevant = std::collections::BTreeSet::from([peer(2)]);
        let snap = c.snapshot_relevant_fresh(&relevant, 100, 1000);
        assert_eq!(snap.len(), 1, "only relevant peers persisted");
        assert_eq!(snap[0].peer_id, peer(2));
    }

    #[test]
    fn persisted_round_trips_through_strings() {
        let mut c = PeerAddrCache::default();
        c.record(peer(1), addr("/ip4/1.1.1.1/tcp/1"), 100);
        c.record(peer(1), addr("/ip4/2.2.2.2/tcp/2"), 150);
        let relevant = std::collections::BTreeSet::from([peer(1)]);

        let records = c.to_persisted(&relevant, 150, 1000);
        // JSON serialize/deserialize as the node-side persistence would.
        let json = serde_json::to_string(&records).expect("serialize");
        let back: Vec<PersistedPeer> = serde_json::from_str(&json).expect("deserialize");

        let restored = PeerAddrCache::from_persisted(back, 150, 1000);
        let got = restored.dial_candidates(150, 1000);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].peer_id, peer(1));
        assert_eq!(
            got[0].addrs,
            vec![addr("/ip4/2.2.2.2/tcp/2"), addr("/ip4/1.1.1.1/tcp/1")],
            "address order (newest-first) survives the round trip"
        );
    }

    #[test]
    fn from_persisted_skips_malformed_and_expired() {
        let records = vec![
            PersistedPeer {
                peer_id: "not-a-peer-id".to_owned(),
                addrs: vec!["/ip4/1.1.1.1/tcp/1".to_owned()],
                last_seen_secs: 100,
            },
            PersistedPeer {
                peer_id: peer(2).to_base58(),
                addrs: vec!["garbage-addr".to_owned()],
                last_seen_secs: 100,
            },
            PersistedPeer {
                peer_id: peer(3).to_base58(),
                addrs: vec!["/ip4/3.3.3.3/tcp/3".to_owned()],
                last_seen_secs: 100,
            },
        ];
        let c = PeerAddrCache::from_persisted(records, 150, 1000);
        let got = c.dial_candidates(150, 1000);
        assert_eq!(got.len(), 1, "only the well-formed entry survives");
        assert_eq!(got[0].peer_id, peer(3));
    }

    #[test]
    fn prune_drops_past_ttl_entries_from_the_live_map() {
        let mut c = PeerAddrCache::default();
        c.record(peer(1), addr("/ip4/1.1.1.1/tcp/1"), 100); // stale at now=2000
        c.record(peer(2), addr("/ip4/2.2.2.2/tcp/1"), 1900); // fresh
        assert_eq!(c.len(), 2);

        // now=2000, ttl=1000 → peer(1)@100 is 1900s old → evicted.
        c.prune(2000, 1000, 100);
        assert_eq!(c.len(), 1, "stale entry removed from the resident map");
        assert_eq!(c.dial_candidates(2000, 1000)[0].peer_id, peer(2));
    }

    #[test]
    fn prune_caps_total_evicting_least_recently_seen() {
        let mut c = PeerAddrCache::default();
        // Five fresh peers, distinct last_seen.
        for (i, n) in [(110u64, 1u8), (120, 2), (130, 3), (140, 4), (150, 5)] {
            c.record(peer(n), addr(&format!("/ip4/1.2.3.{n}/tcp/1")), i);
        }
        assert_eq!(c.len(), 5);

        // All fresh, but cap to 3 → keep the 3 most-recently-seen (3,4,5),
        // evict the 2 oldest (1,2).
        c.prune(200, 1000, 3);
        let kept: std::collections::BTreeSet<PeerId> = c
            .dial_candidates(200, 1000)
            .into_iter()
            .map(|p| p.peer_id)
            .collect();
        assert_eq!(
            kept,
            std::collections::BTreeSet::from([peer(3), peer(4), peer(5)])
        );
    }

    #[test]
    fn prune_is_a_noop_under_the_cap() {
        let mut c = PeerAddrCache::default();
        c.record(peer(1), addr("/ip4/1.1.1.1/tcp/1"), 100);
        c.prune(100, 1000, MAX_PEER_CACHE_ENTRIES);
        assert_eq!(c.len(), 1);
    }
}
