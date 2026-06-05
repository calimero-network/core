//! Persistent member-identity → peer cache, keyed per group.
//!
//! Lets a node dial the **actual members** of a context's group on a
//! cold cache — immediately after restart, before any live signed
//! traffic has had a chance to re-teach it who is whom — instead of
//! falling back to dialing random topic subscribers.
//!
//! ## The gap this fills
//!
//! Dialing "members of context X" needs a three-link chain:
//!
//! ```text
//! member identity (PublicKey) ──▶ member's node (PeerId) ──▶ address
//! ```
//!
//! Links 1 (governance member rows / `trusted_anchors`) and 3
//! (`PeerAddrCache`) are already persisted on disk. The **middle link**
//! — "this `PeerId` is member-identity `X`" — lived only in an in-memory
//! `peer_identities` map rebuilt from scratch every restart by
//! re-observing signed traffic. Until it refilled, peer selection had no
//! membership signal and degraded to random topic subscribers. This
//! cache persists that middle link so the signal survives a restart.
//!
//! ## Authenticated, hint-only
//!
//! Entries are written **only** from the existing `observe_peer_identity`
//! gate (signature verified + `MembershipStatus::Member` at the
//! cross-DAG cut), so the cache never holds a self-asserted claim.
//! Everything here is nonetheless a **routing hint, never authority**:
//! actual sync re-verifies every governance op's signature at adoption,
//! so a stale entry costs at most one wasted dial — never correctness.
//! In particular `role` is the member's role *as last observed*;
//! governance `trusted_anchors` remains the source of truth at selection
//! time (and "owner" is a group `Meta` field, not a [`GroupMemberRole`],
//! so it is derived at resolution rather than stored here).
//!
//! ## Keyed per group
//!
//! A member's role is per-group (an identity can be `Admin` of a
//! subgroup but a plain `Member` at the namespace root), so the storage
//! unit is **one bucket per [`ContextGroupId`]**. Resolution today reads
//! the bucket of the context's own group; the per-group keying is what
//! lets a future refinement walk the group tree (group → parent groups →
//! namespace root) and union the buckets with each level's roles intact,
//! without restructuring storage.
//!
//! Mirrors [`crate`](crate)'s sibling discipline in the network crate's
//! `PeerAddrCache`: pure data + TTL here, with the snapshot tick and
//! startup hydration wired by the node layer.

use std::collections::{BTreeMap, BTreeSet};

use calimero_context_config::types::ContextGroupId;
use calimero_primitives::context::GroupMemberRole;
use calimero_primitives::identity::PublicKey;
use libp2p::PeerId;
use serde::{Deserialize, Serialize};
use tracing::debug;

/// Freshness window for cached observations (24h), matching
/// `PeerAddrCache`. A member not seen within the window ages out of the
/// snapshot so a long-dead mapping isn't dialed after a long downtime.
pub(crate) const PEER_IDENTITY_TTL_SECS: u64 = 24 * 60 * 60;

/// On-disk form of one `(identity, peer)` observation within a group.
///
/// `identity`/`peer_id` are stored as strings (via [`PublicKey`]'s
/// `Display`/`FromStr` and `PeerId`'s base58) so the blob is
/// human-readable and free of libp2p's optional serde features;
/// unparseable rows are skipped on load. One row per hosting peer — an
/// identity hosted on several peers (TEE fleet, multi-device) yields
/// several rows sharing the same `role`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct PersistedIdentityPeer {
    pub(crate) identity: String,
    pub(crate) peer_id: String,
    pub(crate) role: GroupMemberRole,
    pub(crate) last_seen_secs: u64,
}

/// The peers hosting one member identity within a group, plus the role
/// that identity holds there. `peers` is keyed by `PeerId` with its own
/// `last_seen` stamp because one identity can re-home across nodes or be
/// hosted on several at once.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct MemberHosts {
    pub(crate) role: GroupMemberRole,
    /// `peer → last_seen_secs` (wall-clock unix seconds, so freshness
    /// survives a restart).
    pub(crate) peers: BTreeMap<PeerId, u64>,
}

/// All observed members of one group: `identity → {role, hosting peers}`.
#[derive(Default, Clone, Debug)]
pub(crate) struct GroupMembers {
    members: BTreeMap<PublicKey, MemberHosts>,
}

/// A member resolved for dial selection: its identity, role in the
/// group, and the fresh peers hosting it.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ResolvedMember {
    pub(crate) identity: PublicKey,
    pub(crate) role: GroupMemberRole,
    pub(crate) peers: Vec<PeerId>,
}

/// In-memory member-identity cache: one bucket per group, snapshotted to
/// disk per group by the node layer.
#[derive(Default, Debug)]
pub(crate) struct PeerIdentityCache {
    groups: BTreeMap<ContextGroupId, GroupMembers>,
}

impl PeerIdentityCache {
    /// Record an authenticated observation: `identity`, a member of
    /// `group` with `role`, is hosted on `peer`. Refreshes that peer's
    /// `last_seen` and updates the role **last-write-wins** — roles
    /// change via governance (promote/demote), so the most recent
    /// observation is the one to trust.
    pub(crate) fn record(
        &mut self,
        group: ContextGroupId,
        identity: PublicKey,
        peer: PeerId,
        role: GroupMemberRole,
        now_secs: u64,
    ) {
        let member = self
            .groups
            .entry(group)
            .or_default()
            .members
            .entry(identity)
            .or_insert_with(|| MemberHosts {
                role: role.clone(),
                peers: BTreeMap::new(),
            });
        member.role = role;
        let _ = member.peers.insert(peer, now_secs);
    }

    /// Fresh members of `group` for dial selection: each identity with
    /// its role and the peers (seen within `ttl_secs`) hosting it. A
    /// member whose every hosting peer has aged out is omitted entirely.
    pub(crate) fn members_for_group(
        &self,
        group: &ContextGroupId,
        now_secs: u64,
        ttl_secs: u64,
    ) -> Vec<ResolvedMember> {
        let Some(g) = self.groups.get(group) else {
            return Vec::new();
        };
        g.members
            .iter()
            .filter_map(|(identity, hosts)| {
                let peers: Vec<PeerId> = hosts
                    .peers
                    .iter()
                    .filter(|(_, &last_seen)| is_fresh(last_seen, now_secs, ttl_secs))
                    .map(|(peer, _)| *peer)
                    .collect();
                (!peers.is_empty()).then(|| ResolvedMember {
                    identity: *identity,
                    role: hosts.role.clone(),
                    peers,
                })
            })
            .collect()
    }

    /// Identities observed on `peer` across **all** groups — the reverse
    /// direction a future selection refinement can use to flag whether a
    /// discovered peer hosts an anchor identity, straight from the durable
    /// cache. No freshness filter: the caller intersects this with an
    /// authoritative anchor set, so a slightly stale identity can only
    /// fail to match, never mis-match.
    ///
    /// Not yet wired — selection currently reads the in-memory
    /// `peer_identities` reverse view for this; retained (and tested) as
    /// the cache-backed counterpart for when that path moves to the cache.
    #[allow(dead_code)]
    pub(crate) fn identities_for_peer(&self, peer: &PeerId) -> BTreeSet<PublicKey> {
        let mut out = BTreeSet::new();
        for g in self.groups.values() {
            for (identity, hosts) in &g.members {
                if hosts.peers.contains_key(peer) {
                    let _ = out.insert(*identity);
                }
            }
        }
        out
    }

    /// Serialize one group's still-fresh entries for persistence — the
    /// RocksDB value stored under that group's key, one row per
    /// `(identity, peer)` pair.
    pub(crate) fn to_persisted(
        &self,
        group: &ContextGroupId,
        now_secs: u64,
        ttl_secs: u64,
    ) -> Vec<PersistedIdentityPeer> {
        let Some(g) = self.groups.get(group) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for (identity, hosts) in &g.members {
            for (peer, &last_seen) in &hosts.peers {
                if is_fresh(last_seen, now_secs, ttl_secs) {
                    out.push(PersistedIdentityPeer {
                        identity: identity.to_string(),
                        peer_id: peer.to_base58(),
                        role: hosts.role.clone(),
                        last_seen_secs: last_seen,
                    });
                }
            }
        }
        out
    }

    /// The groups currently holding at least one entry — the set the node
    /// layer iterates to snapshot per-group blobs.
    pub(crate) fn groups(&self) -> impl Iterator<Item = &ContextGroupId> {
        self.groups.keys()
    }

    /// Load one group's bucket from a persisted snapshot, parsing the
    /// string ids and dropping any row that is malformed or past
    /// `ttl_secs`. Replaces any existing bucket for `group`; if nothing
    /// survives, the group is left absent rather than stored empty.
    ///
    /// A malformed row is logged at debug and skipped rather than failing
    /// the whole load — one corrupt entry shouldn't lose the rest.
    pub(crate) fn load_group_from_persisted(
        &mut self,
        group: ContextGroupId,
        records: Vec<PersistedIdentityPeer>,
        now_secs: u64,
        ttl_secs: u64,
    ) {
        let mut members: BTreeMap<PublicKey, MemberHosts> = BTreeMap::new();
        for r in records {
            if !is_fresh(r.last_seen_secs, now_secs, ttl_secs) {
                continue;
            }
            let identity = match r.identity.parse::<PublicKey>() {
                Ok(identity) => identity,
                Err(err) => {
                    debug!(identity = %r.identity, ?err, "skipping unparseable cached member identity");
                    continue;
                }
            };
            let peer = match r.peer_id.parse::<PeerId>() {
                Ok(peer) => peer,
                Err(err) => {
                    debug!(peer_id = %r.peer_id, ?err, "skipping unparseable cached peer id");
                    continue;
                }
            };
            let entry = members.entry(identity).or_insert_with(|| MemberHosts {
                role: r.role.clone(),
                peers: BTreeMap::new(),
            });
            // Every row written for a given identity carries that
            // identity's single role (see `to_persisted`), so this
            // assignment is value-stable regardless of row order — it
            // isn't a meaningful last-write race.
            entry.role = r.role;
            let _ = entry.peers.insert(peer, r.last_seen_secs);
        }
        if members.is_empty() {
            let _ = self.groups.remove(&group);
        } else {
            let _ = self.groups.insert(group, GroupMembers { members });
        }
    }
}

/// The role a peer's identity was observed holding in a group, paired
/// with the group, as passed to [`PeerIdentityCache::record`] from the
/// authenticated observation gate. `None` at a call site that can't
/// cheaply resolve the membership means the observation updates only the
/// in-memory reverse view, not the persistent cache.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ObservedMembership {
    pub(crate) group_id: ContextGroupId,
    pub(crate) role: GroupMemberRole,
}

/// One group's persisted bucket: the group id (hex of its 32 bytes,
/// since `ContextGroupId` has no `Display`) and its `(identity, peer)`
/// rows.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct PersistedGroup {
    pub(crate) group_id: String,
    pub(crate) entries: Vec<PersistedIdentityPeer>,
}

/// Current on-disk schema version. Bump on any incompatible change to
/// the persisted format; `load_all_from_persisted` discards a blob whose
/// version it doesn't recognise (rather than deserializing garbage),
/// leaving the cache to refill from live traffic.
pub(crate) const PERSIST_SCHEMA_VERSION: u32 = 1;

fn current_schema_version() -> u32 {
    PERSIST_SCHEMA_VERSION
}

/// Whole-cache on-disk form: every non-empty group bucket. Stored as a
/// single blob under one `Generic` key (like `PeerAddrCache`) — the
/// per-group structure lives *inside* the blob, which keeps load/snapshot
/// to one get/put and avoids per-group key enumeration and stale-key
/// pruning.
///
/// `version` guards forward/backward compatibility: a blob written by an
/// incompatible future schema is detected and dropped on load. Blobs
/// written before this field existed deserialize with `version = 1` (the
/// current format), so existing caches load unchanged.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct PersistedPeerIdentityCache {
    #[serde(default = "current_schema_version")]
    pub(crate) version: u32,
    pub(crate) groups: Vec<PersistedGroup>,
}

impl PeerIdentityCache {
    /// Serialize every group's still-fresh entries into the single-blob
    /// form. Groups with no fresh rows are omitted so the blob shrinks as
    /// members age out.
    pub(crate) fn to_persisted_all(
        &self,
        now_secs: u64,
        ttl_secs: u64,
    ) -> PersistedPeerIdentityCache {
        let groups = self
            .groups()
            .filter_map(|group| {
                let entries = self.to_persisted(group, now_secs, ttl_secs);
                (!entries.is_empty()).then(|| PersistedGroup {
                    group_id: hex::encode(group.to_bytes()),
                    entries,
                })
            })
            .collect();
        PersistedPeerIdentityCache {
            version: PERSIST_SCHEMA_VERSION,
            groups,
        }
    }

    /// Rebuild a cache from the single-blob form, dropping groups whose id
    /// is unparseable and (per group) rows that are malformed or past
    /// `ttl_secs`. A bad group id is logged at debug and skipped rather
    /// than failing the whole load.
    pub(crate) fn load_all_from_persisted(
        blob: PersistedPeerIdentityCache,
        now_secs: u64,
        ttl_secs: u64,
    ) -> Self {
        if blob.version != PERSIST_SCHEMA_VERSION {
            debug!(
                found = blob.version,
                expected = PERSIST_SCHEMA_VERSION,
                "peer-identity cache blob has an unrecognised schema version; ignoring it"
            );
            return Self::default();
        }
        let mut cache = Self::default();
        for g in blob.groups {
            let Some(group) = parse_group_id(&g.group_id) else {
                debug!(group_id = %g.group_id, "skipping unparseable cached group id");
                continue;
            };
            cache.load_group_from_persisted(group, g.entries, now_secs, ttl_secs);
        }
        cache
    }

    /// Drop a member from a group's bucket — invoked when governance
    /// emits `MemberRemoved`, so the removed identity stops being
    /// preferred for sync (and stops being re-persisted) without waiting
    /// for TTL. If the group's bucket empties, the group is removed.
    ///
    /// Only the cache's per-group *membership* view is touched; the
    /// `peer_identities` reverse view is left intact, because the peer
    /// still controls that identity — removal changes group membership,
    /// not key ownership, and anchor status is re-derived from the
    /// now-updated governance `trusted_anchors`.
    pub(crate) fn remove_member(&mut self, group: &ContextGroupId, identity: &PublicKey) {
        let now_empty = match self.groups.get_mut(group) {
            Some(g) => {
                let _ = g.members.remove(identity);
                g.members.is_empty()
            }
            None => false,
        };
        if now_empty {
            let _ = self.groups.remove(group);
        }
    }

    /// All `(peer, identity)` pairs currently held, across every group —
    /// used to hydrate the in-memory reverse view (`peer_identities`)
    /// after a load. No freshness filter: a load has already pruned stale
    /// rows, and the reverse view is intersected with an authoritative
    /// anchor set downstream.
    pub(crate) fn all_peer_identity_pairs(&self) -> Vec<(PeerId, PublicKey)> {
        // De-duplicate: the same `(peer, identity)` can be recorded in
        // several groups (an identity that's a member of both a namespace
        // root and a subgroup), and the caller would otherwise redo the
        // insert and over-count the hydration log.
        let mut out = BTreeSet::new();
        for g in self.groups.values() {
            for (identity, hosts) in &g.members {
                for peer in hosts.peers.keys() {
                    let _ = out.insert((*peer, *identity));
                }
            }
        }
        out.into_iter().collect()
    }
}

/// Parse a hex-encoded 32-byte group id back into a [`ContextGroupId`].
fn parse_group_id(s: &str) -> Option<ContextGroupId> {
    let bytes = hex::decode(s).ok()?;
    let arr: [u8; 32] = bytes.try_into().ok()?;
    Some(ContextGroupId::from(arr))
}

/// `last_seen_secs` is within `ttl_secs` of `now_secs`. A backwards clock
/// jump (`now < last_seen`) counts as fresh — better to keep a
/// possibly-good entry than drop it on a clock glitch.
fn is_fresh(last_seen_secs: u64, now_secs: u64, ttl_secs: u64) -> bool {
    now_secs.saturating_sub(last_seen_secs) <= ttl_secs
}

#[cfg(test)]
mod tests {
    use super::*;

    fn group(n: u8) -> ContextGroupId {
        ContextGroupId::from([n; 32])
    }

    fn pk(n: u8) -> PublicKey {
        PublicKey::from([n; 32])
    }

    fn peer(n: u8) -> PeerId {
        let kp = libp2p::identity::Keypair::ed25519_from_bytes([n; 32]).expect("seed");
        PeerId::from_public_key(&kp.public())
    }

    #[test]
    fn record_inserts_and_refreshes_last_seen() {
        let mut c = PeerIdentityCache::default();
        c.record(group(1), pk(1), peer(1), GroupMemberRole::Member, 100);

        let members = c.members_for_group(&group(1), 100, 1000);
        assert_eq!(members.len(), 1);
        assert_eq!(members[0].identity, pk(1));
        assert_eq!(members[0].peers, vec![peer(1)]);

        // Re-record later bumps last_seen so the entry stays fresh past
        // when the original stamp would have aged out.
        c.record(group(1), pk(1), peer(1), GroupMemberRole::Member, 900);
        assert_eq!(
            c.members_for_group(&group(1), 1800, 1000).len(),
            1,
            "refreshed last_seen keeps the entry fresh"
        );
    }

    #[test]
    fn record_updates_role_last_write_wins() {
        let mut c = PeerIdentityCache::default();
        c.record(group(1), pk(1), peer(1), GroupMemberRole::Member, 100);
        // Promotion observed later must overwrite the stale role.
        c.record(group(1), pk(1), peer(1), GroupMemberRole::Admin, 200);

        let members = c.members_for_group(&group(1), 200, 1000);
        assert_eq!(members[0].role, GroupMemberRole::Admin);
    }

    #[test]
    fn one_identity_hosted_on_multiple_peers() {
        let mut c = PeerIdentityCache::default();
        c.record(group(1), pk(1), peer(1), GroupMemberRole::ReadOnlyTee, 100);
        c.record(group(1), pk(1), peer(2), GroupMemberRole::ReadOnlyTee, 100);

        let members = c.members_for_group(&group(1), 100, 1000);
        assert_eq!(members.len(), 1, "still one identity");
        // Order is by PeerId bytes (BTreeMap), not seed order — compare
        // as a set.
        assert_eq!(
            members[0].peers.iter().copied().collect::<BTreeSet<_>>(),
            BTreeSet::from([peer(1), peer(2)]),
            "both hosting peers returned"
        );
    }

    #[test]
    fn one_peer_hosts_multiple_identities() {
        let mut c = PeerIdentityCache::default();
        c.record(group(1), pk(1), peer(1), GroupMemberRole::Admin, 100);
        c.record(group(1), pk(2), peer(1), GroupMemberRole::Member, 100);

        let ids = c.identities_for_peer(&peer(1));
        assert_eq!(ids, BTreeSet::from([pk(1), pk(2)]));
    }

    #[test]
    fn identities_for_peer_spans_groups() {
        let mut c = PeerIdentityCache::default();
        c.record(group(1), pk(1), peer(1), GroupMemberRole::Admin, 100);
        c.record(group(2), pk(2), peer(1), GroupMemberRole::Member, 100);

        // The reverse index is intentionally group-agnostic: a peer's
        // anchor-ness is judged against an anchor set the caller supplies.
        assert_eq!(
            c.identities_for_peer(&peer(1)),
            BTreeSet::from([pk(1), pk(2)])
        );
    }

    #[test]
    fn groups_are_independent() {
        let mut c = PeerIdentityCache::default();
        c.record(group(1), pk(1), peer(1), GroupMemberRole::Admin, 100);

        assert_eq!(c.members_for_group(&group(1), 100, 1000).len(), 1);
        assert!(
            c.members_for_group(&group(2), 100, 1000).is_empty(),
            "a record in one group must not leak into another"
        );
    }

    #[test]
    fn member_dropped_when_all_hosts_stale() {
        let mut c = PeerIdentityCache::default();
        c.record(group(1), pk(1), peer(1), GroupMemberRole::Member, 100);
        // now=2000, ttl=1000 → 1900 > 1000 → the only host is stale.
        assert!(
            c.members_for_group(&group(1), 2000, 1000).is_empty(),
            "member with no fresh host is omitted"
        );
        // within TTL → kept.
        assert_eq!(c.members_for_group(&group(1), 900, 1000).len(), 1);
    }

    #[test]
    fn members_for_group_drops_only_stale_hosts() {
        let mut c = PeerIdentityCache::default();
        c.record(group(1), pk(1), peer(1), GroupMemberRole::Member, 100);
        c.record(group(1), pk(1), peer(2), GroupMemberRole::Member, 1900);

        // now=2000, ttl=1000: peer(1)@100 is stale, peer(2)@1900 fresh.
        let members = c.members_for_group(&group(1), 2000, 1000);
        assert_eq!(members.len(), 1);
        assert_eq!(
            members[0].peers,
            vec![peer(2)],
            "only the fresh host survives"
        );
    }

    #[test]
    fn to_persisted_filters_stale_pairs() {
        let mut c = PeerIdentityCache::default();
        c.record(group(1), pk(1), peer(1), GroupMemberRole::Member, 100);
        c.record(group(1), pk(1), peer(2), GroupMemberRole::Member, 1900);

        let rows = c.to_persisted(&group(1), 2000, 1000);
        assert_eq!(rows.len(), 1, "stale (identity, peer) pair excluded");
        assert_eq!(rows[0].peer_id, peer(2).to_base58());
    }

    #[test]
    fn persisted_round_trips_through_strings() {
        let mut c = PeerIdentityCache::default();
        c.record(group(1), pk(1), peer(1), GroupMemberRole::Admin, 100);
        c.record(group(1), pk(1), peer(2), GroupMemberRole::Admin, 150);
        c.record(group(1), pk(2), peer(3), GroupMemberRole::ReadOnlyTee, 150);

        let rows = c.to_persisted(&group(1), 150, 1000);
        // JSON serialize/deserialize as the node-side persistence would.
        let json = serde_json::to_string(&rows).expect("serialize");
        let back: Vec<PersistedIdentityPeer> = serde_json::from_str(&json).expect("deserialize");

        let mut restored = PeerIdentityCache::default();
        restored.load_group_from_persisted(group(1), back, 150, 1000);

        let members = restored.members_for_group(&group(1), 150, 1000);
        assert_eq!(members.len(), 2);
        let admin = members.iter().find(|m| m.identity == pk(1)).expect("pk1");
        assert_eq!(admin.role, GroupMemberRole::Admin);
        assert_eq!(
            admin.peers.iter().copied().collect::<BTreeSet<_>>(),
            BTreeSet::from([peer(1), peer(2)]),
            "both hosts survive the round trip"
        );
        let tee = members.iter().find(|m| m.identity == pk(2)).expect("pk2");
        assert_eq!(tee.role, GroupMemberRole::ReadOnlyTee);
    }

    #[test]
    fn load_group_skips_malformed_and_expired() {
        let records = vec![
            PersistedIdentityPeer {
                identity: "not-a-public-key".to_owned(),
                peer_id: peer(1).to_base58(),
                role: GroupMemberRole::Member,
                last_seen_secs: 100,
            },
            PersistedIdentityPeer {
                identity: pk(2).to_string(),
                peer_id: "not-a-peer-id".to_owned(),
                role: GroupMemberRole::Member,
                last_seen_secs: 100,
            },
            PersistedIdentityPeer {
                identity: pk(3).to_string(),
                peer_id: peer(3).to_base58(),
                role: GroupMemberRole::Member,
                last_seen_secs: 50, // now=2000, ttl=1000 → expired
            },
            PersistedIdentityPeer {
                identity: pk(4).to_string(),
                peer_id: peer(4).to_base58(),
                role: GroupMemberRole::Admin,
                last_seen_secs: 1900, // fresh
            },
        ];
        let mut c = PeerIdentityCache::default();
        c.load_group_from_persisted(group(1), records, 2000, 1000);

        let members = c.members_for_group(&group(1), 2000, 1000);
        assert_eq!(members.len(), 1, "only the well-formed, fresh row survives");
        assert_eq!(members[0].identity, pk(4));
        assert_eq!(members[0].role, GroupMemberRole::Admin);
    }

    #[test]
    fn load_group_leaves_group_absent_when_nothing_survives() {
        let records = vec![PersistedIdentityPeer {
            identity: pk(1).to_string(),
            peer_id: peer(1).to_base58(),
            role: GroupMemberRole::Member,
            last_seen_secs: 50, // expired at now=2000/ttl=1000
        }];
        let mut c = PeerIdentityCache::default();
        c.load_group_from_persisted(group(1), records, 2000, 1000);

        assert_eq!(c.groups().count(), 0, "empty load stores no group");
    }

    #[test]
    fn groups_lists_seeded_groups() {
        let mut c = PeerIdentityCache::default();
        c.record(group(1), pk(1), peer(1), GroupMemberRole::Member, 100);
        c.record(group(3), pk(2), peer(2), GroupMemberRole::Member, 100);

        let listed: BTreeSet<ContextGroupId> = c.groups().copied().collect();
        assert_eq!(listed, BTreeSet::from([group(1), group(3)]));
    }

    #[test]
    fn whole_blob_round_trips_across_groups() {
        let mut c = PeerIdentityCache::default();
        c.record(group(1), pk(1), peer(1), GroupMemberRole::Admin, 100);
        c.record(group(2), pk(2), peer(2), GroupMemberRole::ReadOnlyTee, 100);

        let blob = c.to_persisted_all(100, 1000);
        // JSON serialize/deserialize as the node-side persistence would.
        let json = serde_json::to_string(&blob).expect("serialize");
        let back: PersistedPeerIdentityCache = serde_json::from_str(&json).expect("deserialize");
        let restored = PeerIdentityCache::load_all_from_persisted(back, 100, 1000);

        assert_eq!(
            restored.groups().copied().collect::<BTreeSet<_>>(),
            BTreeSet::from([group(1), group(2)]),
            "both groups survive the whole-blob round trip"
        );
        assert_eq!(
            restored.members_for_group(&group(1), 100, 1000)[0].role,
            GroupMemberRole::Admin
        );
        assert_eq!(
            restored.members_for_group(&group(2), 100, 1000)[0].role,
            GroupMemberRole::ReadOnlyTee
        );
    }

    #[test]
    fn to_persisted_all_omits_fully_stale_groups() {
        let mut c = PeerIdentityCache::default();
        c.record(group(1), pk(1), peer(1), GroupMemberRole::Member, 100); // stale at now=2000
        c.record(group(2), pk(2), peer(2), GroupMemberRole::Member, 1900); // fresh

        let blob = c.to_persisted_all(2000, 1000);
        assert_eq!(blob.groups.len(), 1, "the all-stale group is dropped");
        assert_eq!(
            blob.groups[0].group_id,
            hex::encode(group(2).to_bytes()),
            "only the fresh group persists"
        );
    }

    #[test]
    fn load_all_skips_unparseable_group_id() {
        let blob = PersistedPeerIdentityCache {
            version: PERSIST_SCHEMA_VERSION,
            groups: vec![
                PersistedGroup {
                    group_id: "not-hex".to_owned(),
                    entries: vec![PersistedIdentityPeer {
                        identity: pk(1).to_string(),
                        peer_id: peer(1).to_base58(),
                        role: GroupMemberRole::Member,
                        last_seen_secs: 100,
                    }],
                },
                PersistedGroup {
                    group_id: hex::encode(group(2).to_bytes()),
                    entries: vec![PersistedIdentityPeer {
                        identity: pk(2).to_string(),
                        peer_id: peer(2).to_base58(),
                        role: GroupMemberRole::Member,
                        last_seen_secs: 100,
                    }],
                },
            ],
        };
        let c = PeerIdentityCache::load_all_from_persisted(blob, 100, 1000);
        assert_eq!(
            c.groups().copied().collect::<BTreeSet<_>>(),
            BTreeSet::from([group(2)]),
            "only the well-formed group id loads"
        );
    }

    #[test]
    fn load_all_discards_blob_with_unrecognised_version() {
        let mut c = PeerIdentityCache::default();
        c.record(group(1), pk(1), peer(1), GroupMemberRole::Admin, 100);
        let mut blob = c.to_persisted_all(100, 1000);
        assert_eq!(blob.version, PERSIST_SCHEMA_VERSION);
        // Simulate a future, incompatible schema.
        blob.version = PERSIST_SCHEMA_VERSION + 1;

        let restored = PeerIdentityCache::load_all_from_persisted(blob, 100, 1000);
        assert_eq!(
            restored.groups().count(),
            0,
            "a blob with an unrecognised version is discarded, not misread"
        );
    }

    #[test]
    fn all_peer_identity_pairs_dedupes_same_identity_across_groups() {
        let mut c = PeerIdentityCache::default();
        // Same (peer, identity) recorded in two groups — must appear once.
        c.record(group(1), pk(1), peer(1), GroupMemberRole::Admin, 100);
        c.record(group(2), pk(1), peer(1), GroupMemberRole::Member, 100);

        let pairs = c.all_peer_identity_pairs();
        assert_eq!(pairs, vec![(peer(1), pk(1))], "duplicate pair collapsed");
    }

    #[test]
    fn remove_member_drops_identity_and_empties_group() {
        let mut c = PeerIdentityCache::default();
        c.record(group(1), pk(1), peer(1), GroupMemberRole::Admin, 100);
        c.record(group(1), pk(2), peer(2), GroupMemberRole::Member, 100);

        c.remove_member(&group(1), &pk(1));
        let members = c.members_for_group(&group(1), 100, 1000);
        assert_eq!(members.len(), 1);
        assert_eq!(
            members[0].identity,
            pk(2),
            "only the removed member is gone"
        );

        // Removing the last member drops the group bucket entirely.
        c.remove_member(&group(1), &pk(2));
        assert_eq!(c.groups().count(), 0, "emptied group is removed");

        // Removing from an absent group is a no-op.
        c.remove_member(&group(2), &pk(9));
    }

    #[test]
    fn all_peer_identity_pairs_spans_groups() {
        let mut c = PeerIdentityCache::default();
        c.record(group(1), pk(1), peer(1), GroupMemberRole::Admin, 100);
        c.record(group(2), pk(2), peer(1), GroupMemberRole::Member, 100);

        let pairs: BTreeSet<(PeerId, PublicKey)> =
            c.all_peer_identity_pairs().into_iter().collect();
        assert_eq!(
            pairs,
            BTreeSet::from([(peer(1), pk(1)), (peer(1), pk(2))]),
            "hydration pairs cover every group"
        );
    }
}
