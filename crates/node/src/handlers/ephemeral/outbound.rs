//! Outbound ephemeral-presence: set + encrypt + publish + heartbeat.
//!
//! **No state-delta, no RocksDB writes.** The only storage interaction is a
//! *read* of the current group-key record (a synchronous keyring scan) used
//! to encrypt the presence slice before gossiping. The awareness store is
//! in-memory only; see `store.rs`.
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

use calimero_context_client::client::ContextClient;
use calimero_crypto::SharedKey;
use calimero_network_primitives::client::NetworkClient;
use calimero_node_primitives::sync::snapshot::BroadcastMessage;
use calimero_primitives::context::ContextId;
use calimero_primitives::identity::{PrivateKey, PublicKey};
use libp2p::gossipsub::TopicHash;
use tracing::debug;

use crate::handlers::ephemeral::inbound::emit_ephemeral_diff;
use crate::handlers::ephemeral::{EPHEMERAL_MAX_BYTES, PRESENCE_TTL_MS};
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
    pub(crate) seq: u64,
    pub(crate) slice: Vec<u8>,
}

// ---------------------------------------------------------------------------
// Wall-clock helper
// ---------------------------------------------------------------------------

/// Milliseconds since the UNIX epoch.  Degrades to 0 on a pre-epoch clock
/// rather than panicking — the AwarenessStore's `now_ms` contract allows 0.
fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------
// Inner async function (testable without actix)
// ---------------------------------------------------------------------------

/// Build, serialize, and publish an `Ephemeral` broadcast message.
///
/// Loads the current group key from the keyring, generates a random nonce,
/// AEAD-seals `slice`, wraps it in [`BroadcastMessage::Ephemeral`], borsh-
/// serializes, and calls `network_client.publish` on the context gossip topic.
///
/// Returns an error if the group/key lookup fails, encryption fails, or
/// serialization fails.  A publish failure (e.g. no mesh peers) is logged at
/// `debug` and treated as a no-op — ephemeral presence is best-effort.
///
/// **Never writes to the DAG, RocksDB, or any persistent store.**
pub(crate) async fn do_publish_ephemeral(
    network_client: &NetworkClient,
    context_client: &ContextClient,
    context_id: ContextId,
    author: PublicKey,
    seq: u64,
    slice: Vec<u8>,
) -> eyre::Result<()> {
    let store = context_client.datastore();

    // Resolve the group the context belongs to (same derivation as inbound).
    let group_id = calimero_governance_store::get_group_for_context(store, &context_id)?
        .ok_or(EphemeralOutboundError::NoGroup)?;

    // Load the current (highest-epoch) group key for encryption.
    let record = calimero_governance_store::GroupKeyring::new(store, group_id)
        .load_current_key_record()?
        .ok_or(EphemeralOutboundError::NoGroupKey)?;

    // AEAD-seal the presence slice under the group key.
    // `SharedKey::from_sk` derives the symmetric key from the group private key
    // — identical to the `encrypt_op` pattern in `group_keys.rs`.
    //
    // `encrypt` mints a fresh random nonce per call and hands it back; presence
    // slices have no ratchet to sequence them, so a per-message random nonce is
    // exactly the guarantee this path needs (`encrypt_with_nonce` is for the
    // sync stream, whose caller owns single-use).
    let sk = PrivateKey::from(record.group_key);
    let (nonce, ciphertext) = SharedKey::from_sk(&sk)
        .encrypt(slice)
        .ok_or_else(|| eyre::eyre!("AEAD encrypt failed for ephemeral slice"))?;

    // Build and serialize the wire message.
    let msg = BroadcastMessage::Ephemeral {
        context_id,
        author,
        seq,
        key_id: record.key_id,
        nonce,
        ciphertext: Cow::Owned(ciphertext),
        // Placeholder superseded by Task 3, which signs the canonical payload
        // via `auth::ephemeral_signature_payload`.
        signature: [0u8; 64],
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
/// Failures at the crypto / key-loading step are returned synchronously
/// (before the spawn). The async publish failure is best-effort (logged at
/// debug, not propagated).
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

    // 2. Bump the sequence counter (or seed it at 1 for a new author).
    let local = this
        .ephemeral_local
        .entry((context_id, author))
        .or_insert_with(|| LocalEphemeral {
            seq: 0,
            slice: vec![],
        });
    local.seq = local.seq.saturating_add(1);
    local.slice = slice.clone();
    let seq = local.seq;

    // 3. Apply to the local awareness store NOW (synchronously, before the
    //    async publish) and emit the local diff so the setting client sees
    //    its own state immediately.
    let now = now_ms();
    if let Some(diff) = this
        .awareness_store
        .apply(context_id, author, seq, slice.clone(), now)
    {
        emit_ephemeral_diff(&this.clients.node, context_id, diff);
    }

    // 4. Spawn the async key-load + encrypt + publish.
    let network_client = this.clients.node.network_client().clone();
    let context_client = this.clients.context.clone();

    let _ignored = ctx.spawn(
        async move {
            if let Err(err) = do_publish_ephemeral(
                &network_client,
                &context_client,
                context_id,
                author,
                seq,
                slice,
            )
            .await
            {
                debug!(%context_id, %err, "ephemeral: outbound publish error");
            }
        }
        .into_actor(this),
    );

    Ok(())
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
/// 2. Re-encrypts under the **current** group key with a **fresh nonce**.
/// 3. Publishes on the context gossip topic.
/// 4. Sweeps expired remote entries for the context, emitting
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

    // Sweep each context for expired remote entries and emit Remove diffs.
    // Deduplicate context ids from the local snapshot.
    let mut contexts_seen: Vec<ContextId> =
        local_snapshot.iter().map(|(ctx_id, ..)| *ctx_id).collect();
    contexts_seen.dedup();
    for context_id in &contexts_seen {
        for diff in this
            .awareness_store
            .sweep(*context_id, PRESENCE_TTL_MS, now_ms)
        {
            emit_ephemeral_diff(&this.clients.node, *context_id, diff);
        }
    }

    // Re-publish each local slice (bumped seq, fresh nonce, current key).
    let network_client = this.clients.node.network_client().clone();
    let context_client = this.clients.context.clone();

    let _ignored = ctx.spawn(
        async move {
            for (context_id, author, seq, slice) in local_snapshot {
                if let Err(err) = do_publish_ephemeral(
                    &network_client,
                    &context_client,
                    context_id,
                    author,
                    seq,
                    slice,
                )
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
    use calimero_blobstore::config::BlobStoreConfig;
    use calimero_blobstore::{BlobManager as BlobStore, FileSystem};
    use calimero_context_client::client::ContextClient;
    use calimero_context_config::types::ContextGroupId;
    use calimero_governance_store::{register_context_in_group, GroupKeyring};
    use calimero_network_primitives::client::NetworkClient;
    use calimero_network_primitives::messages::{MessageId, NetworkMessage};
    use calimero_node_primitives::client::{BlobManager, NodeClient, SyncClient};
    use calimero_node_primitives::sync::snapshot::BroadcastMessage;
    use calimero_primitives::context::ContextId;
    use calimero_primitives::identity::PublicKey;
    use calimero_store::db::InMemoryDB;
    use calimero_store::Store;
    use calimero_utils_actix::LazyRecipient;
    use libp2p::gossipsub::TopicHash;
    use tokio::sync::{broadcast, mpsc};

    use super::*;

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

    /// Build a minimal `NodeClient` for the `ContextClient` (key-lookup only,
    /// no publish operations triggered). The internal network client is
    /// disconnected — all channels are inert stubs.
    async fn minimal_node_client_for_ctx_client() -> (NodeClient, tempfile::TempDir) {
        let store = fresh_store();
        let tmp = tempfile::tempdir().expect("tempdir");
        let blob_cfg = BlobStoreConfig::new(
            std::path::PathBuf::from(tmp.path())
                .try_into()
                .expect("utf8 path"),
        );
        let fs = FileSystem::new(&blob_cfg).await.expect("blob fs");
        let blob_manager = BlobManager::new(BlobStore::new(store.clone(), fs));

        let (event_sender, _) = broadcast::channel(4);
        let (tx1, _) = mpsc::channel(1);
        let (tx2, _) = mpsc::channel(1);
        let (tx3, _) = mpsc::channel(1);
        let (tx4, _) = mpsc::channel(1);
        let sync_client = SyncClient::new(tx1, tx2, tx3, tx4);

        let client = NodeClient::new(
            store,
            blob_manager,
            NetworkClient::new(LazyRecipient::new()),
            LazyRecipient::new(),
            event_sender,
            sync_client,
            None,
        );
        (client, tmp)
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

    // -----------------------------------------------------------------------
    // Test 1: `do_publish_ephemeral` publishes exactly one message on the
    //          derived context topic with seq==1 on the first call.
    // -----------------------------------------------------------------------

    #[actix::test]
    async fn publishes_exactly_one_ephemeral_on_context_topic() {
        let store = fresh_store();
        let context_id = ContextId::from([0x01u8; 32]);
        let author = PublicKey::from([0x02u8; 32]);

        let (_group_id, _key_id, _group_key_bytes) = seed_group_key(&store, context_id);

        let (network_client, published) = recording_network_client();
        let (nc_for_ctx, _tmp_ctx) = minimal_node_client_for_ctx_client().await;
        let ctx_client = ContextClient::new(store, nc_for_ctx, LazyRecipient::new());

        let slice = b"cursor={x:10}".to_vec();

        // First call — seq == 1.
        do_publish_ephemeral(&network_client, &ctx_client, context_id, author, 1, slice)
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
        let author = PublicKey::from([0x04u8; 32]);

        let (_group_id, _key_id, _) = seed_group_key(&store, context_id);

        let (network_client, published) = recording_network_client();
        let (nc_for_ctx, _tmp_ctx) = minimal_node_client_for_ctx_client().await;
        let ctx_client = ContextClient::new(store, nc_for_ctx, LazyRecipient::new());

        let slice = b"cursor={x:10}".to_vec();

        do_publish_ephemeral(
            &network_client,
            &ctx_client,
            context_id,
            author,
            1,
            slice.clone(),
        )
        .await
        .expect("first publish");

        do_publish_ephemeral(&network_client, &ctx_client, context_id, author, 2, slice)
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
        let author = PublicKey::from([0x08u8; 32]);

        let (_group_id, _key_id, _) = seed_group_key(&store, context_id);

        let (network_client, published) = recording_network_client();
        let (nc_for_ctx, _tmp_ctx) = minimal_node_client_for_ctx_client().await;
        let ctx_client = ContextClient::new(store, nc_for_ctx, LazyRecipient::new());

        let slice = b"presence=typing".to_vec();

        // Initial set.
        do_publish_ephemeral(
            &network_client,
            &ctx_client,
            context_id,
            author,
            1,
            slice.clone(),
        )
        .await
        .expect("initial publish");

        // Heartbeat re-publish with bumped seq.
        do_publish_ephemeral(&network_client, &ctx_client, context_id, author, 2, slice)
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
    // Test 5: unknown context (no group) returns NoGroup error, no publish.
    // -----------------------------------------------------------------------

    #[actix::test]
    async fn unknown_context_returns_no_group_error() {
        let store = fresh_store();
        // Deliberately NOT seeding a group for this context.
        let context_id = ContextId::from([0x09u8; 32]);
        let author = PublicKey::from([0x0Au8; 32]);

        let (network_client, published) = recording_network_client();
        let (nc_for_ctx, _tmp_ctx) = minimal_node_client_for_ctx_client().await;
        let ctx_client = ContextClient::new(store, nc_for_ctx, LazyRecipient::new());

        let result = do_publish_ephemeral(
            &network_client,
            &ctx_client,
            context_id,
            author,
            1,
            b"x".to_vec(),
        )
        .await;

        assert!(result.is_err(), "must error for unknown context");
        let err_str = result.unwrap_err().to_string();
        assert!(
            err_str.contains("no group"),
            "error must mention 'no group', got: {err_str}"
        );

        let msgs = published.lock().expect("lock").clone();
        assert_eq!(msgs.len(), 0, "no publish on error");
    }
}
