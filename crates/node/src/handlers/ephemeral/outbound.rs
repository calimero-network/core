//! Outbound ephemeral-presence: set + encrypt + publish + heartbeat.
//!
//! **No state-delta, no RocksDB writes.** The only storage interaction is a
//! *read* of the group, its current key record, and this node's local signing
//! key ([`resolve_publish_material`]) — all synchronous keyring scans, taken
//! once per publish on the actor thread so the spawned publish itself touches
//! no store at all. The awareness store is in-memory only; see `store.rs`.
//!
//! # Two entry points
//!
//! * [`set_local_ephemeral`] — called when the local client sets its own
//!   presence slice. Validates the size, bumps the per-author sequence
//!   counter, encrypts under the current group key, applies to the local
//!   [`AwarenessStore`], emits the local upsert diff so the setting client
//!   sees its own state echoed, and spawns an async publish onto the context
//!   gossip topic.
//!
//! * [`heartbeat_tick`] — called on the [`PRESENCE_HEARTBEAT_MS`] interval.
//!   Re-publishes every locally-set slice (re-encrypted, fresh nonce, bumped
//!   seq so remote nodes refresh their `last_seen_ms` liveness timestamp) and
//!   sweeps expired remote entries from the [`AwarenessStore`], emitting
//!   [`Diff::Remove`] events.

use std::borrow::Cow;

use calimero_crypto::SharedKey;
use calimero_network_primitives::client::NetworkClient;
use calimero_node_primitives::sync::snapshot::BroadcastMessage;
use calimero_primitives::context::ContextId;
use calimero_primitives::identity::{PrivateKey, PublicKey};
use libp2p::gossipsub::TopicHash;
use tracing::debug;

use crate::handlers::ephemeral::inbound::emit_ephemeral_diff;
// `now_ms` lives in the parent module so the publish path, the inbound apply,
// and the snapshot RPC cannot disagree about what a `now_ms` means; see there
// for the pre-epoch behaviour.
use crate::handlers::ephemeral::{now_ms, EPHEMERAL_MAX_BYTES, PRESENCE_TTL_MS};
use crate::NodeManager;

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

/// Typed errors for the outbound ephemeral path.
#[derive(Debug, thiserror::Error)]
pub enum EphemeralOutboundError {
    /// Caller supplied a slice that exceeds the protocol maximum.
    #[error(
        "ephemeral slice is too large: {size} bytes (max {EPHEMERAL_MAX_BYTES})",
        size = .0
    )]
    SliceTooLarge(usize),
    /// The context has not been registered in any group.
    #[error("context has no group")]
    NoGroup,
    /// The group has no current encryption key.
    #[error("no current group key")]
    NoGroupKey,
    /// The author is not a local signing identity for this context, so the
    /// envelope cannot be signed. Presence is not publishable without it.
    #[error("no local signing key for the ephemeral author")]
    NoLocalSigningKey,
}

// ---------------------------------------------------------------------------
// Per-author local state (seq counter + current slice)
// ---------------------------------------------------------------------------

/// Owned outbound presence state for one `(ContextId, PublicKey)` pair.
///
/// Kept on `NodeManager` so the heartbeat tick can re-publish without the
/// caller re-supplying the slice, and so the sequence counter is monotonically
/// owned in one place.
#[derive(Debug, Clone)]
pub(crate) struct LocalEphemeral {
    /// Per-author LWW counter. Seeded by [`initial_seq`], never from zero —
    /// see there for why.
    pub(crate) seq: u64,
    pub(crate) slice: Vec<u8>,
}

/// Seed value for a new author's `seq` counter: the current wall clock in
/// milliseconds.
///
/// **Not zero, and not a process counter.** `seq` is the receiver's LWW rule
/// ([`store::AwarenessStore::apply`](crate::handlers::ephemeral::store::AwarenessStore::apply)
/// drops anything at or below the seq it already holds), and this map lives
/// only in memory. Seeded at 0, a node that restarts republishes at seq 1..n
/// while its peers still hold the seq it reached before the restart, so every
/// update it sends is discarded as stale until those entries TTL-sweep —
/// bounded at [`PRESENCE_TTL_MS`] and self-healing, but a real gap in which a
/// node that is demonstrably back looks absent to everyone.
///
/// Seeding from the wall clock makes the counter monotonic *across* process
/// lifetimes without persisting anything: the value after a restart exceeds
/// any value the previous run could have published, because that run's own
/// seeds came from an earlier clock reading. Presence stays entirely in
/// memory; nothing here is written to the DAG or any store.
///
/// It does not need to be collision-free across authors — `seq` is compared
/// only within a single `(context, author)` entry, never between authors — and
/// it does not need to be dense. A clock that jumps backwards more than the
/// TTL reintroduces the original staleness for one TTL, which is the same
/// bound as before and strictly better than the guaranteed hit at 0.
fn initial_seq() -> u64 {
    now_ms()
}

// ---------------------------------------------------------------------------
// Publish material (synchronous store resolution)
// ---------------------------------------------------------------------------

/// Everything a publish needs out of the store, resolved in one place.
///
/// Kept as one value because the three lookups are only ever useful together:
/// a caller holding a group key but no signing key cannot publish, and one
/// holding a signing key but no group key cannot encrypt. Resolving them as a
/// unit is what lets [`set_local_ephemeral`] decide, synchronously, whether a
/// publish is possible at all before it echoes anything to the local client.
#[derive(Debug, Clone, Copy)]
pub(crate) struct PublishMaterial {
    /// `sha256(group_key)` of the group's current key — goes on the wire so
    /// receivers can check they hold the same key.
    key_id: [u8; 32],
    /// The group's current encryption key.
    group_key: [u8; 32],
    /// This node's local signing key for `author` in the context.
    signing_key: [u8; 32],
}

/// Resolve the group, its current key, and the local signing key for `author`.
///
/// Synchronous: every one of these is a plain store read. Split out of
/// [`do_publish_ephemeral`] so the failures are available to a caller *before*
/// it commits to a local echo — see [`set_local_ephemeral`].
///
/// **Never writes to the DAG, RocksDB, or any persistent store.**
pub(crate) fn resolve_publish_material(
    store: &calimero_store::Store,
    context_id: ContextId,
    author: PublicKey,
) -> eyre::Result<PublishMaterial> {
    // Resolve the group the context belongs to (same derivation as inbound).
    let group_id = calimero_governance_store::get_group_for_context(store, &context_id)?
        .ok_or(EphemeralOutboundError::NoGroup)?;

    // Load the current (highest-epoch) group key for encryption.
    let record = calimero_governance_store::GroupKeyring::new(store, group_id)
        .load_current_key_record()?
        .ok_or(EphemeralOutboundError::NoGroupKey)?;

    let signing_key =
        calimero_governance_store::resolve_local_signing_key(store, &context_id, &author)?
            .ok_or(EphemeralOutboundError::NoLocalSigningKey)?;

    Ok(PublishMaterial {
        key_id: record.key_id,
        group_key: record.group_key,
        signing_key,
    })
}

// ---------------------------------------------------------------------------
// Inner async function (testable without actix)
// ---------------------------------------------------------------------------

/// Build, serialize, and publish an `Ephemeral` broadcast message.
///
/// Takes its keys pre-resolved ([`PublishMaterial`]): both callers hold a store
/// handle synchronously and resolve there, so this function does no store I/O
/// at all — it generates a random nonce, AEAD-seals `slice`, signs the
/// envelope, wraps it in [`BroadcastMessage::Ephemeral`], borsh-serializes, and
/// calls `network_client.publish` on the context gossip topic.
///
/// Returns an error if encryption, signing, or serialization fails. A publish
/// failure (e.g. no mesh peers) is logged at `debug` and treated as a no-op —
/// ephemeral presence is best-effort.
///
/// **Never writes to the DAG, RocksDB, or any persistent store.**
pub(crate) async fn do_publish_ephemeral(
    network_client: &NetworkClient,
    context_id: ContextId,
    author: PublicKey,
    seq: u64,
    slice: Vec<u8>,
    material: PublishMaterial,
) -> eyre::Result<()> {
    // AEAD-seal the presence slice under the group key.
    // `SharedKey::from_sk` derives the symmetric key from the group private key
    // — identical to the `encrypt_op` pattern in `group_keys.rs`.
    //
    // `encrypt` mints a fresh random nonce per call and hands it back; presence
    // slices have no ratchet to sequence them, so a per-message random nonce is
    // exactly the guarantee this path needs (`encrypt_with_nonce` is for the
    // sync stream, whose caller owns single-use).
    let sk = PrivateKey::from(material.group_key);
    let (nonce, ciphertext) = SharedKey::from_sk(&sk)
        .encrypt(slice)
        .ok_or_else(|| eyre::eyre!("AEAD encrypt failed for ephemeral slice"))?;

    // Stamp the publish time. Read here — as late as possible, immediately
    // before signing — so the receiver's freshness window is spent on flight
    // time rather than on however long this node took to get here.
    let sent_at_ms = now_ms();

    // Sign the envelope. `author`, `seq`, `sent_at_ms` and `nonce` all ride
    // outside the AEAD, so the signature is what makes them tamper-evident; the
    // receive path refuses anything that does not verify.
    let envelope = crate::handlers::ephemeral::auth::SignedEnvelope {
        context_id,
        author,
        seq,
        key_id: material.key_id,
        sent_at_ms,
        nonce,
        ciphertext: &ciphertext,
    };
    let signature_payload =
        crate::handlers::ephemeral::auth::ephemeral_signature_payload(envelope)?;
    let signature = PrivateKey::from(material.signing_key)
        .sign(&signature_payload)
        .map_err(|err| eyre::eyre!("failed to sign ephemeral envelope: {err}"))?
        .to_bytes();

    // Build and serialize the wire message.
    let msg = BroadcastMessage::Ephemeral {
        context_id,
        author,
        seq,
        key_id: material.key_id,
        sent_at_ms,
        nonce,
        ciphertext: Cow::Owned(ciphertext),
        signature,
    };
    let bytes = borsh::to_vec(&msg)?;

    // Publish on the context gossip topic — same derivation used throughout
    // the node (e.g. `broadcast_heartbeat` in `client.rs:633`).
    let topic = TopicHash::from_raw(context_id);

    match network_client.publish(topic, bytes).await {
        Ok(_) => {}
        Err(err) => {
            debug!(
                %context_id,
                %err,
                "ephemeral: failed to publish (no mesh peers or network error) — will retry on next heartbeat"
            );
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Actix entry points (operate on NodeManager)
// ---------------------------------------------------------------------------

/// Set the local node's ephemeral presence slice for `context_id`.
///
/// * Rejects slices larger than [`EPHEMERAL_MAX_BYTES`] with
///   [`EphemeralOutboundError::SliceTooLarge`].
/// * Bumps the per-`(context_id, author)` monotonic sequence counter.
/// * Applies the slice to the local [`AwarenessStore`] and emits a
///   [`Diff::Upsert`] on the node's event broadcast sink so any WebSocket
///   subscriber on **this node** sees its own state echoed immediately.
/// * Spawns [`do_publish_ephemeral`] on this actor's Arbiter so the message
///   is gossiped to peers.
///
/// Rejects synchronously — before the local echo and before the spawn — for
/// every condition that makes a publish impossible: an oversized slice, no
/// group for the context ([`EphemeralOutboundError::NoGroup`]), no current
/// group key ([`EphemeralOutboundError::NoGroupKey`]), and no local signing key
/// for `author` ([`EphemeralOutboundError::NoLocalSigningKey`]). All three
/// store lookups happen here, in [`resolve_publish_material`], rather than
/// inside the spawned publish, where a failure would be swallowed at `debug!`:
/// the caller would get `Ok(())`, the local client would see its own presence
/// echoed back, and nothing would ever reach a peer.
///
/// Only genuinely best-effort failures stay inside the spawn: encryption,
/// signing, serialization, and the network/mesh publish itself, which is
/// expected to fail whenever there are no mesh peers yet.
pub(crate) fn set_local_ephemeral(
    this: &mut NodeManager,
    ctx: &mut actix::Context<NodeManager>,
    context_id: ContextId,
    author: PublicKey,
    slice: Vec<u8>,
) -> eyre::Result<()> {
    use actix::{AsyncContext, WrapFuture};

    // 1. Size guard.
    if slice.len() > EPHEMERAL_MAX_BYTES {
        return Err(EphemeralOutboundError::SliceTooLarge(slice.len()).into());
    }

    // 1b. Publishability guard. The group, its current key, and the local
    // signing key are all resolved here — synchronously, before the local echo
    // — rather than inside the spawned `do_publish_ephemeral`, where any of
    // them failing would be swallowed at `debug!` and never reach the caller
    // (see the doc comment above): the local echo would still fire, the
    // setting client would see its own cursor working, and nothing would ever
    // actually publish. Failing fast here keeps the caller's contract — "a
    // synchronous error means nothing was set or published" — true for all
    // three failure modes.
    //
    // The resolved material is then handed to the publish rather than looked up
    // a second time inside the spawn.
    let material = resolve_publish_material(this.clients.context.datastore(), context_id, author)?;

    // 2. Bump the sequence counter (or seed a new author's — from the wall
    //    clock, not 0, so a restarted node's updates are not rejected as stale
    //    by peers still holding its pre-restart seq; see `initial_seq`).
    let local = this
        .ephemeral_local
        .entry((context_id, author))
        .or_insert_with(|| LocalEphemeral {
            seq: initial_seq(),
            slice: vec![],
        });
    local.seq = local.seq.saturating_add(1);
    local.slice = slice.clone();
    let seq = local.seq;

    // 3. Apply to the local awareness store NOW (synchronously, before the
    //    async publish) and emit the local diff so the setting client sees
    //    its own state immediately. This local echo carries no envelope
    //    signature — it never went through `resolve_and_decrypt`'s verify
    //    step, because it never left this node unsigned in the first place.
    //    See the security note on `EphemeralPayload` for the client-facing
    //    implication (not every event of this type has actually been through
    //    signature verification).
    let now = now_ms();
    for diff in this
        .awareness_store
        .apply(context_id, author, seq, slice.clone(), now)
    {
        emit_ephemeral_diff(&this.clients.node, context_id, diff);
    }

    // 4. Spawn the encrypt + sign + publish. No store reads left in here.
    let network_client = this.clients.node.network_client().clone();

    let _ignored = ctx.spawn(
        async move {
            if let Err(err) =
                do_publish_ephemeral(&network_client, context_id, author, seq, slice, material)
                    .await
            {
                debug!(%context_id, %err, "ephemeral: outbound publish error");
            }
        }
        .into_actor(this),
    );

    Ok(())
}

/// Whether a failed [`resolve_publish_material`] means this `(context, author)`
/// pair is gone for good, and its `ephemeral_local` entry should be reclaimed.
///
/// The distinction matters because the alternative to reclaiming is heartbeating
/// forever, and the alternative to *keeping* is losing a live author's presence
/// over a transient blip.
///
/// Permanent — the pair is dropped:
/// * [`EphemeralOutboundError::NoGroup`] — the context is not registered in any
///   group. That is what a deleted or left context looks like from here, and it
///   does not come back for the same context id.
/// * [`EphemeralOutboundError::NoLocalSigningKey`] — this node no longer holds a
///   signing identity for `author` in the context, i.e. the member departed or
///   the identity was removed. Presence is unpublishable without it, and this is
///   the same condition that would reject a fresh `set_local_ephemeral`.
///
/// Not permanent — the entry is kept:
/// * [`EphemeralOutboundError::NoGroupKey`] — the group exists but this node
///   currently resolves no current key. A node mid-rotation or still syncing the
///   keyring hits this, and it resolves itself; dropping here would silently end
///   a live author's presence.
/// * any other error (store I/O and the like) — transient by assumption, and
///   the wrong thing to widen: an unrecognised failure should not be able to
///   delete state.
fn is_permanent_publish_failure(err: &eyre::Report) -> bool {
    matches!(
        err.downcast_ref::<EphemeralOutboundError>(),
        Some(EphemeralOutboundError::NoGroup | EphemeralOutboundError::NoLocalSigningKey)
    )
}

/// Refresh the local author entries, then expire everything stale.
///
/// Pure over the [`AwarenessStore`] — no actor, no network — so the ordering
/// guarantee below is unit-testable on its own. Returns the `(context, diff)`
/// pairs the caller must emit.
///
/// **Order is load-bearing: touch first, sweep second.**
///
/// `local` names the `(context, author)` pairs this node publishes itself.
/// Gossipsub does not deliver a node's own published message back to itself,
/// so nothing ever re-stamps `last_seen_ms` for a locally-set slice after the
/// original `set_local_ephemeral`. Without the touch, the sweep would evict
/// the node's OWN presence one TTL later even though it is still heartbeating
/// every `PRESENCE_HEARTBEAT_MS` and every remote peer holds a fresh copy —
/// local readers (the event stream, and the replay a new subscriber is seeded
/// with) would see the local author sawtooth in and out, with a spurious
/// `Diff::Remove` each cycle.
///
/// The sweep covers every context the store holds entries for — NOT just the
/// ones this node has locally published to. A receive-only node (a read-only
/// viewer, a TEE node, a peer who has not yet set its own presence) never
/// appears in `local`, but the store still holds remote entries for it that
/// must expire on the same TTL. Collected into a `HashSet` (rather than
/// sorted-then-`dedup`, which only removes ADJACENT duplicates and silently
/// half-works on an unsorted source) so every context is swept exactly once
/// regardless of source order.
pub(crate) fn refresh_and_sweep(
    awareness_store: &mut crate::handlers::ephemeral::store::AwarenessStore,
    local: &[(ContextId, PublicKey)],
    ttl_ms: u64,
    now_ms: u64,
) -> Vec<(ContextId, crate::handlers::ephemeral::store::Diff)> {
    for (context_id, author) in local {
        awareness_store.touch(*context_id, *author, now_ms);
    }

    let contexts_seen: std::collections::HashSet<ContextId> = awareness_store
        .contexts()
        .chain(local.iter().map(|(ctx_id, _)| *ctx_id))
        .collect();

    let mut diffs = Vec::new();
    for context_id in contexts_seen {
        for diff in awareness_store.sweep(context_id, ttl_ms, now_ms) {
            diffs.push((context_id, diff));
        }
    }
    diffs
}

/// Heartbeat tick: re-publish all locally-set ephemeral slices and sweep
/// stale remote entries.
///
/// Called on the [`PRESENCE_HEARTBEAT_MS`] interval (wired in
/// `manager/startup.rs`). For every `(context_id, author)` pair in the
/// node's local map:
///
/// 1. Bumps the sequence counter (so remote nodes update their
///    `last_seen_ms` on receipt — same-seq re-applies are no-ops in the
///    remote [`AwarenessStore`]).
/// 2. Refreshes the node's own entry in the [`AwarenessStore`] so its own
///    TTL sweep cannot evict a slice it is still heartbeating — gossipsub
///    never echoes a node's own publish back to it, so nothing else would.
/// 3. Re-encrypts under the **current** group key with a **fresh nonce**.
/// 4. Publishes on the context gossip topic.
/// 5. Sweeps expired remote entries for the context, emitting
///    [`Diff::Remove`] events for each evicted author.
pub(crate) fn heartbeat_tick(
    this: &mut NodeManager,
    ctx: &mut actix::Context<NodeManager>,
    now_ms: u64,
) {
    use actix::{AsyncContext, WrapFuture};

    // Collect a snapshot of local entries so we can mutate `ephemeral_local`
    // and `awareness_store` synchronously while holding no borrow.
    let local_snapshot: Vec<(ContextId, PublicKey, u64, Vec<u8>)> = this
        .ephemeral_local
        .iter_mut()
        .map(|((ctx_id, author), entry)| {
            entry.seq = entry.seq.saturating_add(1);
            (*ctx_id, *author, entry.seq, entry.slice.clone())
        })
        .collect();

    let local_pairs: Vec<(ContextId, PublicKey)> = local_snapshot
        .iter()
        .map(|(ctx_id, author, ..)| (*ctx_id, *author))
        .collect();

    for (context_id, diff) in refresh_and_sweep(
        &mut this.awareness_store,
        &local_pairs,
        PRESENCE_TTL_MS,
        now_ms,
    ) {
        emit_ephemeral_diff(&this.clients.node, context_id, diff);
    }

    // Resolve each pair's publish material here, synchronously, for the same
    // reason `set_local_ephemeral` does: these are plain store reads, and doing
    // them on the actor thread keeps `do_publish_ephemeral` free of store
    // access entirely. Re-resolved every tick rather than cached, so a key
    // rotation is picked up on the next heartbeat.
    //
    // A pair that no longer resolves is either *departed* or *degraded*, and
    // the two are treated differently — see `is_permanent_publish_failure`.
    // Departed pairs are dropped from `ephemeral_local` here; this is the
    // subsystem's only reclamation path for locally-set presence, and without
    // it a long-lived node accumulates one permanent entry per
    // `(context, author)` it ever published for, heartbeating forever into
    // contexts it has left.
    let store = this.clients.context.datastore();
    let mut departed: Vec<(ContextId, PublicKey)> = Vec::new();
    let publishable: Vec<(ContextId, PublicKey, u64, Vec<u8>, PublishMaterial)> = local_snapshot
        .into_iter()
        .filter_map(|(context_id, author, seq, slice)| {
            match resolve_publish_material(store, context_id, author) {
                Ok(material) => Some((context_id, author, seq, slice, material)),
                Err(err) => {
                    let permanent = is_permanent_publish_failure(&err);
                    debug!(
                        %context_id, %author, %err, permanent,
                        "ephemeral: heartbeat cannot publish"
                    );
                    if permanent {
                        departed.push((context_id, author));
                    }
                    None
                }
            }
        })
        .collect();

    // Reclaim after the `store` borrow ends. Dropping the entry stops the
    // heartbeat from refreshing this author, so the local echo still sitting in
    // the awareness store ages out on its own within `PRESENCE_TTL_MS` and the
    // sweep emits the `Diff::Remove` — no separate teardown needed.
    for pair in departed {
        let _ignored = this.ephemeral_local.remove(&pair);
    }

    // Re-publish each local slice (bumped seq, fresh nonce, current key).
    let network_client = this.clients.node.network_client().clone();

    let _ignored = ctx.spawn(
        async move {
            for (context_id, author, seq, slice, material) in publishable {
                if let Err(err) =
                    do_publish_ephemeral(&network_client, context_id, author, seq, slice, material)
                        .await
                {
                    debug!(%context_id, %err, "ephemeral: heartbeat re-publish error");
                }
            }
        }
        .into_actor(this),
    );
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use actix::Actor;
    use calimero_context_config::types::ContextGroupId;
    use calimero_governance_store::{register_context_in_group, GroupKeyring};
    use calimero_network_primitives::client::NetworkClient;
    use calimero_network_primitives::messages::{MessageId, NetworkMessage};
    use calimero_node_primitives::sync::snapshot::BroadcastMessage;
    use calimero_primitives::context::ContextId;
    use calimero_primitives::identity::{PrivateKey, PublicKey};
    use calimero_store::db::InMemoryDB;
    use calimero_store::Store;
    use calimero_utils_actix::LazyRecipient;
    use libp2p::gossipsub::TopicHash;

    use super::*;
    use crate::handlers::ephemeral::auth::SignedEnvelope;
    use crate::handlers::ephemeral::PRESENCE_HEARTBEAT_MS;

    // -----------------------------------------------------------------------
    // Shared test scaffolding
    // -----------------------------------------------------------------------

    fn fresh_store() -> Store {
        Store::new(Arc::new(InMemoryDB::owned()))
    }

    /// Recording network actor: captures every `Publish` call's topic and
    /// payload bytes.  Replies with a stub `MessageId` so callers don't hang.
    ///
    /// Mirrors the `CountingNetworkActor` in `calimero-node-primitives` tests
    /// but records the full `(topic, data)` pair for content assertions.
    struct RecordingNetworkActor {
        published: Arc<Mutex<Vec<(TopicHash, Vec<u8>)>>>,
    }

    impl Actor for RecordingNetworkActor {
        type Context = actix::Context<Self>;
    }

    impl actix::Handler<NetworkMessage> for RecordingNetworkActor {
        type Result = ();

        fn handle(&mut self, msg: NetworkMessage, _ctx: &mut Self::Context) {
            if let NetworkMessage::Publish { request, outcome } = msg {
                self.published
                    .lock()
                    .expect("published lock")
                    .push((request.topic, request.data));
                let _ = outcome.send(Ok(MessageId(b"stub".to_vec())));
            }
            // All other message variants: no-op.
        }
    }

    /// Build a `NetworkClient` backed by a live recording actor.
    ///
    /// Must be called inside an `#[actix::test]` (or otherwise inside an
    /// actix System) so `RecordingNetworkActor::create` has an Arbiter to
    /// start on.  Returns the `NetworkClient` and a shared handle to the
    /// captured publish calls.
    fn recording_network_client() -> (NetworkClient, Arc<Mutex<Vec<(TopicHash, Vec<u8>)>>>) {
        let published: Arc<Mutex<Vec<(TopicHash, Vec<u8>)>>> = Arc::new(Mutex::new(vec![]));
        let network_recipient = LazyRecipient::<NetworkMessage>::new();
        let network_client = NetworkClient::new(network_recipient.clone());
        let actor = RecordingNetworkActor {
            published: Arc::clone(&published),
        };
        let _addr = RecordingNetworkActor::create(move |ctx| {
            assert!(network_recipient.init(ctx), "network recipient init");
            actor
        });
        (network_client, published)
    }

    /// Seed a group key and register the context. Returns
    /// `(group_id, key_id, group_key_bytes)`.
    fn seed_group_key(
        store: &Store,
        context_id: ContextId,
    ) -> (ContextGroupId, [u8; 32], [u8; 32]) {
        let group_id = ContextGroupId::from([0xAB; 32]);
        register_context_in_group(store, &group_id, &context_id)
            .expect("register_context_in_group");
        let group_key_bytes = [0x42u8; 32];
        let ring = GroupKeyring::new(store, group_id);
        let key_id = ring.store_key(&group_key_bytes).expect("store_key");
        (group_id, key_id, group_key_bytes)
    }

    /// Seed the `ContextIdentity` row that marks `sk` as a local signing
    /// identity for `context_id`, so `resolve_local_signing_key` finds it.
    fn store_local_identity(store: &Store, context_id: &ContextId, sk: &PrivateKey) {
        let key = calimero_store::key::ContextIdentity::new(*context_id, sk.public_key());
        let value = calimero_store::types::ContextIdentity {
            private_key: Some(*sk.as_bytes()),
        };
        store.handle().put(&key, &value).expect("put identity");
    }

    // -----------------------------------------------------------------------
    // Test 1: `do_publish_ephemeral` publishes exactly one message on the
    //          derived context topic with seq==1 on the first call.
    // -----------------------------------------------------------------------

    #[actix::test]
    async fn publishes_exactly_one_ephemeral_on_context_topic() {
        let store = fresh_store();
        let context_id = ContextId::from([0x01u8; 32]);
        let author_sk = PrivateKey::from([0x02u8; 32]);
        let author = author_sk.public_key();

        let (_group_id, _key_id, _group_key_bytes) = seed_group_key(&store, context_id);
        store_local_identity(&store, &context_id, &author_sk);

        let (network_client, published) = recording_network_client();
        let material =
            resolve_publish_material(&store, context_id, author).expect("publish material");

        let slice = b"cursor={x:10}".to_vec();

        // First call — seq == 1.
        do_publish_ephemeral(&network_client, context_id, author, 1, slice, material)
            .await
            .expect("do_publish_ephemeral should succeed");

        let msgs = published.lock().expect("lock").clone();
        assert_eq!(msgs.len(), 1, "exactly one publish");

        // Verify the context-derived gossipsub topic.
        let expected_topic = TopicHash::from_raw(context_id);
        assert_eq!(
            msgs[0].0, expected_topic,
            "topic must be derived from context_id"
        );

        // Decode and verify seq and author identity.
        let decoded: BroadcastMessage<'_> = borsh::from_slice(&msgs[0].1).expect("borsh decode");
        match decoded {
            BroadcastMessage::Ephemeral {
                seq,
                context_id: cid,
                author: a,
                ..
            } => {
                assert_eq!(seq, 1, "seq must be 1 on first call");
                assert_eq!(cid, context_id);
                assert_eq!(a, author);
            }
            other => panic!("expected BroadcastMessage::Ephemeral, got {other:?}"),
        }
    }

    // -----------------------------------------------------------------------
    // Test 2: second call with seq==2 produces a second publish with seq==2.
    // -----------------------------------------------------------------------

    #[actix::test]
    async fn second_call_seq_increments() {
        let store = fresh_store();
        let context_id = ContextId::from([0x03u8; 32]);
        let author_sk = PrivateKey::from([0x04u8; 32]);
        let author = author_sk.public_key();

        let (_group_id, _key_id, _) = seed_group_key(&store, context_id);
        store_local_identity(&store, &context_id, &author_sk);

        let (network_client, published) = recording_network_client();
        let material =
            resolve_publish_material(&store, context_id, author).expect("publish material");

        let slice = b"cursor={x:10}".to_vec();

        do_publish_ephemeral(
            &network_client,
            context_id,
            author,
            1,
            slice.clone(),
            material,
        )
        .await
        .expect("first publish");

        do_publish_ephemeral(&network_client, context_id, author, 2, slice, material)
            .await
            .expect("second publish");

        let msgs = published.lock().expect("lock").clone();
        assert_eq!(msgs.len(), 2, "two publishes for two calls");

        let seq1 = match borsh::from_slice::<BroadcastMessage<'_>>(&msgs[0].1).expect("decode1") {
            BroadcastMessage::Ephemeral { seq, .. } => seq,
            other => panic!("expected Ephemeral, got {other:?}"),
        };
        let seq2 = match borsh::from_slice::<BroadcastMessage<'_>>(&msgs[1].1).expect("decode2") {
            BroadcastMessage::Ephemeral { seq, .. } => seq,
            other => panic!("expected Ephemeral, got {other:?}"),
        };

        assert_eq!(seq1, 1, "first msg seq");
        assert_eq!(seq2, 2, "second msg seq");
    }

    // -----------------------------------------------------------------------
    // Test 2b: a new author's seq is seeded from the wall clock, not 0.
    //
    //          `ephemeral_local` is in-memory only, so a restart wipes it. If
    //          the counter restarted at 0, every update a restarted node sent
    //          would lose the receivers' LWW comparison against the seq it had
    //          reached before the restart, and the node would look absent until
    //          those entries TTL-swept.
    // -----------------------------------------------------------------------

    #[test]
    fn a_new_authors_seq_is_seeded_above_any_previous_process() {
        let before = now_ms();
        let seeded = initial_seq();
        let after = now_ms();

        assert!(seeded > 0, "a fresh author must not start at 0");
        assert!(
            (before..=after).contains(&seeded),
            "the seed must be a wall-clock reading ({seeded} not in {before}..={after}), \
             which is what makes it exceed anything a previous process published"
        );
    }

    // -----------------------------------------------------------------------
    // Test 3: oversized slice returns the typed error; nothing is published.
    //
    //          `set_local_ephemeral` is the caller that enforces the size
    //          guard; this test validates the error variant and the contract
    //          that no publish is attempted for an over-limit slice.
    // -----------------------------------------------------------------------

    #[test]
    fn oversized_slice_size_error_variant() {
        let oversized_len = EPHEMERAL_MAX_BYTES + 1;
        let err = EphemeralOutboundError::SliceTooLarge(oversized_len);
        assert!(
            matches!(err, EphemeralOutboundError::SliceTooLarge(n) if n == oversized_len),
            "error variant and size must match"
        );
        // The error string must mention the oversized length.
        let msg = err.to_string();
        assert!(
            msg.contains(&oversized_len.to_string()),
            "error message must contain the bad size, got: {msg}"
        );
    }

    // -----------------------------------------------------------------------
    // Test 4: heartbeat re-publish: second do_publish call (bumped seq)
    //          results in a second publish with a fresh nonce.
    // -----------------------------------------------------------------------

    #[actix::test]
    async fn heartbeat_republish_sends_fresh_message() {
        let store = fresh_store();
        let context_id = ContextId::from([0x07u8; 32]);
        let author_sk = PrivateKey::from([0x08u8; 32]);
        let author = author_sk.public_key();

        let (_group_id, _key_id, _) = seed_group_key(&store, context_id);
        store_local_identity(&store, &context_id, &author_sk);

        let (network_client, published) = recording_network_client();
        let material =
            resolve_publish_material(&store, context_id, author).expect("publish material");

        let slice = b"presence=typing".to_vec();

        // Initial set.
        do_publish_ephemeral(
            &network_client,
            context_id,
            author,
            1,
            slice.clone(),
            material,
        )
        .await
        .expect("initial publish");

        // Heartbeat re-publish with bumped seq.
        do_publish_ephemeral(&network_client, context_id, author, 2, slice, material)
            .await
            .expect("heartbeat re-publish");

        let msgs = published.lock().expect("lock").clone();
        assert_eq!(msgs.len(), 2, "heartbeat produces a second publish");

        // Both messages must be well-formed Ephemeral messages.
        let (nonce1, nonce2);
        match borsh::from_slice::<BroadcastMessage<'_>>(&msgs[0].1).expect("decode msg 0") {
            BroadcastMessage::Ephemeral { nonce, seq, .. } => {
                assert_eq!(seq, 1);
                nonce1 = nonce;
            }
            other => panic!("msg 0: expected Ephemeral, got {other:?}"),
        }
        match borsh::from_slice::<BroadcastMessage<'_>>(&msgs[1].1).expect("decode msg 1") {
            BroadcastMessage::Ephemeral { nonce, seq, .. } => {
                assert_eq!(seq, 2);
                nonce2 = nonce;
            }
            other => panic!("msg 1: expected Ephemeral, got {other:?}"),
        }
        // Nonces must differ (with overwhelming probability).
        assert_ne!(nonce1, nonce2, "each publish must use a fresh random nonce");
    }

    // -----------------------------------------------------------------------
    // Test 5: the three publishability failures are all reported by
    //         `resolve_publish_material`, which `set_local_ephemeral` calls
    //         SYNCHRONOUSLY — so each one reaches the RPC caller instead of
    //         being swallowed at `debug!` inside the spawned publish while the
    //         caller sees `Ok(())` and its own presence echoed back.
    // -----------------------------------------------------------------------

    #[test]
    fn unknown_context_returns_no_group_error() {
        let store = fresh_store();
        // Deliberately NOT seeding a group for this context.
        let context_id = ContextId::from([0x09u8; 32]);
        let author = PublicKey::from([0x0Au8; 32]);

        let err = resolve_publish_material(&store, context_id, author)
            .expect_err("must error for unknown context")
            .to_string();
        assert!(
            err.contains("no group"),
            "error must mention 'no group', got: {err}"
        );
    }

    /// `ephemeral_local` has no TTL, so the heartbeat's classification of a
    /// failed resolve is the only thing that reclaims a departed pair. Getting
    /// this wrong in either direction is a real bug: too narrow leaks an entry
    /// that heartbeats forever into a context the node has left, too wide
    /// deletes a live author's presence over a transient blip.
    #[test]
    fn only_departure_failures_are_treated_as_permanent() {
        assert!(
            is_permanent_publish_failure(&eyre::Report::new(EphemeralOutboundError::NoGroup)),
            "a context in no group is gone — reclaim it"
        );
        assert!(
            is_permanent_publish_failure(&eyre::Report::new(
                EphemeralOutboundError::NoLocalSigningKey
            )),
            "no local signing identity means the member departed — reclaim it"
        );
        assert!(
            !is_permanent_publish_failure(&eyre::Report::new(EphemeralOutboundError::NoGroupKey)),
            "a missing current key can be mid-rotation — keep the entry"
        );
        assert!(
            !is_permanent_publish_failure(&eyre::Report::msg("transient store failure")),
            "an unrecognised error must never delete state"
        );
    }

    /// The permanent classification must line up with what the store actually
    /// reports for a departed context, not just with the error enum in
    /// isolation — a deleted/left context surfaces here as `NoGroup`.
    #[test]
    fn a_context_in_no_group_classifies_as_permanent() {
        let store = fresh_store();
        let err = resolve_publish_material(
            &store,
            ContextId::from([0x3Au8; 32]),
            PublicKey::from([0x3Bu8; 32]),
        )
        .expect_err("unregistered context must error");
        assert!(
            is_permanent_publish_failure(&err),
            "a departed context must be reclaimable, got: {err}"
        );
    }

    #[test]
    fn context_without_a_group_key_returns_no_group_key_error() {
        let store = fresh_store();
        let context_id = ContextId::from([0x0Bu8; 32]);
        let author_sk = PrivateKey::from([0x0Cu8; 32]);
        let author = author_sk.public_key();

        // Registered in a group, but the group's keyring is empty.
        let group_id = ContextGroupId::from([0xAC; 32]);
        register_context_in_group(&store, &group_id, &context_id).expect("register");
        store_local_identity(&store, &context_id, &author_sk);

        let err = resolve_publish_material(&store, context_id, author)
            .expect_err("must error when the group holds no key")
            .to_string();
        assert!(
            err.contains("no current group key"),
            "error must mention the missing group key, got: {err}"
        );
    }

    #[test]
    fn author_without_a_local_signing_key_returns_no_local_signing_key_error() {
        let store = fresh_store();
        let context_id = ContextId::from([0x0Du8; 32]);
        let author = PublicKey::from([0x0Eu8; 32]);

        // Group and key are fine; the author is simply not ours to sign for.
        let (_group_id, _key_id, _) = seed_group_key(&store, context_id);

        let err = resolve_publish_material(&store, context_id, author)
            .expect_err("must error without a local signing key")
            .to_string();
        assert!(
            err.contains("no local signing key"),
            "error must mention the missing signing key, got: {err}"
        );
    }

    #[test]
    fn fully_seeded_context_resolves_publish_material() {
        // Positive control: with all three present, resolution succeeds and
        // hands back the group's CURRENT key id, so the guards above are
        // rejecting for the reason they claim rather than always failing.
        let store = fresh_store();
        let context_id = ContextId::from([0x0Fu8; 32]);
        let author_sk = PrivateKey::from([0x10u8; 32]);
        let author = author_sk.public_key();

        let (_group_id, key_id, group_key_bytes) = seed_group_key(&store, context_id);
        store_local_identity(&store, &context_id, &author_sk);

        let material = resolve_publish_material(&store, context_id, author).expect("material");
        assert_eq!(material.key_id, key_id);
        assert_eq!(material.group_key, group_key_bytes);
        assert_eq!(material.signing_key, *author_sk.as_bytes());
    }

    // -----------------------------------------------------------------------
    // Test 6: the published envelope carries a signature that verifies under
    //          the author's identity key. This is the send-side half of the
    //          anti-impersonation guarantee.
    // -----------------------------------------------------------------------

    #[actix::test]
    async fn published_message_carries_a_verifiable_signature() {
        use crate::handlers::ephemeral::auth::verify_ephemeral_signature;

        let store = fresh_store();
        let context_id = ContextId::from([0x71u8; 32]);

        let (_group_id, _key_id, _) = seed_group_key(&store, context_id);

        // The author must be a local signing identity, or signing cannot
        // resolve a key. `store_local_identity` seeds the ContextIdentity row
        // that `resolve_local_signing_key` reads.
        let sk = PrivateKey::from([0x74u8; 32]);
        let author = sk.public_key();
        store_local_identity(&store, &context_id, &sk);

        let (network_client, published) = recording_network_client();
        let material =
            resolve_publish_material(&store, context_id, author).expect("publish material");

        do_publish_ephemeral(
            &network_client,
            context_id,
            author,
            3,
            b"hi".to_vec(),
            material,
        )
        .await
        .expect("publish");

        let sent = published.lock().expect("lock").clone();
        assert_eq!(sent.len(), 1, "exactly one publish");
        let msg: BroadcastMessage<'_> = borsh::from_slice(&sent[0].1).expect("decode");
        let BroadcastMessage::Ephemeral {
            author: got_author,
            seq,
            key_id,
            sent_at_ms,
            nonce,
            ciphertext,
            signature,
            ..
        } = msg
        else {
            panic!("expected Ephemeral");
        };
        let envelope = SignedEnvelope {
            context_id,
            author: got_author,
            seq,
            key_id,
            sent_at_ms,
            nonce,
            ciphertext: &ciphertext,
        };
        verify_ephemeral_signature(envelope, &signature).expect("published signature must verify");

        // The nonce is bound into the signed payload, so a relay flipping it to
        // make the receiver's AEAD fail breaks the signature instead of causing
        // an undetectable silent drop.
        let mut tampered = envelope;
        tampered.nonce = [0xFFu8; calimero_crypto::NONCE_LEN];
        assert!(
            verify_ephemeral_signature(tampered, &signature).is_err(),
            "the published signature must cover the AEAD nonce"
        );

        // The stamp must be a real publish-time reading, not a placeholder: a
        // receiver judges freshness against it, so a zero (or wildly off) value
        // would make every publish from this node undeliverable.
        let now = now_ms();
        assert!(
            now.abs_diff(sent_at_ms) < 60_000,
            "sent_at_ms must be stamped from the publish-time wall clock, got {sent_at_ms} (now {now})"
        );
    }

    // -----------------------------------------------------------------------
    // Test 7: a local author's own entry survives past PRESENCE_TTL_MS as
    //          long as the heartbeat keeps ticking.
    //
    //          A node never receives its own gossip back, so the heartbeat is
    //          the ONLY thing that can re-stamp its own `last_seen_ms`. Before
    //          the touch was wired in, the node's own sweep evicted its own
    //          presence one TTL after `set_local_ephemeral`.
    // -----------------------------------------------------------------------

    #[test]
    fn local_author_survives_ttl_across_heartbeats() {
        use crate::handlers::ephemeral::store::{AwarenessStore, Diff};

        let context_id = ContextId::from([0x21u8; 32]);
        let local_author = PublicKey::from([0x22u8; 32]);
        let remote_author = PublicKey::from([0x23u8; 32]);

        let mut store = AwarenessStore::new();
        let t0 = 1_000_000u64;
        let _ = store.apply(context_id, local_author, 1, b"cursor".to_vec(), t0);
        let _ = store.apply(context_id, remote_author, 1, b"remote".to_vec(), t0);

        let local = [(context_id, local_author)];

        // Tick the heartbeat every PRESENCE_HEARTBEAT_MS well past the TTL.
        let ticks = PRESENCE_TTL_MS / PRESENCE_HEARTBEAT_MS + 4;
        let mut removed_local = 0;
        let mut removed_remote = 0;
        for i in 1..=ticks {
            let now = t0 + i * PRESENCE_HEARTBEAT_MS;
            for (ctx, diff) in refresh_and_sweep(&mut store, &local, PRESENCE_TTL_MS, now) {
                assert_eq!(ctx, context_id);
                match diff {
                    Diff::Remove { author } if author == local_author => removed_local += 1,
                    Diff::Remove { author } if author == remote_author => removed_remote += 1,
                    other => panic!("unexpected diff {other:?}"),
                }
            }
        }

        let elapsed = ticks * PRESENCE_HEARTBEAT_MS;
        assert!(
            elapsed > PRESENCE_TTL_MS,
            "the test must run past the TTL to be meaningful (elapsed {elapsed}ms, ttl {PRESENCE_TTL_MS}ms)"
        );
        assert_eq!(
            removed_local, 0,
            "the local author must never be swept while it is heartbeating"
        );
        assert_eq!(
            removed_remote, 1,
            "a remote author that stopped heartbeating must still be swept exactly once"
        );

        let live: Vec<_> = store
            .snapshot(context_id, t0 + elapsed)
            .into_iter()
            .map(|(author, slice, _age)| (author, slice))
            .collect();
        assert_eq!(
            live,
            vec![(local_author, b"cursor".to_vec())],
            "only the local author's entry remains live"
        );
    }

    // -----------------------------------------------------------------------
    // Test 8: without the local touch, the sweep would evict the local author
    //          — the falsifier for Test 7 (proves the touch is what saves it,
    //          not the TTL arithmetic).
    // -----------------------------------------------------------------------

    #[test]
    fn without_local_touch_the_local_author_is_evicted() {
        use crate::handlers::ephemeral::store::{AwarenessStore, Diff};

        let context_id = ContextId::from([0x24u8; 32]);
        let local_author = PublicKey::from([0x25u8; 32]);

        let mut store = AwarenessStore::new();
        let t0 = 1_000_000u64;
        let _ = store.apply(context_id, local_author, 1, b"cursor".to_vec(), t0);

        // Empty `local` == the pre-fix behaviour (sweep only, no touch).
        let diffs = refresh_and_sweep(&mut store, &[], PRESENCE_TTL_MS, t0 + PRESENCE_TTL_MS);
        assert_eq!(
            diffs,
            vec![(
                context_id,
                Diff::Remove {
                    author: local_author
                }
            )],
            "with no touch the entry is evicted at the TTL boundary"
        );
    }
}
