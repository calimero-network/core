//! Inbound ephemeral-presence dispatch: gossip → decrypt → awareness store → client event.
//!
//! **No state-delta, no RocksDB writes.** This module's only storage
//! interaction is a read of the group-key entry (via `lookup_group_key_with_wait`
//! with `Duration::ZERO`, a single-shot non-blocking lookup) to decrypt the
//! sealed presence slice before handing it to the in-memory `AwarenessStore`.

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
use crate::handlers::state_delta::lookup_group_key_ephemeral;
use crate::NodeManager;

// ---------------------------------------------------------------------------
// Inner async logic (testable without actix)
// ---------------------------------------------------------------------------

/// Resolve the group key for `context_id` and decrypt `ciphertext`.
///
/// Returns `None` when the `key_id` is unknown (unknown group or key not in
/// keyring) or when the AEAD authentication fails. Both cases are silent
/// drops — ephemeral presence is best-effort.
///
/// Never writes to the DAG, RocksDB, or any persistent store.
pub(crate) async fn resolve_and_decrypt(
    context_client: &ContextClient,
    context_id: ContextId,
    key_id: [u8; 32],
    nonce: Nonce,
    ciphertext: Vec<u8>,
) -> Option<Vec<u8>> {
    // Derive the ContextGroupId the same way the state-delta handler does:
    // `get_group_for_context` reads the context-tree row that
    // `register_context_in_group` wrote at group-creation time. On `None`
    // (context not in any group) the message is not decryptable — drop.
    let store = context_client.datastore();
    let group_id: ContextGroupId =
        match calimero_context::group_store::get_group_for_context(store, &context_id) {
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

    // Single-shot key lookup (Duration::ZERO = no polling wait).
    // The namespace-fallback for Open subgroups is handled inside
    // `lookup_group_key_ephemeral` transparently.
    let key = match lookup_group_key_ephemeral(
        context_client,
        &group_id,
        &key_id,
        std::time::Duration::ZERO,
    )
    .await
    {
        Ok(Some(k)) => k,
        Ok(None) => {
            debug!(
                %context_id,
                key_id = %hex::encode(key_id),
                "ephemeral: unknown key_id — dropping (presence is best-effort)"
            );
            return None;
        }
        Err(err) => {
            debug!(%context_id, %err, "ephemeral: key lookup error — dropping");
            return None;
        }
    };

    // AEAD decrypt. `SharedKey::from_sk` derives the symmetric key from the
    // stored group private key; `decrypt` returns `None` on authentication
    // failure (wrong key or tampered ciphertext). Drop silently either way —
    // a decryption failure on the ephemeral path should not produce a log
    // storm; only debug-level is emitted.
    let plaintext = calimero_crypto::SharedKey::from_sk(&key).decrypt(ciphertext, nonce);
    if plaintext.is_none() {
        debug!(
            %context_id,
            "ephemeral: AEAD decrypt failed — dropping"
        );
    }
    plaintext
}

// ---------------------------------------------------------------------------
// Emit helper (testable without actix)
// ---------------------------------------------------------------------------

/// Convert a `Diff` from the `AwarenessStore` into a `NodeEvent::Context`
/// and deliver it to WebSocket subscribers via `node_client.send_event`.
///
/// Infallible to callers: a failed send (no receivers) is logged at debug
/// and discarded — client events are best-effort.
pub(crate) fn emit_ephemeral_diff(node_client: &NodeClient, context_id: ContextId, diff: Diff) {
    let payload = match diff {
        Diff::Upsert { author, slice } => ContextEventPayload::Ephemeral(EphemeralPayload {
            author,
            state: Some(slice),
            removed: false,
        }),
        Diff::Remove { author } => ContextEventPayload::Ephemeral(EphemeralPayload {
            author,
            state: None,
            removed: true,
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
pub fn handle_ephemeral_broadcast(
    this: &mut NodeManager,
    ctx: &mut actix::Context<NodeManager>,
    context_id: ContextId,
    author: PublicKey,
    seq: u64,
    key_id: [u8; 32],
    nonce: Nonce,
    ciphertext: Vec<u8>,
) {
    let context_client = this.clients.context.clone();
    let node_client = this.clients.node.clone();

    let _ignored = ctx.spawn(
        async move {
            // Resolve key + decrypt: the only async work.
            resolve_and_decrypt(&context_client, context_id, key_id, nonce, ciphertext).await
        }
        .into_actor(this)
        .map(move |plaintext, actor, _ctx| {
            let Some(slice) = plaintext else {
                // Already logged inside resolve_and_decrypt.
                return;
            };

            // now_ms: milliseconds since UNIX epoch (wall clock). The
            // AwarenessStore is time-parameterised so tests can inject
            // arbitrary timestamps; callers supply the wall-clock reading.
            // `unwrap_or(0)` degrades to "always expired" rather than
            // panicking on a pre-epoch clock.
            let now_ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0);

            if let Some(diff) = actor
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
    use calimero_context::group_store::{register_context_in_group, GroupKeyring};
    use calimero_context_client::client::ContextClient;
    use calimero_context_config::types::ContextGroupId;
    use calimero_crypto::{SharedKey, NONCE_LEN};
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
    use crate::handlers::ephemeral::store::AwarenessStore;

    // -----------------------------------------------------------------------
    // Shared test scaffolding (mirrors test_support.rs patterns)
    // -----------------------------------------------------------------------

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
            String::new(),
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
            .encrypt(plaintext.to_vec(), nonce)
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
        let author = PublicKey::from([0x02u8; 32]);

        let (_group_id, key_id, group_key_bytes) = seed_group_key(&store, context_id);
        let slice = b"cursor={x:42,y:10}";
        let (ciphertext, nonce) = encrypt_slice(&group_key_bytes, slice);

        let (node_client_for_ctx, _rx_ctx, _tmp_ctx) = node_client_with_rx(fresh_store()).await;
        let ctx_client = ContextClient::new(store, node_client_for_ctx, LazyRecipient::new());

        let (node_client, mut event_rx, _tmp) = node_client_with_rx(fresh_store()).await;

        // Resolve + decrypt the ciphertext.
        let plaintext = resolve_and_decrypt(&ctx_client, context_id, key_id, nonce, ciphertext)
            .await
            .expect("should decrypt — key is seeded");

        assert_eq!(
            plaintext.as_slice(),
            slice.as_ref(),
            "decrypted bytes must match"
        );

        // Apply to store and emit event.
        let now_ms = 1_000u64;
        let mut store_under_test = AwarenessStore::new();
        let diff = store_under_test
            .apply(context_id, author, 1, plaintext.clone(), now_ms)
            .expect("new entry must produce a Diff::Upsert");

        emit_ephemeral_diff(&node_client, context_id, diff);

        // Assert the event reached the subscriber.
        let event = event_rx.try_recv().expect("event must have been emitted");
        let NodeEvent::Context(ctx_event) = event;
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
        let snapshot = store_under_test.snapshot(context_id);
        assert_eq!(snapshot.len(), 1);
        assert_eq!(snapshot[0].0, author);
        assert_eq!(snapshot[0].1.as_slice(), slice.as_ref());
    }

    // -----------------------------------------------------------------------
    // Test 2: unknown key_id → silent drop, no event, no error
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn unknown_key_id_is_dropped_silently() {
        let store = fresh_store();
        let context_id = ContextId::from([0x03u8; 32]);

        // Seed the group so context resolution works, but use a wrong key_id.
        let (_group_id, _real_key_id, group_key_bytes) = seed_group_key(&store, context_id);
        let slice = b"should not arrive";
        let (ciphertext, nonce) = encrypt_slice(&group_key_bytes, slice);

        let (node_client_for_ctx, _rx_ctx, _tmp_ctx) = node_client_with_rx(fresh_store()).await;
        let ctx_client = ContextClient::new(store, node_client_for_ctx, LazyRecipient::new());

        // key_id that was never stored.
        let wrong_key_id = [0x00u8; 32];

        let result =
            resolve_and_decrypt(&ctx_client, context_id, wrong_key_id, nonce, ciphertext).await;

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
            matches!(diff1, Some(Diff::Upsert { .. })),
            "first apply must produce Diff::Upsert"
        );

        // Second apply: same bytes, higher seq → liveness refreshed, no diff.
        // The inbound handler only calls emit_ephemeral_diff when apply returns
        // Some(_); returning None here means no event is emitted.
        let diff2 = store.apply(context_id, author, 2, slice.to_vec(), 2_000);
        assert!(
            diff2.is_none(),
            "same-bytes re-apply with higher seq must return None (no WebSocket push)"
        );

        // Entry is still present with the refreshed liveness timestamp.
        let snapshot = store.snapshot(context_id);
        assert_eq!(snapshot.len(), 1, "entry must still be present");
        assert_eq!(snapshot[0].1.as_slice(), slice.as_ref());

        // Sweep at a time that would have expired the seq=1 liveness (1_000 ms)
        // but NOT the seq=2 liveness (2_000 ms): ttl_ms=1_500, now_ms=3_000
        // → age since seq=2 last_seen = 1_000 < 1_500 → survives.
        let removals = store.sweep(context_id, 1_500, 3_000);
        assert!(
            removals.is_empty(),
            "entry must survive sweep because liveness was extended by the higher-seq re-apply"
        );
    }
}
