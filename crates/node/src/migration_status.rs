//! In-memory TTL cache of per-(namespace, peer) migration heartbeats.
//!
//! PR-6c Task 6c.8. Modeled directly on [`crate::readiness::ReadinessCache`]:
//! a `Mutex<BTreeMap<(namespace, peer), CacheEntry>>` with TTL fresh-peers
//! filtering and opportunistic eviction, populated by verified-on-receive
//! heartbeats and never persisted.
//!
//! A [`SignedMigrationHeartbeat`] is purely observability telemetry — signed
//! TTL gossip a node publishes on the namespace topic to advertise its loaded
//! `schema_version` and remaining unconverted residue. It is NOT replicated
//! governance state and must never be treated as a migration gate; the
//! `get_migration_status` rollup (Task 6c.9) reads this cache to report
//! per-cohort completion, and a member with no fresh heartbeat is `unknown`
//! (never silently "migrated").
//!
//! **Verification contract**: [`MigrationStatusCache::insert`] assumes the
//! heartbeat has already been verified for signature AND namespace membership
//! by the caller. The single legitimate caller is the receiver-side
//! `network_event::namespace` dispatch, which calls
//! `calimero_context::governance_broadcast::verify_migration_heartbeat`
//! (sig + member-set check) BEFORE invoking `insert`. Putting verification
//! inside `insert` would couple the cache to `&Store`, drag
//! namespace-membership state into this module, and duplicate work since the
//! receiver gate runs first — exactly the split the readiness cache uses.
use std::collections::{BTreeMap, HashMap};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use actix::{Actor, AsyncContext, Context, Handler, Message};
use calimero_context::group_store::NamespaceRepository;
use calimero_context_client::local_governance::{NamespaceTopicMsg, SignedMigrationHeartbeat};
use calimero_node_primitives::client::NodeClient;
use calimero_node_primitives::sync::BroadcastMessage;
use calimero_primitives::identity::PublicKey;
use calimero_store::Store;
use zeroize::Zeroize;

/// Default freshness window for a cached heartbeat. A node beats on-change
/// and on a low-frequency periodic tick; 60s matches the readiness cache's
/// TTL so a member that stopped reporting drops to `unknown` rather than
/// pinning a stale "migrated" claim.
pub const DEFAULT_HEARTBEAT_TTL: Duration = Duration::from_secs(60);

/// Maximum tolerated drift between a heartbeat's `ts_millis` and local
/// wall-clock. Mirrors [`crate::readiness::MAX_BEACON_CLOCK_DRIFT_MS`]:
/// heartbeats claiming a wall-clock more than this far in the future are
/// rejected to close the cache-poisoning vector documented on
/// [`MigrationStatusCache::insert`].
pub const MAX_HEARTBEAT_CLOCK_DRIFT_MS: u64 = 60_000;

/// Per-(namespace, peer) snapshot of the most recent fresh heartbeat we
/// have received from that peer.
#[derive(Debug, Clone)]
pub struct CacheEntry {
    /// Schema/binary version the peer has loaded.
    pub schema_version: u32,
    /// Unconverted Convergent ("auto") contexts the peer still has pending.
    pub residue_auto: u64,
    /// Unconverted identity-gated entries the peer still has pending
    /// (the peer's local-derived residue scan from Task 6c.6).
    pub residue_identity: u64,
    /// Governance HLC the peer has synced/applied through.
    pub synced_up_to_hlc: u64,
    /// Peer-signed millis-since-epoch from the heartbeat itself.
    /// Authoritative per-peer ordering signal — used by `insert` to drop
    /// stale heartbeats that gossipsub may re-deliver out-of-order on mesh
    /// churn / peer reconnect.
    pub ts_millis: u64,
    /// Local receive instant — the TTL freshness reference.
    pub received_at: Instant,
}

/// Per-namespace, per-peer migration-heartbeat cache.
///
/// Uses `BTreeMap` (not `HashMap`) because
/// `calimero_primitives::identity::PublicKey` derives `Ord` but not `Hash`
/// — identical rationale to [`crate::readiness::ReadinessCache`]. Lookups
/// are O(log n) on a per-namespace map holding at most one entry per peer
/// (the practical n is the namespace member count).
#[derive(Debug, Default)]
pub struct MigrationStatusCache {
    entries: Mutex<BTreeMap<([u8; 32], PublicKey), CacheEntry>>,
}

impl MigrationStatusCache {
    /// Acquire the entries map, recovering from a poisoned mutex.
    ///
    /// A `PoisonError` only happens if a previous holder panicked while the
    /// guard was live; the BTreeMap's invariants are not at risk here, so
    /// continuing with the inner guard via `into_inner()` is strictly
    /// preferable to permanently DoSing the migration-status subsystem on
    /// the first transient panic. Mirrors `ReadinessCache::entries_lock`.
    fn entries_lock(
        &self,
    ) -> std::sync::MutexGuard<'_, BTreeMap<([u8; 32], PublicKey), CacheEntry>> {
        self.entries
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Insert a heartbeat into the cache.
    ///
    /// **Verification contract**: this method assumes the heartbeat has
    /// already been verified for signature AND namespace membership by the
    /// caller (see the module docs). The receiver-side dispatch runs
    /// `verify_migration_heartbeat` before calling this.
    ///
    /// Insert iff the incoming heartbeat is *newer* than any cached entry
    /// from the same peer (by `ts_millis`, with `synced_up_to_hlc` as a
    /// tiebreaker on clock equality). Gossipsub does not guarantee delivery
    /// order — without this filter an older re-delivered heartbeat could
    /// overwrite a fresher one, regressing a peer's reported residue.
    ///
    /// Also rejects heartbeats with `ts_millis` more than
    /// [`MAX_HEARTBEAT_CLOCK_DRIFT_MS`] ahead of local wall-clock. Without
    /// this bound a malicious or clock-skewed member could sign a heartbeat
    /// with `ts_millis = year 2100`, poisoning their cache entry: every
    /// subsequent legitimate heartbeat would be dropped by the
    /// `older-than-existing` filter, freezing the reported residue at
    /// attacker-chosen values indefinitely.
    ///
    /// Opportunistically evicts entries past `2 × MAX_HEARTBEAT_CLOCK_DRIFT_MS`
    /// for *this namespace* on every insert — keeps long-lived nodes from
    /// accumulating entries from peers that left the namespace.
    /// Stale-but-within-eviction-window entries are still filtered out of
    /// `fresh_peers` by the per-call `ttl` check.
    pub fn insert(&self, hb: &SignedMigrationHeartbeat) {
        // Wall-clock sanity bound — reject far-future ts_millis.
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        if hb.ts_millis > now_ms.saturating_add(MAX_HEARTBEAT_CLOCK_DRIFT_MS) {
            return;
        }

        let now = Instant::now();
        let mut g = self.entries_lock();
        let key = (hb.namespace_id, hb.peer_pubkey);
        if let Some(existing) = g.get(&key) {
            // Drop the heartbeat if it's older or equal-clock-but-not-fresher.
            if hb.ts_millis < existing.ts_millis
                || (hb.ts_millis == existing.ts_millis
                    && hb.synced_up_to_hlc <= existing.synced_up_to_hlc)
            {
                return;
            }
        }

        // Opportunistic eviction for the same namespace — keep the BTreeMap
        // from accumulating entries from peers that left. Eviction window
        // (`2 × MAX_HEARTBEAT_CLOCK_DRIFT_MS`) is intentionally wider than
        // typical TTLs so reads can still see "stale-but-recent" entries
        // (filtered by per-call `ttl`) without competing against this prune.
        let evict_window = Duration::from_millis(MAX_HEARTBEAT_CLOCK_DRIFT_MS.saturating_mul(2));
        g.retain(|(ns, _), entry| {
            *ns != hb.namespace_id || now.duration_since(entry.received_at) <= evict_window
        });

        let _ = g.insert(
            key,
            CacheEntry {
                schema_version: hb.schema_version,
                residue_auto: hb.residue_auto,
                residue_identity: hb.residue_identity,
                synced_up_to_hlc: hb.synced_up_to_hlc,
                ts_millis: hb.ts_millis,
                received_at: now,
            },
        );
    }

    /// All peers in `ns` whose most recent heartbeat is fresh within `ttl`.
    ///
    /// Stale entries (past `ttl`) are filtered out — the rollup treats a
    /// member with no fresh entry here as `unknown`, never as migrated.
    pub fn fresh_peers(&self, ns: [u8; 32], ttl: Duration) -> Vec<(PublicKey, CacheEntry)> {
        let g = self.entries_lock();
        let now = Instant::now();
        g.iter()
            .filter(|((nns, _), e)| *nns == ns && now.duration_since(e.received_at) <= ttl)
            .map(|((_, pk), e)| (*pk, e.clone()))
            .collect()
    }

    /// The freshest heartbeat for a specific `(ns, peer)`, or `None` if the
    /// peer has no entry or its entry is stale past `ttl`.
    ///
    /// Used by the rollup (Task 6c.9) to look up each pinned-cohort member's
    /// reported status. A `None` here maps to `unknown` and keeps
    /// `all_migrated == false`.
    pub fn peer_entry(&self, ns: [u8; 32], peer: PublicKey, ttl: Duration) -> Option<CacheEntry> {
        let g = self.entries_lock();
        let now = Instant::now();
        g.get(&(ns, peer))
            .filter(|e| now.duration_since(e.received_at) <= ttl)
            .cloned()
    }
}

/// The per-namespace migration facts this node advertises in its own
/// heartbeat. Computed locally at emit time and signed into a
/// [`SignedMigrationHeartbeat`] body.
///
/// `residue_identity` is this node's local-derived count of unconverted
/// identity-gated entries (Task 6c.6); `residue_auto` is the matching count
/// for Convergent contexts (the 6a marker). A node reporting both at 0 with
/// `schema_version >= target` is what a rollup reads as "this member migrated".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MigrationFacts {
    pub schema_version: u32,
    pub residue_auto: u64,
    pub residue_identity: u64,
    pub synced_up_to_hlc: u64,
}

/// Decide whether the local node should emit an *on-change* heartbeat for a
/// namespace, given the facts carried by the last heartbeat it emitted (if
/// any) and the freshly-computed facts.
///
/// Mirrors the readiness "edge-trigger on tier transition" pattern: a peer
/// should re-advertise immediately when its *reported state* changes —
/// here when `schema_version`, `residue_auto`, or `residue_identity` flips
/// — rather than waiting up to a full periodic interval. `synced_up_to_hlc`
/// is intentionally NOT an edge trigger: it advances on every applied op and
/// would defeat the purpose of debouncing to the periodic tick; the periodic
/// beat carries its latest value.
#[must_use]
pub fn should_emit_on_change(last: Option<MigrationFacts>, current: MigrationFacts) -> bool {
    match last {
        None => true,
        Some(prev) => {
            prev.schema_version != current.schema_version
                || prev.residue_auto != current.residue_auto
                || prev.residue_identity != current.residue_identity
        }
    }
}

/// Fold a freshly-computed `facts` for `namespace_id` into the per-namespace
/// `last_emitted` carry-forward map and report whether this is an *edge* that
/// warrants an immediate (out-of-cycle) emit.
///
/// Pure (no store / network): the actor handler calls this AFTER overlaying the
/// real `synced_up_to_hlc` (`refresh_hlc`) and BEFORE publishing. It always
/// records the latest facts — seeding the namespace into `last_emitted` on
/// first sight — so the periodic keep-alive tick has a non-empty working set
/// once the node has signalled at least one namespace (the seam that makes the
/// emitter live in production; before this the map could only ever stay empty).
/// The returned bool is the [`should_emit_on_change`] edge decision computed
/// against the *prior* recorded facts.
pub fn record_facts_update(
    last_emitted: &mut HashMap<[u8; 32], MigrationFacts>,
    namespace_id: [u8; 32],
    facts: MigrationFacts,
) -> bool {
    let prior = last_emitted.get(&namespace_id).copied();
    let emit = should_emit_on_change(prior, facts);
    let _ = last_emitted.insert(namespace_id, facts);
    emit
}

/// Build and sign a [`SignedMigrationHeartbeat`] for `namespace_id` over the
/// canonical `MIGRATION_HEARTBEAT_SIGN_DOMAIN || borsh(body)` payload.
///
/// Pure: takes the signing identity and the locally-computed facts and
/// returns the signed wire message. The actor's publish path calls this then
/// hands the bytes to gossipsub; receivers verify via
/// `SignedMigrationHeartbeat::verify_signature` + namespace membership
/// (`verify_migration_heartbeat`). `ts_millis` is the signer's wall-clock at
/// build time — the per-peer ordering signal the cache uses to drop
/// out-of-order re-deliveries.
pub fn build_signed_heartbeat(
    signer_sk: &calimero_primitives::identity::PrivateKey,
    namespace_id: [u8; 32],
    facts: MigrationFacts,
    ts_millis: u64,
) -> Result<SignedMigrationHeartbeat, calimero_context_client::local_governance::GovernanceError> {
    let peer_pubkey = signer_sk.public_key();
    let mut hb = SignedMigrationHeartbeat {
        namespace_id,
        peer_pubkey,
        schema_version: facts.schema_version,
        residue_auto: facts.residue_auto,
        residue_identity: facts.residue_identity,
        synced_up_to_hlc: facts.synced_up_to_hlc,
        ts_millis,
        signature: [0u8; 64],
    };
    let signable = hb.signable_bytes()?;
    let signature = signer_sk.sign(&signable)?.to_bytes();
    hb.signature = signature;
    Ok(hb)
}

/// Default low-frequency interval at which the emitter re-publishes each
/// namespace's heartbeat even when nothing changed, so a member that joined
/// the topic late still learns our status within one period. Edge-trigger
/// emits (residue / schema change) fire out-of-band; this is the keep-alive
/// floor. 30s — well under the 60s [`DEFAULT_HEARTBEAT_TTL`] so a steady
/// peer never lapses to `unknown` between beats.
pub const DEFAULT_EMIT_INTERVAL: Duration = Duration::from_secs(30);

/// Per-namespace migration-heartbeat emitter.
///
/// The publish twin of the ingest [`MigrationStatusCache`]: signs and
/// publishes this node's own [`SignedMigrationHeartbeat`] on the namespace
/// topic, both periodically (keep-alive) and on-change (residue / schema
/// flips). Modeled on [`crate::readiness::ReadinessManager`]'s beacon
/// emission — best-effort gossip, never blocking, never a gate.
///
/// Facts are computed at emit time from local state. `synced_up_to_hlc`
/// reads the namespace governance head sequence (the same source readiness
/// uses for `applied_through`); `schema_version`, `residue_auto`, and
/// `residue_identity` are supplied by the node when it triggers an emit and
/// are otherwise carried forward from the last emit (see
/// [`MigrationFactsUpdate`]). A node that has not yet computed residue
/// reports `0` — the honest "nothing pending locally" telemetry.
pub struct MigrationEmitter {
    pub node_client: NodeClient,
    pub datastore: Store,
    pub interval: Duration,
    /// Last facts we emitted per namespace — the on-change reference for
    /// [`should_emit_on_change`] and the carry-forward source for the
    /// periodic tick.
    pub last_emitted: HashMap<[u8; 32], MigrationFacts>,
}

impl Actor for MigrationEmitter {
    type Context = Context<Self>;

    fn started(&mut self, ctx: &mut Self::Context) {
        // Periodic keep-alive re-publish for every namespace we have facts
        // for. Edge-trigger emits arrive via `MigrationFactsUpdate`.
        ctx.run_interval(self.interval, |this, _ctx| {
            let ns_ids: Vec<[u8; 32]> = this.last_emitted.keys().copied().collect();
            for ns_id in ns_ids {
                if let Some(facts) = this.last_emitted.get(&ns_id).copied() {
                    let facts = this.refresh_hlc(ns_id, facts);
                    this.last_emitted.insert(ns_id, facts);
                    this.publish_heartbeat(ns_id, facts);
                }
            }
        });
    }
}

/// Tells the emitter the node's freshly-computed migration facts for a
/// namespace. Emits immediately iff [`should_emit_on_change`] reports the
/// reported state changed (residue / schema flip); otherwise the periodic
/// tick carries the value. Sent by the node whenever a governance apply or
/// owner-driven convert may have changed local residue.
#[derive(Message)]
#[rtype(result = "()")]
pub struct MigrationFactsUpdate {
    pub namespace_id: [u8; 32],
    pub facts: MigrationFacts,
}

impl Handler<MigrationFactsUpdate> for MigrationEmitter {
    type Result = ();

    fn handle(&mut self, msg: MigrationFactsUpdate, _ctx: &mut Self::Context) {
        let facts = self.refresh_hlc(msg.namespace_id, msg.facts);
        let last = self.last_emitted.get(&msg.namespace_id).copied();
        if should_emit_on_change(last, facts) {
            self.last_emitted.insert(msg.namespace_id, facts);
            self.publish_heartbeat(msg.namespace_id, facts);
        } else {
            // No edge — still record the carry-forward value so the next
            // periodic beat advertises the latest HLC.
            self.last_emitted.insert(msg.namespace_id, facts);
        }
    }
}

impl MigrationEmitter {
    /// Overlay the namespace governance head's current sequence onto `facts`
    /// as `synced_up_to_hlc`. Read-only; defaults to the incoming value on a
    /// missing head or store error so a transient read never regresses the
    /// reported HLC.
    fn refresh_hlc(&self, ns_id: [u8; 32], mut facts: MigrationFacts) -> MigrationFacts {
        let handle = self.datastore.handle();
        let key = calimero_store::key::NamespaceGovHead::new(ns_id);
        if let Ok(Some(head)) = handle.get(&key) {
            facts.synced_up_to_hlc = head.sequence;
        }
        facts
    }

    /// Sign and publish a [`SignedMigrationHeartbeat`] on the namespace topic.
    ///
    /// Best-effort, mirroring [`crate::readiness::ReadinessManager::publish_beacon`]:
    /// any error (no identity yet, signing failure, no peers subscribed) logs
    /// at debug and returns. The periodic tick retries; an edge-trigger fires
    /// on the next residue/schema change.
    fn publish_heartbeat(&self, ns_id: [u8; 32], facts: MigrationFacts) {
        let group_id = calimero_context_config::types::ContextGroupId::from(ns_id);
        let identity = match NamespaceRepository::new(&self.datastore).identity(&group_id) {
            Ok(Some(id)) => id,
            Ok(None) => return, // No identity for this namespace yet — skip.
            Err(err) => {
                tracing::debug!(?err, ?ns_id, "MigrationHeartbeat: identity load failed");
                return;
            }
        };
        let (_peer_pubkey, mut sk_bytes, mut sender_key) = identity;
        // `sender_key` is unused on this path — zeroize immediately. `sk_bytes`
        // is consumed into `PrivateKey::from(...)`; because `[u8; 32]: Copy`
        // that move leaves a stack copy we zeroize after signing.
        sender_key.zeroize();

        let ts_millis = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);

        let signing_key = calimero_primitives::identity::PrivateKey::from(sk_bytes);
        sk_bytes.zeroize();
        let heartbeat = match build_signed_heartbeat(&signing_key, ns_id, facts, ts_millis) {
            Ok(hb) => hb,
            Err(err) => {
                tracing::debug!(?err, "MigrationHeartbeat: sign failed");
                return;
            }
        };

        let topic = calimero_context::governance_broadcast::ns_topic(ns_id);
        // Wrap in the `BroadcastMessage::NamespaceGovernanceDelta` envelope
        // the receiver decodes on `ns/<id>` topics, then borsh the inner
        // `NamespaceTopicMsg::MigrationHeartbeat`. delta_id/parent_ids are
        // zero/empty — heartbeats are not DAG content.
        let inner = match borsh::to_vec(&NamespaceTopicMsg::MigrationHeartbeat(heartbeat)) {
            Ok(b) => b,
            Err(err) => {
                tracing::debug!(?err, "MigrationHeartbeat: borsh encode (inner) failed");
                return;
            }
        };
        let envelope = BroadcastMessage::NamespaceGovernanceDelta {
            namespace_id: ns_id,
            delta_id: [0u8; 32],
            parent_ids: Vec::new(),
            payload: inner,
        };
        let bytes = match borsh::to_vec(&envelope) {
            Ok(b) => b,
            Err(err) => {
                tracing::debug!(?err, "MigrationHeartbeat: borsh encode (envelope) failed");
                return;
            }
        };

        // Detached publish — bypasses the 10s mesh-wait gate of
        // `publish_on_namespace`; heartbeat emission must not block.
        let net = self.node_client.network_client().clone();
        let log_ns = ns_id;
        let log_schema = facts.schema_version;
        let log_residue = facts.residue_identity;
        actix::spawn(async move {
            match net.publish(topic, bytes).await {
                Ok(_) => tracing::debug!(
                    namespace_id = %hex::encode(log_ns),
                    schema_version = log_schema,
                    residue_identity = log_residue,
                    "migration heartbeat emitted"
                ),
                Err(err) => {
                    tracing::debug!(?err, "MigrationHeartbeat publish failed (non-fatal)");
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use calimero_context_client::local_governance::wire::{
        SignableMigrationHeartbeat, MIGRATION_HEARTBEAT_SIGN_DOMAIN,
    };
    use calimero_primitives::identity::PrivateKey;

    use super::*;

    const NS: [u8; 32] = [42u8; 32];

    /// Build a heartbeat signed by `sk` over the canonical signable bytes.
    fn signed_hb(
        sk: &PrivateKey,
        ns: [u8; 32],
        schema_version: u32,
        residue_auto: u64,
        residue_identity: u64,
        ts_millis: u64,
    ) -> SignedMigrationHeartbeat {
        let peer_pubkey = sk.public_key();
        let body = SignableMigrationHeartbeat {
            namespace_id: ns,
            peer_pubkey,
            schema_version,
            residue_auto,
            residue_identity,
            synced_up_to_hlc: 0,
            ts_millis,
        };
        let mut signable = Vec::new();
        signable.extend_from_slice(MIGRATION_HEARTBEAT_SIGN_DOMAIN);
        signable.extend_from_slice(&borsh::to_vec(&body).unwrap());
        let signature = sk.sign(&signable).unwrap().to_bytes();
        SignedMigrationHeartbeat {
            namespace_id: ns,
            peer_pubkey,
            schema_version,
            residue_auto,
            residue_identity,
            synced_up_to_hlc: 0,
            ts_millis,
            signature,
        }
    }

    #[test]
    fn verified_heartbeat_is_cached_and_readable() {
        let cache = MigrationStatusCache::default();
        let sk = PrivateKey::random(&mut rand::thread_rng());
        let hb = signed_hb(&sk, NS, 2, 0, 0, 0);
        // Caller verifies first; a well-formed heartbeat verifies.
        assert!(hb.verify_signature().is_ok());
        cache.insert(&hb);

        let fresh = cache.fresh_peers(NS, DEFAULT_HEARTBEAT_TTL);
        assert_eq!(fresh.len(), 1, "verified heartbeat must be cached");
        let entry = cache
            .peer_entry(NS, sk.public_key(), DEFAULT_HEARTBEAT_TTL)
            .expect("entry readable by (ns, peer)");
        assert_eq!(entry.schema_version, 2);
        assert_eq!(entry.residue_identity, 0);
    }

    #[test]
    fn wire_verify_signature_rejects_field_substitution() {
        // Documents the WIRE-TYPE contract `insert`'s verification-precondition
        // depends on: `SignedMigrationHeartbeat::verify_signature` covers every
        // signed field, so flipping `residue_identity` after signing breaks it.
        //
        // NOTE: this is NOT the ingest gate. The actual receiver gate is
        // `calimero_context::governance_broadcast::verify_migration_heartbeat`
        // (signature + cohort membership), exercised end-to-end in that crate's
        // `verify_migration_heartbeat_rejects_bad_signature` test — it needs a
        // `&Store` and namespace member-set, neither of which this pure cache
        // unit owns. Here we only pin that the wire type's own signature check
        // (the primitive the gate is built on) catches a mutated field.
        let sk = PrivateKey::random(&mut rand::thread_rng());
        let mut hb = signed_hb(&sk, NS, 2, 0, 5, 0);
        assert!(hb.verify_signature().is_ok());
        hb.residue_identity = 0; // tampered after signing
        assert!(
            hb.verify_signature().is_err(),
            "verify_signature must reject a mutated residue_identity"
        );
    }

    #[test]
    fn stale_entry_is_filtered_after_ttl() {
        let cache = MigrationStatusCache::default();
        let sk = PrivateKey::random(&mut rand::thread_rng());
        cache.insert(&signed_hb(&sk, NS, 2, 0, 0, 0));
        // Drive the TTL via a very small per-call window — the same seam the
        // readiness tests use (`pick_sync_partner_excludes_stale_entries`).
        std::thread::sleep(Duration::from_millis(10));
        assert!(
            cache.fresh_peers(NS, Duration::from_millis(5)).is_empty(),
            "entry past TTL must not be fresh"
        );
        assert!(
            cache
                .peer_entry(NS, sk.public_key(), Duration::from_millis(5))
                .is_none(),
            "stale (ns, peer) lookup must report unknown (None)"
        );
    }

    #[test]
    fn insert_drops_stale_heartbeat_from_same_peer() {
        // Gossipsub out-of-order delivery must not stale-overwrite a fresher
        // entry: the fresher heartbeat's fields remain after the older one
        // arrives second.
        let cache = MigrationStatusCache::default();
        let sk = PrivateKey::random(&mut rand::thread_rng());
        let mut fresh = signed_hb(&sk, NS, 2, 0, 0, 2000);
        fresh.residue_identity = 0;
        let mut stale = signed_hb(&sk, NS, 1, 0, 9, 1000);
        stale.residue_identity = 9;
        cache.insert(&fresh);
        cache.insert(&stale); // arrives second but is older — must be dropped
        let entry = cache
            .peer_entry(NS, sk.public_key(), DEFAULT_HEARTBEAT_TTL)
            .unwrap();
        assert_eq!(
            entry.schema_version, 2,
            "stale heartbeat must not overwrite fresher entry from same peer"
        );
        assert_eq!(entry.residue_identity, 0);
    }

    #[test]
    fn insert_accepts_newer_heartbeat_from_same_peer() {
        let cache = MigrationStatusCache::default();
        let sk = PrivateKey::random(&mut rand::thread_rng());
        let older = signed_hb(&sk, NS, 1, 0, 9, 1000);
        let newer = signed_hb(&sk, NS, 2, 0, 0, 2000);
        cache.insert(&older);
        cache.insert(&newer);
        let entry = cache
            .peer_entry(NS, sk.public_key(), DEFAULT_HEARTBEAT_TTL)
            .unwrap();
        assert_eq!(
            entry.schema_version, 2,
            "newer heartbeat must replace older"
        );
        assert_eq!(entry.residue_identity, 0);
    }

    #[test]
    fn insert_rejects_far_future_ts_millis() {
        // Cache-poisoning regression: a far-future ts_millis would otherwise
        // freeze the entry, dropping every later legitimate heartbeat as
        // "older".
        let cache = MigrationStatusCache::default();
        let sk = PrivateKey::random(&mut rand::thread_rng());
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        let poison = signed_hb(&sk, NS, 9, 0, 0, now_ms + 600_000);
        cache.insert(&poison);
        assert!(
            cache.fresh_peers(NS, DEFAULT_HEARTBEAT_TTL).is_empty(),
            "far-future heartbeat must be rejected to prevent cache poisoning"
        );
        let legit = signed_hb(&sk, NS, 2, 0, 0, now_ms);
        cache.insert(&legit);
        let entry = cache
            .peer_entry(NS, sk.public_key(), DEFAULT_HEARTBEAT_TTL)
            .unwrap();
        assert_eq!(entry.schema_version, 2);
    }

    #[test]
    fn on_change_emit_fires_when_residue_changes() {
        let base = MigrationFacts {
            schema_version: 2,
            residue_auto: 0,
            residue_identity: 4,
            synced_up_to_hlc: 10,
        };
        // First-ever emit (no prior) always fires.
        assert!(should_emit_on_change(None, base));
        // residue_identity drops 4 -> 0: on-change must fire.
        let drained = MigrationFacts {
            residue_identity: 0,
            ..base
        };
        assert!(
            should_emit_on_change(Some(base), drained),
            "residue_identity change must edge-trigger an emit"
        );
        // residue_auto change must also fire.
        let auto_changed = MigrationFacts {
            residue_auto: 1,
            ..base
        };
        assert!(should_emit_on_change(Some(base), auto_changed));
        // schema_version change must fire.
        let bumped = MigrationFacts {
            schema_version: 3,
            ..base
        };
        assert!(should_emit_on_change(Some(base), bumped));
    }

    #[test]
    fn on_change_emit_suppressed_when_only_hlc_advances() {
        // synced_up_to_hlc advances on every applied op; it must NOT edge-
        // trigger (the periodic beat carries its latest value), otherwise
        // the debounce-to-periodic intent is defeated.
        let prev = MigrationFacts {
            schema_version: 2,
            residue_auto: 0,
            residue_identity: 0,
            synced_up_to_hlc: 10,
        };
        let advanced = MigrationFacts {
            synced_up_to_hlc: 99,
            ..prev
        };
        assert!(
            !should_emit_on_change(Some(prev), advanced),
            "an HLC-only advance must not edge-trigger an emit"
        );
    }

    #[test]
    fn record_facts_update_seeds_namespace_and_reports_edge() {
        // The production sender (governance-apply path) feeds the emitter via
        // `MigrationFactsUpdate`, whose handler calls `record_facts_update`.
        // Before any update the carry-forward map is empty, so the periodic
        // keep-alive tick has nothing to publish; the first update MUST seed it
        // (and report an edge, since there is no prior) so the tick goes live.
        let mut last_emitted: HashMap<[u8; 32], MigrationFacts> = HashMap::new();
        let facts = MigrationFacts {
            schema_version: 2,
            residue_auto: 0,
            residue_identity: 3,
            synced_up_to_hlc: 10,
        };

        let emit = record_facts_update(&mut last_emitted, NS, facts);
        assert!(emit, "first-ever facts for a namespace must edge-trigger");
        assert_eq!(
            last_emitted.get(&NS).copied(),
            Some(facts),
            "first update must seed last_emitted so the periodic tick has work \
             (the dead-empty-map regression)"
        );

        // An HLC-only advance records the new value but does NOT edge-trigger;
        // the periodic tick carries it.
        let advanced = MigrationFacts {
            synced_up_to_hlc: 99,
            ..facts
        };
        let emit = record_facts_update(&mut last_emitted, NS, advanced);
        assert!(!emit, "HLC-only advance must not edge-trigger");
        assert_eq!(
            last_emitted.get(&NS).copied(),
            Some(advanced),
            "carry-forward value must still update on a non-edge"
        );

        // A residue drop is an edge.
        let drained = MigrationFacts {
            residue_identity: 0,
            ..advanced
        };
        let emit = record_facts_update(&mut last_emitted, NS, drained);
        assert!(emit, "residue drop must edge-trigger");
    }

    #[test]
    fn built_heartbeat_verifies_and_carries_facts() {
        let sk = PrivateKey::random(&mut rand::thread_rng());
        let facts = MigrationFacts {
            schema_version: 2,
            residue_auto: 3,
            residue_identity: 1,
            synced_up_to_hlc: 77,
        };
        let hb = build_signed_heartbeat(&sk, NS, facts, 1234).expect("sign");
        // The receiver's signature gate accepts it, and the cache then reads
        // back exactly the facts we signed.
        assert!(hb.verify_signature().is_ok());
        assert_eq!(hb.peer_pubkey, sk.public_key());
        assert_eq!(hb.schema_version, 2);
        assert_eq!(hb.residue_auto, 3);
        assert_eq!(hb.residue_identity, 1);
        assert_eq!(hb.synced_up_to_hlc, 77);

        let cache = MigrationStatusCache::default();
        cache.insert(&hb);
        let entry = cache
            .peer_entry(NS, sk.public_key(), DEFAULT_HEARTBEAT_TTL)
            .expect("built heartbeat is cacheable");
        assert_eq!(entry.residue_identity, 1);
    }
}
