//! Inbound ephemeral-presence dispatch: gossip → decrypt → awareness store → client event.
//!
//! **No state-delta, no RocksDB writes.** This module's only storage
//! interaction is a read of the group's *current* key record (via
//! `GroupKeyring::load_current_key_record`) to decrypt the sealed presence
//! slice before handing it to the in-memory `AwarenessStore`. Unlike the
//! state-delta path, presence never accepts a key this node's own keyring
//! resolves as *superseded* — the keyring retains those for historical
//! decrypt only, and presence has no history — so a `key_id` that does not
//! match what `load_current_key_record` returns is a silent drop.
//!
//! **Rotation-as-eviction rests on `load_current_key_record`'s ordering.**
//! `GroupKeyring::store_key` stamps every key at epoch 0 (only
//! `store_key_with_epoch` sets a real DAG epoch), so a node can hold two
//! epoch-0 keys — e.g. it learned a post-rotation key by direct pull rather
//! than by applying the rotation op. Equal epoch-0 keys are ordered by the
//! keyring's per-group `insertion_seq` (the order this node learned them),
//! **not** by `key_id` hash order, precisely so the older key can never be
//! resolved as "current" here: were it, this module would drop every
//! legitimate member's presence and — since the superseded key still decrypts
//! a captured envelope — treat the rotated-out holder of that key as current
//! instead. Equal *non-zero* epochs still tie-break by `key_id`, which is what
//! makes concurrent rotations converge across nodes; both of those keys are
//! current by construction, so either is safe here.
//!
//! **Security — the wire `author` and the publish time are signed.** Every
//! presence envelope carries an ed25519 signature, by `author`'s identity key,
//! over `(context_id, author, seq, key_id, sent_at_ms, nonce,
//! sha256(ciphertext))` —
//! see [`crate::handlers::ephemeral::auth`]. `resolve_and_decrypt` verifies
//! this signature after AEAD decryption and before the plaintext is handed to
//! the `AwarenessStore`; a mismatch (including a group-key holder stamping
//! another member's `author`) is a silent drop, same as an unknown `key_id`, a
//! `sent_at_ms` outside the freshness window, or a failed AEAD decrypt. See
//! [`EphemeralPayload`] for the client-facing note.
//!
//! [`EphemeralPayload`]: calimero_primitives::events::EphemeralPayload

use actix::{ActorFutureExt, AsyncContext, WrapFuture};
use calimero_context_client::client::ContextClient;
use calimero_context_config::types::ContextGroupId;
use calimero_crypto::Nonce;
use calimero_node_primitives::client::NodeClient;
use calimero_primitives::context::ContextId;
use calimero_primitives::events::{ContextEvent, ContextEventPayload, EphemeralPayload, NodeEvent};
use calimero_primitives::identity::PublicKey;
use tracing::debug;

use crate::handlers::ephemeral::store::Diff;
use crate::handlers::ephemeral::EPHEMERAL_MAX_BYTES;
use crate::NodeManager;

/// Maximum on-wire ciphertext length accepted on the receive path:
/// [`EPHEMERAL_MAX_BYTES`] of plaintext plus the fixed AEAD tag overhead.
/// Checked before any clone or decrypt so an oversized envelope (even one
/// that would still fit under gossipsub's own 1 MiB ceiling) is dropped
/// without paying for the allocation or pinning it in the `AwarenessStore`.
const EPHEMERAL_MAX_CIPHERTEXT_BYTES: usize = EPHEMERAL_MAX_BYTES + calimero_crypto::AEAD_TAG_LEN;

// ---------------------------------------------------------------------------
// Inner async logic (testable without actix)
// ---------------------------------------------------------------------------

/// Resolve the group key for `context_id`, decrypt `ciphertext`, and verify
/// the envelope's freshness and signature.
///
/// Returns `None` when `context_id` has no group, when `key_id` is not the
/// group's *current* key (unknown key ids and superseded keys are treated
/// identically — presence has no history, so only the current key is ever
/// accepted), when `sent_at_ms` sits outside the freshness window, when the
/// AEAD authentication fails, or when the envelope signature does not verify
/// under `author`. All cases are silent drops — ephemeral presence is
/// best-effort.
///
/// `now_ms` is passed in rather than read here so the gate is deterministic
/// under test, matching the `AwarenessStore`'s convention.
///
/// Never writes to the DAG, RocksDB, or any persistent store.
pub(crate) async fn resolve_and_decrypt(
    context_client: &ContextClient,
    context_id: ContextId,
    author: PublicKey,
    seq: u64,
    key_id: [u8; 32],
    sent_at_ms: u64,
    now_ms: u64,
    nonce: Nonce,
    ciphertext: Vec<u8>,
    signature: [u8; 64],
) -> Option<Vec<u8>> {
    // Derive the ContextGroupId the same way the state-delta handler does:
    // `get_group_for_context` reads the context-tree row that
    // `register_context_in_group` wrote at group-creation time. On `None`
    // (context not in any group) the message is not decryptable — drop.
    let store = context_client.datastore();
    let group_id: ContextGroupId =
        match calimero_governance_store::get_group_for_context(store, &context_id) {
            Ok(Some(gid)) => gid,
            Ok(None) => {
                debug!(%context_id, "ephemeral: context has no group — dropping");
                return None;
            }
            Err(err) => {
                debug!(%context_id, %err, "ephemeral: group lookup error — dropping");
                return None;
            }
        };

    // Presence is transient and has no history, so only the CURRENT group key is
    // acceptable. The keyring deliberately retains superseded keys for
    // state-delta decryption; accepting them here would let a rotated-out member
    // keep publishing presence straight through a rotation, defeating rotation
    // as the eviction mechanism.
    //
    // Deliberately does not touch `lookup_group_key_with_wait` — the state-delta
    // path still needs historical keys.
    let record = match calimero_governance_store::GroupKeyring::new(store, group_id)
        .load_current_key_record()
    {
        Ok(Some(r)) => r,
        Ok(None) => {
            debug!(%context_id, "ephemeral: no current group key — dropping");
            return None;
        }
        Err(err) => {
            debug!(%context_id, %err, "ephemeral: current key lookup error — dropping");
            return None;
        }
    };
    if record.key_id != key_id {
        // Distinguish "we have never seen this key_id" from "we know this
        // key_id but our keyring no longer resolves it as current". The
        // second case is the expected, benign one right after a rotation (a
        // peer still publishing under the old key); it is also the signal an
        // operator would see if `load_current_key_record` ever ordered two
        // keys wrongly — see the ordering note at the top of this module.
        let keyring = calimero_governance_store::GroupKeyring::new(store, group_id);
        let known_but_superseded = matches!(keyring.load_key_by_id(&key_id), Ok(Some(_)));
        debug!(
            %context_id,
            key_id = %hex::encode(key_id),
            current = %hex::encode(record.key_id),
            known_but_superseded,
            "ephemeral: key_id is not the current group key — dropping"
        );
        return None;
    }

    // Freshness. Sits here deliberately: after the current-key check (a single
    // keyring read that most junk fails on) and before the decrypt and the
    // ed25519 verify, which are the expensive gates — a replay should cost the
    // receiver an integer subtraction, not a signature verification.
    //
    // `sent_at_ms` is covered by the envelope signature, so a mesh peer
    // replaying recorded bytes cannot restamp it to look fresh; verifying that
    // binding is the signature check further down.
    if !crate::handlers::ephemeral::auth::is_fresh(now_ms, sent_at_ms) {
        debug!(
            %context_id,
            %author,
            sent_at_ms,
            now_ms,
            max_skew_ms = crate::handlers::ephemeral::PRESENCE_MAX_SKEW_MS,
            "ephemeral: sent_at_ms outside the freshness window — dropping (replay or clock skew)"
        );
        return None;
    }

    let key = calimero_primitives::identity::PrivateKey::from(record.group_key);

    // Enforce the documented size cap on the receive path too. Outbound only
    // enforces `EPHEMERAL_MAX_BYTES` on the sender's own plaintext; a patched
    // or malicious peer can put anything up to gossipsub's 1 MiB ceiling on
    // the wire. Reject before the clone/allocate/decrypt below so an
    // oversized envelope never gets pinned in the `AwarenessStore`.
    if ciphertext.len() > EPHEMERAL_MAX_CIPHERTEXT_BYTES {
        debug!(
            %context_id,
            len = ciphertext.len(),
            max = EPHEMERAL_MAX_CIPHERTEXT_BYTES,
            "ephemeral: ciphertext exceeds size cap — dropping"
        );
        return None;
    }

    // `SharedKey::decrypt` consumes `ciphertext` by value, but the signature
    // verify below needs the exact bytes that arrived on the wire — capture
    // them before decrypting.
    let ciphertext_for_verify = ciphertext.clone();

    // AEAD decrypt. `SharedKey::from_sk` derives the symmetric key from the
    // stored group private key; `decrypt` returns `None` on authentication
    // failure (wrong key or tampered ciphertext). Drop silently either way —
    // a decryption failure on the ephemeral path should not produce a log
    // storm; only debug-level is emitted.
    let plaintext = calimero_crypto::SharedKey::from_sk(&key).decrypt(ciphertext, nonce);
    let plaintext = match plaintext {
        Some(plaintext) => plaintext,
        None => {
            debug!(
                %context_id,
                "ephemeral: AEAD decrypt failed — dropping"
            );
            return None;
        }
    };

    // Envelope signature. Sits after decryption so the ed25519 verify stays
    // off the path for traffic that could not produce valid ciphertext, and
    // before the awareness store so a forged author never becomes visible
    // state.
    if let Err(err) = crate::handlers::ephemeral::auth::verify_ephemeral_signature(
        crate::handlers::ephemeral::auth::SignedEnvelope {
            context_id,
            author,
            seq,
            key_id,
            sent_at_ms,
            nonce,
            ciphertext: &ciphertext_for_verify,
        },
        &signature,
    ) {
        debug!(
            %context_id,
            %author,
            %err,
            "ephemeral: envelope signature verification failed — dropping"
        );
        return None;
    }

    Some(plaintext)
}

// ---------------------------------------------------------------------------
// Emit helper (testable without actix)
// ---------------------------------------------------------------------------

/// Convert a `Diff` from the `AwarenessStore` into a `NodeEvent::Context`
/// and deliver it to WebSocket subscribers via `node_client.send_event`.
///
/// Infallible to callers: a failed send (no receivers) is logged at debug
/// and discarded — client events are best-effort.
///
/// `age_ms` is always `None` here: this is the *live* path, and a delta is
/// fresh by construction — it is emitted the instant the awareness store
/// changed, so the subscriber's own receipt time is a better reading than
/// anything this node could stamp. Age is carried only on the replay path
/// (`calimero-server`'s subscribe handlers), where the entry may be arbitrarily
/// old within the TTL window. See [`EphemeralPayload::age_ms`].
pub(crate) fn emit_ephemeral_diff(node_client: &NodeClient, context_id: ContextId, diff: Diff) {
    let payload = match diff {
        Diff::Upsert { author, slice } => ContextEventPayload::Ephemeral(EphemeralPayload {
            author,
            state: Some(slice),
            removed: false,
            age_ms: None,
        }),
        Diff::Remove { author } => ContextEventPayload::Ephemeral(EphemeralPayload {
            author,
            state: None,
            removed: true,
            age_ms: None,
        }),
    };

    let event = NodeEvent::Context(ContextEvent {
        context_id,
        payload,
    });
    if let Err(err) = node_client.send_event(event) {
        debug!(%context_id, %err, "ephemeral: failed to deliver context event (no subscribers)");
    }
}

// ---------------------------------------------------------------------------
// Actix entry point
// ---------------------------------------------------------------------------

/// Handle an inbound `BroadcastMessage::Ephemeral` gossip message.
///
/// Wires the async key-resolution / decrypt path (`resolve_and_decrypt`)
/// onto the actor's Arbiter via `ctx.spawn`, then in the synchronous
/// `.map()` callback applies the decrypted slice to the `AwarenessStore`
/// and emits any resulting `Diff` as a `NodeEvent::Context(Ephemeral)` on
/// the node's event broadcast sink.
///
/// **Never touches state-delta, RocksDB, or the DAG.**
pub(crate) fn handle_ephemeral_broadcast(
    this: &mut NodeManager,
    ctx: &mut actix::Context<NodeManager>,
    context_id: ContextId,
    author: PublicKey,
    seq: u64,
    key_id: [u8; 32],
    sent_at_ms: u64,
    nonce: Nonce,
    ciphertext: Vec<u8>,
    signature: [u8; 64],
) {
    let context_client = this.clients.context.clone();
    let node_client = this.clients.node.clone();

    // now_ms: milliseconds since UNIX epoch (wall clock). Read once, here, and
    // used for BOTH the freshness gate and the awareness-store stamp, so an
    // envelope accepted as fresh is recorded against the same reading it was
    // judged by. The AwarenessStore and the freshness gate are both
    // time-parameterised so tests can inject arbitrary timestamps; callers
    // supply the wall-clock reading. See `ephemeral::now_ms` for what a
    // pre-epoch clock degrades to (it freezes presence; it does not expire it).
    let now_ms = crate::handlers::ephemeral::now_ms();

    let _ignored = ctx.spawn(
        async move {
            // Resolve key + freshness + decrypt + verify signature: the only
            // async work.
            resolve_and_decrypt(
                &context_client,
                context_id,
                author,
                seq,
                key_id,
                sent_at_ms,
                now_ms,
                nonce,
                ciphertext,
                signature,
            )
            .await
        }
        .into_actor(this)
        .map(move |plaintext, actor, _ctx| {
            let Some(slice) = plaintext else {
                // Already logged inside resolve_and_decrypt.
                return;
            };

            // May yield two diffs: admitting a new author into a full context
            // evicts the stalest one, and clients must be told it went away.
            for diff in actor
                .awareness_store
                .apply(context_id, author, seq, slice, now_ms)
            {
                emit_ephemeral_diff(&node_client, context_id, diff);
            }
        }),
    );
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use calimero_blobstore::config::BlobStoreConfig;
    use calimero_blobstore::{BlobManager as BlobStore, FileSystem};
    use calimero_context_client::client::ContextClient;
    use calimero_context_config::types::ContextGroupId;
    use calimero_crypto::{SharedKey, NONCE_LEN};
    use calimero_governance_store::{register_context_in_group, GroupKeyring};
    use calimero_network_primitives::client::NetworkClient;
    use calimero_node_primitives::client::{BlobManager, NodeClient, SyncClient};
    use calimero_primitives::context::ContextId;
    use calimero_primitives::events::{ContextEventPayload, NodeEvent};
    use calimero_primitives::identity::{PrivateKey, PublicKey};
    use calimero_store::db::InMemoryDB;
    use calimero_store::Store;
    use calimero_utils_actix::LazyRecipient;
    use tokio::sync::{broadcast, mpsc};

    use super::*;
    use crate::handlers::ephemeral::auth::SignedEnvelope;
    use crate::handlers::ephemeral::store::AwarenessStore;

    // -----------------------------------------------------------------------
    // Shared test scaffolding (mirrors test_support.rs patterns)
    // -----------------------------------------------------------------------

    /// Fixed wall clock for the receive-path tests. `SENT_AT == NOW` means
    /// every envelope built with these is inside the freshness window, so a
    /// test that is about some *other* gate cannot pass (or fail) because of
    /// this one.
    const NOW: u64 = 1_700_000_000_000;
    const SENT_AT: u64 = NOW;

    fn fresh_store() -> Store {
        Store::new(Arc::new(InMemoryDB::owned()))
    }

    /// A fully-wired (but inert) `NodeClient` with a live event receiver.
    /// Returns the client, the event receiver, and a `TempDir` guard that
    /// must be kept alive for the blob filesystem.
    async fn node_client_with_rx(
        store: Store,
    ) -> (
        NodeClient,
        broadcast::Receiver<NodeEvent>,
        tempfile::TempDir,
    ) {
        let tmp = tempfile::tempdir().expect("tempdir");
        let blob_config = BlobStoreConfig::new(
            std::path::PathBuf::from(tmp.path())
                .try_into()
                .expect("utf8 path"),
        );
        let file_system = FileSystem::new(&blob_config).await.expect("blob fs");
        let blob_store = BlobStore::new(store.clone(), file_system);
        let blob_manager = BlobManager::new(blob_store);
        let network_client = NetworkClient::new(LazyRecipient::new());

        let (event_sender, event_rx) = broadcast::channel(16);
        let (ctx_sync_tx, _ctx_sync_rx) = mpsc::channel(1);
        let (ns_sync_tx, _ns_sync_rx) = mpsc::channel(1);
        let (ns_join_tx, _ns_join_rx) = mpsc::channel(1);
        let (open_subgroup_join_tx, _open_subgroup_join_rx) = mpsc::channel(1);
        let sync_client =
            SyncClient::new(ctx_sync_tx, ns_sync_tx, ns_join_tx, open_subgroup_join_tx);

        let client = NodeClient::new(
            store,
            blob_manager,
            network_client,
            LazyRecipient::new(),
            event_sender,
            sync_client,
            None,
        );
        (client, event_rx, tmp)
    }

    /// Seed a group key and register a context into the group.
    /// Returns `(group_id, key_id, group_key_bytes)`.
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

    /// Encrypt `plaintext` under `group_key_bytes` with a fixed nonce.
    fn encrypt_slice(group_key_bytes: &[u8; 32], plaintext: &[u8]) -> (Vec<u8>, [u8; NONCE_LEN]) {
        let nonce = [0x11u8; NONCE_LEN];
        let sk = PrivateKey::from(*group_key_bytes);
        let cipher = SharedKey::from_sk(&sk)
            .encrypt_with_nonce(plaintext.to_vec(), nonce)
            .expect("encrypt");
        (cipher, nonce)
    }

    // -----------------------------------------------------------------------
    // Test 1: decryptable slice emits an Ephemeral event + populates store
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn decryptable_slice_emits_event_and_populates_store() {
        let store = fresh_store();
        let context_id = ContextId::from([0x01u8; 32]);
        let author_sk = PrivateKey::from([0x02u8; 32]);
        let author = author_sk.public_key();
        let seq = 1u64;

        let (_group_id, key_id, group_key_bytes) = seed_group_key(&store, context_id);
        let slice = b"cursor={x:42,y:10}";
        let (ciphertext, nonce) = encrypt_slice(&group_key_bytes, slice);
        let payload =
            crate::handlers::ephemeral::auth::ephemeral_signature_payload(SignedEnvelope {
                context_id,
                author,
                seq,
                key_id,
                sent_at_ms: SENT_AT,
                nonce,
                ciphertext: &ciphertext,
            })
            .expect("payload");
        let signature = author_sk.sign(&payload).expect("sign").to_bytes();

        let (node_client_for_ctx, _rx_ctx, _tmp_ctx) = node_client_with_rx(fresh_store()).await;
        let ctx_client = ContextClient::new(store, node_client_for_ctx, LazyRecipient::new());

        let (node_client, mut event_rx, _tmp) = node_client_with_rx(fresh_store()).await;

        // Resolve + decrypt + verify the ciphertext.
        let plaintext = resolve_and_decrypt(
            &ctx_client,
            context_id,
            author,
            seq,
            key_id,
            SENT_AT,
            NOW,
            nonce,
            ciphertext,
            signature,
        )
        .await
        .expect("should decrypt — key is seeded and signature is valid");

        assert_eq!(
            plaintext.as_slice(),
            slice.as_ref(),
            "decrypted bytes must match"
        );

        // Apply to store and emit event.
        let now_ms = 1_000u64;
        let mut store_under_test = AwarenessStore::new();
        let mut diffs = store_under_test.apply(context_id, author, 1, plaintext.clone(), now_ms);
        assert_eq!(
            diffs.len(),
            1,
            "a new entry under the cap must produce exactly one Diff::Upsert"
        );
        let diff = diffs.pop().expect("just asserted one diff");

        emit_ephemeral_diff(&node_client, context_id, diff);

        // Assert the event reached the subscriber.
        let event = event_rx.try_recv().expect("event must have been emitted");
        // `NodeEvent` gained a `GroupMembership` variant upstream, so this
        // binding is no longer irrefutable — anything else is a test failure.
        let NodeEvent::Context(ctx_event) = event else {
            panic!("expected NodeEvent::Context");
        };
        assert_eq!(ctx_event.context_id, context_id);
        let ContextEventPayload::Ephemeral(payload) = ctx_event.payload else {
            panic!("expected ContextEventPayload::Ephemeral");
        };
        assert_eq!(payload.author, author);
        assert_eq!(
            payload.state.as_deref(),
            Some(slice.as_ref()),
            "event state must be the decrypted slice"
        );
        assert!(!payload.removed, "upsert must not be marked removed");

        // Assert the store snapshot reflects the entry.
        let snapshot = store_under_test.snapshot(context_id, now_ms);
        assert_eq!(snapshot.len(), 1);
        assert_eq!(snapshot[0].0, author);
        assert_eq!(snapshot[0].1.as_slice(), slice.as_ref());
    }

    // -----------------------------------------------------------------------
    // Test 2: unknown key_id → silent drop, no event, no error
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn unknown_key_id_is_dropped_silently() {
        use crate::handlers::ephemeral::auth::ephemeral_signature_payload;

        let store = fresh_store();
        let context_id = ContextId::from([0x03u8; 32]);
        let author_sk = PrivateKey::from([0x09u8; 32]);
        let author = author_sk.public_key();

        // Seed the group so context resolution works, but use a wrong key_id.
        let (_group_id, _real_key_id, group_key_bytes) = seed_group_key(&store, context_id);
        let slice = b"should not arrive";
        let (ciphertext, nonce) = encrypt_slice(&group_key_bytes, slice);

        let (node_client_for_ctx, _rx_ctx, _tmp_ctx) = node_client_with_rx(fresh_store()).await;
        let ctx_client = ContextClient::new(store, node_client_for_ctx, LazyRecipient::new());

        // key_id that was never stored. The signature is VALID — computed
        // over this exact (wrong) key_id — so the only thing that can stop
        // this message is the key_id-vs-current comparison. A garbage
        // signature here would let this test pass for the wrong reason (the
        // signature gate, not the key gate) even if the key check were
        // deleted.
        let wrong_key_id = [0x00u8; 32];
        let payload = ephemeral_signature_payload(SignedEnvelope {
            context_id,
            author,
            seq: 1,
            key_id: wrong_key_id,
            sent_at_ms: SENT_AT,
            nonce,
            ciphertext: &ciphertext,
        })
        .expect("payload");
        let signature = author_sk.sign(&payload).expect("sign").to_bytes();

        let result = resolve_and_decrypt(
            &ctx_client,
            context_id,
            author,
            1,
            wrong_key_id,
            SENT_AT,
            NOW,
            nonce,
            ciphertext,
            signature,
        )
        .await;

        // Must return None — unknown key_id → silent drop.
        assert!(
            result.is_none(),
            "unknown key_id must resolve to None, not an error or panic"
        );

        // The caller would not call emit_ephemeral_diff on None, so no event
        // reaches any subscriber. Confirm directly by checking that a
        // properly-wired channel stays empty (the NodeClient must be kept
        // alive so the sender is not dropped — a dropped sender would turn
        // try_recv into Err(Closed) rather than Err(Empty)).
        let (node_client_check, mut event_rx, _tmp) = node_client_with_rx(fresh_store()).await;
        match event_rx.try_recv() {
            Err(broadcast::error::TryRecvError::Empty) => { /* correct: no event */ }
            other => panic!("expected empty channel, got {other:?}"),
        }
        drop(node_client_check);
    }

    // -----------------------------------------------------------------------
    // Test: a superseded key must not be accepted, even though it still
    // decrypts — this is what makes key rotation an eviction mechanism.
    // -----------------------------------------------------------------------

    // Presence has no history, so a superseded key must not be accepted even
    // though it is still in the keyring for state-delta decryption. This is
    // what makes key rotation an eviction mechanism.
    #[tokio::test]
    async fn superseded_key_is_dropped_even_though_it_decrypts() {
        use crate::handlers::ephemeral::auth::ephemeral_signature_payload;

        let store = fresh_store();
        let context_id = ContextId::from([0x91u8; 32]);
        let group_id = ContextGroupId::from([0x92u8; 32]);
        register_context_in_group(&store, &group_id, &context_id).expect("register");

        // Old key at epoch 0, then a rotation to a newer key at epoch 5.
        let old_key = [0x93u8; 32];
        let old_key_id = GroupKeyring::new(&store, group_id)
            .store_key(&old_key)
            .expect("store old key");
        let new_key = [0x94u8; 32];
        let _new_key_id = GroupKeyring::new(&store, group_id)
            .store_key_with_epoch(&new_key, 5)
            .expect("store new key");

        // A correctly signed message — but sealed under the SUPERSEDED key.
        let (ciphertext, nonce) = encrypt_slice(&old_key, b"stale");
        let sk = PrivateKey::from([0x95u8; 32]);
        let author = sk.public_key();
        let payload = ephemeral_signature_payload(SignedEnvelope {
            context_id,
            author,
            seq: 1,
            key_id: old_key_id,
            sent_at_ms: SENT_AT,
            nonce,
            ciphertext: &ciphertext,
        })
        .expect("payload");
        let signature = sk.sign(&payload).expect("sign").to_bytes();

        let (node_client_for_ctx, _rx_ctx, _tmp_ctx) = node_client_with_rx(fresh_store()).await;
        let ctx_client = ContextClient::new(store, node_client_for_ctx, LazyRecipient::new());

        let out = resolve_and_decrypt(
            &ctx_client,
            context_id,
            author,
            1,
            old_key_id,
            SENT_AT,
            NOW,
            nonce,
            ciphertext,
            signature,
        )
        .await;

        assert!(
            out.is_none(),
            "a superseded key must be refused on the presence path"
        );
    }

    // -----------------------------------------------------------------------
    // THE regression guard for this whole plan: a message signed by A but
    // claiming B as author must be dropped, even though it decrypts cleanly
    // under the shared group key.
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn forged_author_is_dropped() {
        use crate::handlers::ephemeral::auth::ephemeral_signature_payload;

        let store = fresh_store();
        let context_id = ContextId::from([0x81u8; 32]);

        let (_group_id, key_id, group_key_bytes) = seed_group_key(&store, context_id);
        let (ciphertext, nonce) = encrypt_slice(&group_key_bytes, b"forged");

        // Attacker holds the group key and signs correctly — with their OWN
        // key — but stamps the victim's public key as `author`.
        let attacker = PrivateKey::from([0x84u8; 32]);
        let victim = PrivateKey::from([0x85u8; 32]).public_key();
        let payload = ephemeral_signature_payload(SignedEnvelope {
            context_id,
            author: victim,
            seq: 1,
            key_id,
            sent_at_ms: SENT_AT,
            nonce,
            ciphertext: &ciphertext,
        })
        .expect("payload");
        let signature = attacker.sign(&payload).expect("sign").to_bytes();

        let (node_client_for_ctx, _rx_ctx, _tmp_ctx) = node_client_with_rx(fresh_store()).await;
        let ctx_client = ContextClient::new(store, node_client_for_ctx, LazyRecipient::new());

        let out = resolve_and_decrypt(
            &ctx_client,
            context_id,
            victim,
            1,
            key_id,
            SENT_AT,
            NOW,
            nonce,
            ciphertext,
            signature,
        )
        .await;

        assert!(out.is_none(), "a forged author must not decrypt through");
    }

    // Positive control: a correctly signed message still passes, so the guard
    // above is proving something.
    #[tokio::test]
    async fn correctly_signed_message_passes() {
        use crate::handlers::ephemeral::auth::ephemeral_signature_payload;

        let store = fresh_store();
        let context_id = ContextId::from([0x86u8; 32]);

        let (_group_id, key_id, group_key_bytes) = seed_group_key(&store, context_id);
        let (ciphertext, nonce) = encrypt_slice(&group_key_bytes, b"genuine");
        let sk = PrivateKey::from([0x89u8; 32]);
        let author = sk.public_key();
        let payload = ephemeral_signature_payload(SignedEnvelope {
            context_id,
            author,
            seq: 1,
            key_id,
            sent_at_ms: SENT_AT,
            nonce,
            ciphertext: &ciphertext,
        })
        .expect("payload");
        let signature = sk.sign(&payload).expect("sign").to_bytes();

        let (node_client_for_ctx, _rx_ctx, _tmp_ctx) = node_client_with_rx(fresh_store()).await;
        let ctx_client = ContextClient::new(store, node_client_for_ctx, LazyRecipient::new());

        let out = resolve_and_decrypt(
            &ctx_client,
            context_id,
            author,
            1,
            key_id,
            SENT_AT,
            NOW,
            nonce,
            ciphertext,
            signature,
        )
        .await;

        assert_eq!(out.as_deref(), Some(b"genuine".as_ref()));
    }

    // -----------------------------------------------------------------------
    // Replay protection: a recorded envelope re-injected after the freshness
    // window must be refused, even though everything else about it is still
    // valid (current key, intact AEAD, genuine signature). This is the whole
    // point of `sent_at_ms` — without it, a mesh peer holding no group key
    // could keep a departed author rendered present indefinitely.
    // -----------------------------------------------------------------------

    /// Build a fully valid envelope stamped at `sent_at_ms`, then hand it to
    /// `resolve_and_decrypt` at wall clock `now_ms`.
    async fn replayed_at(sent_at_ms: u64, now_ms: u64) -> Option<Vec<u8>> {
        use crate::handlers::ephemeral::auth::ephemeral_signature_payload;

        let store = fresh_store();
        let context_id = ContextId::from([0x8Au8; 32]);
        let (_group_id, key_id, group_key_bytes) = seed_group_key(&store, context_id);
        let (ciphertext, nonce) = encrypt_slice(&group_key_bytes, b"recorded");
        let sk = PrivateKey::from([0x8Bu8; 32]);
        let author = sk.public_key();
        let payload = ephemeral_signature_payload(SignedEnvelope {
            context_id,
            author,
            seq: 1,
            key_id,
            sent_at_ms,
            nonce,
            ciphertext: &ciphertext,
        })
        .expect("payload");
        let signature = sk.sign(&payload).expect("sign").to_bytes();

        let (node_client_for_ctx, _rx_ctx, _tmp_ctx) = node_client_with_rx(fresh_store()).await;
        let ctx_client = ContextClient::new(store, node_client_for_ctx, LazyRecipient::new());

        resolve_and_decrypt(
            &ctx_client,
            context_id,
            author,
            1,
            key_id,
            sent_at_ms,
            now_ms,
            nonce,
            ciphertext,
            signature,
        )
        .await
    }

    #[tokio::test]
    async fn stale_envelope_replayed_after_the_window_is_dropped() {
        use crate::handlers::ephemeral::PRESENCE_MAX_SKEW_MS;

        // Recorded when the author was live, re-injected long after they left
        // — well past the TTL sweep, so the receiver holds no entry and the
        // LWW `seq` rule would happily accept it.
        let sent_at = NOW;
        let replay_at = NOW + PRESENCE_MAX_SKEW_MS + 1;
        assert!(
            replayed_at(sent_at, replay_at).await.is_none(),
            "an envelope replayed outside the freshness window must be dropped"
        );
    }

    #[tokio::test]
    async fn fresh_envelope_inside_the_window_passes() {
        use crate::handlers::ephemeral::PRESENCE_MAX_SKEW_MS;

        // Positive control for the test above: same envelope, delivered inside
        // the window (including at the exact edge), still gets through — so
        // the drop above is the freshness gate and not some other rejection.
        assert_eq!(
            replayed_at(NOW, NOW + 1_000).await.as_deref(),
            Some(b"recorded".as_ref()),
            "a fresh envelope must pass"
        );
        assert_eq!(
            replayed_at(NOW, NOW + PRESENCE_MAX_SKEW_MS)
                .await
                .as_deref(),
            Some(b"recorded".as_ref()),
            "an envelope exactly at the window edge must still pass"
        );
    }

    #[tokio::test]
    async fn far_future_sent_at_ms_is_dropped() {
        use crate::handlers::ephemeral::PRESENCE_MAX_SKEW_MS;

        // A stamp far in the future is as suspicious as a stale one: left
        // accepted, it would keep a recorded envelope replayable for as long
        // as the attacker post-dated it.
        assert!(
            replayed_at(NOW + PRESENCE_MAX_SKEW_MS + 1, NOW)
                .await
                .is_none(),
            "a far-future sent_at_ms must be dropped"
        );
    }

    /// The freshness stamp is bound INTO the signature, not merely carried
    /// beside it: an attacker who rewrites the wire field to look fresh fails
    /// the signature check instead. Without this, `sent_at_ms` would be
    /// decoration — it rides outside the AEAD, in the clear.
    #[tokio::test]
    async fn restamped_sent_at_ms_fails_signature_verification() {
        use crate::handlers::ephemeral::auth::ephemeral_signature_payload;

        let store = fresh_store();
        let context_id = ContextId::from([0x8Cu8; 32]);
        let (_group_id, key_id, group_key_bytes) = seed_group_key(&store, context_id);
        let (ciphertext, nonce) = encrypt_slice(&group_key_bytes, b"recorded");
        let sk = PrivateKey::from([0x8Du8; 32]);
        let author = sk.public_key();

        // Signed with the ORIGINAL (now stale) stamp...
        let stale_stamp = NOW;
        let payload = ephemeral_signature_payload(SignedEnvelope {
            context_id,
            author,
            seq: 1,
            key_id,
            sent_at_ms: stale_stamp,
            nonce,
            ciphertext: &ciphertext,
        })
        .expect("payload");
        let signature = sk.sign(&payload).expect("sign").to_bytes();

        let (node_client_for_ctx, _rx_ctx, _tmp_ctx) = node_client_with_rx(fresh_store()).await;
        let ctx_client = ContextClient::new(store, node_client_for_ctx, LazyRecipient::new());

        // ...but replayed with the wire field restamped to "now", which is
        // exactly what a replayer would try in order to clear the freshness
        // gate.
        let replay_at = NOW + 10 * crate::handlers::ephemeral::PRESENCE_MAX_SKEW_MS;
        let out = resolve_and_decrypt(
            &ctx_client,
            context_id,
            author,
            1,
            key_id,
            replay_at,
            replay_at,
            nonce,
            ciphertext,
            signature,
        )
        .await;

        assert!(
            out.is_none(),
            "restamping sent_at_ms must fail signature verification, not slip through"
        );
    }

    // -----------------------------------------------------------------------
    // Test 3: same bytes + higher seq → liveness extended, no Upsert
    //
    // Task-5 reviewer coverage request: made visible here through the
    // inbound path (the diff returned from `store.apply` drives the emit
    // decision). A re-publish with unchanged bytes but a higher sequence
    // number must NOT produce a `Diff::Upsert` — no spurious WebSocket push.
    // -----------------------------------------------------------------------

    #[test]
    fn same_bytes_higher_seq_extends_liveness_no_upsert() {
        let context_id = ContextId::from([0x04u8; 32]);
        let author = PublicKey::from([0x05u8; 32]);
        let slice = b"presence data";

        let mut store = AwarenessStore::new();

        // First apply: new entry → Upsert (would trigger an event).
        let diff1 = store.apply(context_id, author, 1, slice.to_vec(), 1_000);
        assert!(
            matches!(diff1.as_slice(), [Diff::Upsert { .. }]),
            "first apply must produce Diff::Upsert"
        );

        // Second apply: same bytes, higher seq → liveness refreshed, no diff.
        // The inbound handler emits one event per diff apply returns, so an
        // empty result means no event is emitted.
        let diff2 = store.apply(context_id, author, 2, slice.to_vec(), 2_000);
        assert!(
            diff2.is_empty(),
            "same-bytes re-apply with higher seq must yield no diffs (no WebSocket push)"
        );

        // Entry is still present with the refreshed liveness timestamp. Reading
        // the snapshot at the second apply's timestamp must report age 0 — that
        // is what proves the no-diff re-apply still refreshed `last_seen_ms`
        // (before ageing was reported, this test could only assert presence).
        let snapshot = store.snapshot(context_id, 2_000);
        assert_eq!(snapshot.len(), 1, "entry must still be present");
        assert_eq!(snapshot[0].1.as_slice(), slice.as_ref());
        assert_eq!(
            snapshot[0].2, 0,
            "same-bytes re-apply must refresh last_seen_ms even though it emits no diff"
        );

        // Sweep at a time that would have expired the seq=1 liveness (1_000 ms)
        // but NOT the seq=2 liveness (2_000 ms): ttl_ms=1_500, now_ms=3_000
        // → age since seq=2 last_seen = 1_000 < 1_500 → survives.
        let removals = store.sweep(context_id, 1_500, 3_000);
        assert!(
            removals.is_empty(),
            "entry must survive sweep because liveness was extended by the higher-seq re-apply"
        );
    }

    // -----------------------------------------------------------------------
    // Oversized ciphertext is dropped before decrypt (finding 2): a patched
    // peer pushing well past EPHEMERAL_MAX_BYTES must not be decrypted,
    // allocated, or pinned in the AwarenessStore, even though gossipsub's own
    // ceiling (1 MiB) would happily carry it.
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn oversized_ciphertext_is_dropped_before_decrypt() {
        let store = fresh_store();
        let context_id = ContextId::from([0x40u8; 32]);
        let author = PrivateKey::from([0x41u8; 32]).public_key();

        let (_group_id, key_id, _group_key_bytes) = seed_group_key(&store, context_id);

        // Oversized ciphertext — well past EPHEMERAL_MAX_BYTES plus AEAD
        // overhead. Not real ciphertext (decrypt would fail on it anyway) —
        // the point is that the size gate must reject it before any decrypt
        // is even attempted, so the signature/nonce below need not be valid.
        let oversized_ciphertext =
            vec![0xAAu8; crate::handlers::ephemeral::EPHEMERAL_MAX_BYTES * 64];
        let nonce = [0x11u8; calimero_crypto::NONCE_LEN];

        let (node_client_for_ctx, _rx_ctx, _tmp_ctx) = node_client_with_rx(fresh_store()).await;
        let ctx_client = ContextClient::new(store, node_client_for_ctx, LazyRecipient::new());

        let result = resolve_and_decrypt(
            &ctx_client,
            context_id,
            author,
            1,
            key_id,
            SENT_AT,
            NOW,
            nonce,
            oversized_ciphertext,
            [0u8; 64],
        )
        .await;

        assert!(
            result.is_none(),
            "oversized ciphertext must be dropped before decrypt"
        );
    }
}
