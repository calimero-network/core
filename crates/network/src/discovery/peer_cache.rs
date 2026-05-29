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
//! **Staleness / IP-change handling** is threefold and lives here + in the
//! callers:
//!   1. `record` overwrites a peer's entry with the freshest observed
//!      address on every (re)connection — identify pushes listen-addr
//!      updates, so an IP change is captured live, not on a timer.
//!   2. `snapshot_fresh` / `retain_fresh` drop entries past a wall-clock
//!      TTL so a peer we haven't seen in a long time ages out of the file.
//!   3. The dialing caller evicts an address after repeated dial failures
//!      (the discovery book's existing failure-eviction threshold), so a
//!      stale cached address can't wedge reconnection.

use std::collections::HashMap;

use libp2p::{Multiaddr, PeerId};

/// Max addresses retained per peer, most-recent-first. A peer rarely
/// needs more than one good address; a small cap tolerates an in-flight
/// IP change (old + new both present briefly) without unbounded growth.
const MAX_ADDRS_PER_PEER: usize = 4;

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

    /// Drop `peer_id` entirely (e.g. it left every overlay we share).
    pub(crate) fn forget(&mut self, peer_id: &PeerId) {
        let _removed = self.peers.remove(peer_id);
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

    /// All currently-cached, still-fresh peers — the dial-on-startup set.
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
    fn forget_removes_peer() {
        let mut c = PeerAddrCache::default();
        c.record(peer(1), addr("/ip4/1.1.1.1/tcp/1"), 100);
        c.forget(&peer(1));
        assert!(c.dial_candidates(100, 1000).is_empty());
    }
}
