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
//! **Security — authorship and time are signed, inside the seal.** The
//! ciphertext is one [`PresenceUpdate`], whose statement (context, author,
//! seq, time, state hash) its author signed. [`open_update`] decrypts it,
//! checks that signature and the freshness of the signed stamp, and, for an
//! account's update, the device certificate, membership and revocation by
//! this node's own governance data. Any failure is a silent drop, the same as
//! an unknown `key_id` or a failed AEAD decrypt. See [`EphemeralPayload`] for
//! the client-facing note.
//!
//! [`PresenceUpdate`]: calimero_node_primitives::presence::PresenceUpdate
//! [`EphemeralPayload`]: calimero_primitives::events::EphemeralPayload

use actix::{ActorFutureExt, AsyncContext, WrapFuture};
use calimero_context_client::client::ContextClient;
use calimero_context_config::types::ContextGroupId;
use calimero_crypto::Nonce;
use calimero_node_primitives::client::NodeClient;
use calimero_node_primitives::presence::PresenceUpdate;
use calimero_primitives::context::ContextId;
use calimero_primitives::events::{ContextEvent, ContextEventPayload, EphemeralPayload, NodeEvent};
use calimero_primitives::identity::{AccountId, PublicKey};
use tracing::debug;

use crate::handlers::ephemeral::store::Diff;
use crate::handlers::ephemeral::EPHEMERAL_MAX_BYTES;
use crate::NodeManager;

/// Room for everything around the slice in a sealed [`PresenceUpdate`]: the
/// statement, its signature, and an account's certificate chain. A
/// certificate with no root rollover is about 240 bytes; this leaves room
/// for a chain of rollovers.
const PRESENCE_UPDATE_OVERHEAD_BYTES: usize = 4_096;

/// Maximum on-wire ciphertext length accepted on the receive path:
/// [`EPHEMERAL_MAX_BYTES`] of slice, the update around it, and the AEAD tag.
/// Checked before any clone or decrypt so an oversized envelope (even one
/// that would still fit under gossipsub's own 1 MiB ceiling) is dropped
/// without paying for the allocation or pinning it in the `AwarenessStore`.
pub(crate) const EPHEMERAL_MAX_CIPHERTEXT_BYTES: usize =
    EPHEMERAL_MAX_BYTES + PRESENCE_UPDATE_OVERHEAD_BYTES + calimero_crypto::AEAD_TAG_LEN;

// ---------------------------------------------------------------------------
// The wire envelope
// ---------------------------------------------------------------------------

/// One inbound presence envelope, exactly as it rides on the wire: the owned
/// counterpart of [`BroadcastMessage::Ephemeral`].
///
/// [`BroadcastMessage::Ephemeral`]: calimero_node_primitives::sync::BroadcastMessage::Ephemeral
#[derive(Debug)]
pub(crate) struct EphemeralEnvelope {
    pub context_id: ContextId,
    /// The group key the update was sealed under. Only the group's *current*
    /// key is accepted on this path.
    pub key_id: [u8; 32],
    pub nonce: Nonce,
    /// The AEAD-sealed borsh of a [`PresenceUpdate`].
    pub ciphertext: Vec<u8>,
}

/// An update [`open_update`] accepted.
#[derive(Debug)]
pub(crate) struct Accepted {
    pub author: PublicKey,
    /// The account a verified certificate named; `None` for a node's own.
    pub account: Option<AccountId>,
    pub seq: u64,
    /// The slice, or `None` to retract.
    pub state: Option<Vec<u8>>,
}

// ---------------------------------------------------------------------------
// Inner async logic (testable without actix)
// ---------------------------------------------------------------------------

/// Resolve the group key for the envelope's context, decrypt it, and verify the
/// update inside.
///
/// Returns `None` when the context has no group, when `key_id` is not the
/// group's *current* key (unknown and superseded keys are treated alike —
/// presence has no history, so only the current key is accepted), when the
/// ciphertext is oversized or fails AEAD authentication, when the update does
/// not decode or verify, when its signed stamp is outside the freshness window,
/// or when an account's update comes from a non-member or a revoked device.
/// All are silent drops — ephemeral presence is best-effort.
///
/// `now_ms` is passed in so the gate is deterministic under test.
///
/// Never writes to the DAG, RocksDB, or any persistent store.
pub(crate) async fn open_update(
    context_client: &ContextClient,
    envelope: EphemeralEnvelope,
    now_ms: u64,
) -> Option<Accepted> {
    let EphemeralEnvelope {
        context_id,
        key_id,
        nonce,
        ciphertext,
    } = envelope;

    // The context-tree row `register_context_in_group` wrote: on `None` the
    // message is not decryptable.
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

    // The keyring that seals the context's state deltas: the namespace's for an
    // Open chain, otherwise the group's own.
    let key_group_id = match calimero_governance_store::key_covering_group(store, &group_id) {
        Ok(key_group_id) => key_group_id,
        Err(err) => {
            debug!(%context_id, %err, "ephemeral: covering keyring lookup error - dropping");
            return None;
        }
    };

    // Only the CURRENT key: the keyring keeps superseded keys for state-delta
    // decryption, and accepting them here would let a rotated-out member keep
    // publishing presence straight through a rotation.
    let record = match calimero_governance_store::GroupKeyring::new(store, key_group_id)
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
        let keyring = calimero_governance_store::GroupKeyring::new(store, key_group_id);
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

    // Before any clone or decrypt: a patched peer can put anything up to
    // gossipsub's 1 MiB ceiling on the wire.
    if ciphertext.len() > EPHEMERAL_MAX_CIPHERTEXT_BYTES {
        debug!(
            %context_id,
            len = ciphertext.len(),
            max = EPHEMERAL_MAX_CIPHERTEXT_BYTES,
            "ephemeral: ciphertext exceeds size cap — dropping"
        );
        return None;
    }

    let key = calimero_primitives::identity::PrivateKey::from(record.group_key);
    let Some(plaintext) = calimero_crypto::SharedKey::from_sk(&key).decrypt(ciphertext, nonce)
    else {
        debug!(%context_id, "ephemeral: AEAD decrypt failed — dropping");
        return None;
    };

    let update: PresenceUpdate = match borsh::from_slice(&plaintext) {
        Ok(update) => update,
        Err(err) => {
            debug!(%context_id, %err, "ephemeral: undecodable update — dropping");
            return None;
        }
    };
    let verified = match update.verify(context_id) {
        Ok(verified) => verified,
        Err(err) => {
            debug!(%context_id, %err, "ephemeral: update refused — dropping");
            return None;
        }
    };

    // The stamp is inside the seal and the signature, so a mesh peer replaying
    // recorded bytes cannot restamp it to look fresh.
    if !crate::handlers::ephemeral::auth::is_fresh(now_ms, verified.sent_at_ms) {
        debug!(
            %context_id,
            author = %verified.author,
            sent_at_ms = verified.sent_at_ms,
            now_ms,
            max_skew_ms = crate::handlers::ephemeral::PRESENCE_MAX_SKEW_MS,
            "ephemeral: sent_at_ms outside the freshness window — dropping (replay or clock skew)"
        );
        return None;
    }

    // An account's update: holding the key is not enough. The account must be
    // a member here, and the device not revoked, by this node's own data.
    if let (Some(account), Some(device)) = (verified.account, verified.device) {
        match crate::handlers::ephemeral::standing::account_standing(
            store,
            &context_id,
            account,
            device,
        ) {
            Ok(crate::handlers::ephemeral::standing::Standing::Member) => {}
            other => {
                debug!(%context_id, %account, ?other, "ephemeral: account may not publish here — dropping");
                return None;
            }
        }
    }

    if update
        .state
        .as_ref()
        .is_some_and(|slice| slice.len() > EPHEMERAL_MAX_BYTES)
    {
        debug!(%context_id, "ephemeral: slice exceeds size cap — dropping");
        return None;
    }

    Some(Accepted {
        author: verified.author,
        account: verified.account,
        seq: verified.seq,
        state: update.state,
    })
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
        Diff::Upsert {
            author,
            account,
            slice,
        } => ContextEventPayload::Ephemeral(EphemeralPayload {
            author,
            account,
            state: Some(slice),
            removed: false,
            age_ms: None,
        }),
        Diff::Remove { author } => ContextEventPayload::Ephemeral(EphemeralPayload {
            author,
            account: None,
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
/// Wires the async key-resolution / decrypt / verify path ([`open_update`])
/// onto the actor's Arbiter via `ctx.spawn`, then in the synchronous
/// `.map()` callback applies the decrypted slice to the `AwarenessStore`
/// and emits any resulting `Diff` as a `NodeEvent::Context(Ephemeral)` on
/// the node's event broadcast sink.
///
/// **Never touches state-delta, RocksDB, or the DAG.**
pub(crate) fn handle_ephemeral_broadcast(
    this: &mut NodeManager,
    ctx: &mut actix::Context<NodeManager>,
    envelope: EphemeralEnvelope,
) {
    let context_client = this.clients.context.clone();
    let node_client = this.clients.node.clone();

    // Read once and used for BOTH the freshness gate and the awareness-store
    // stamp, so an update accepted as fresh is recorded against the same
    // reading it was judged by.
    let now_ms = crate::handlers::ephemeral::now_ms();
    let context_id = envelope.context_id;

    let _ignored = ctx.spawn(
        async move { open_update(&context_client, envelope, now_ms).await }
            .into_actor(this)
            .map(move |accepted, actor, _ctx| {
                // Already logged inside `open_update`.
                let Some(accepted) = accepted else {
                    return;
                };
                let diffs = match accepted.state {
                    // May yield two diffs: admitting a new author into a full
                    // context evicts the stalest one.
                    Some(slice) => actor.awareness_store.apply(
                        context_id,
                        accepted.author,
                        accepted.account,
                        accepted.seq,
                        slice,
                        now_ms,
                    ),
                    None => actor
                        .awareness_store
                        .retract(context_id, accepted.author, accepted.seq)
                        .into_iter()
                        .collect(),
                };
                for diff in diffs {
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
    use calimero_crypto::SharedKey;
    use calimero_governance_store::test_fixtures::{account_for, real_join_account};
    use calimero_governance_store::{
        register_context_in_group, GroupKeyring, MembershipRepository,
    };
    use calimero_network_primitives::client::NetworkClient;
    use calimero_node_primitives::client::{BlobManager, NodeClient, SyncClient};
    use calimero_node_primitives::presence::PresenceUpdate;
    use calimero_primitives::context::{ContextId, GroupMemberRole};
    use calimero_primitives::events::{ContextEventPayload, NodeEvent};
    use calimero_primitives::identity::PrivateKey;
    use calimero_store::db::InMemoryDB;
    use calimero_store::Store;
    use calimero_utils_actix::LazyRecipient;
    use tokio::sync::{broadcast, mpsc};

    use super::*;
    use crate::handlers::ephemeral::store::AwarenessStore;

    /// Fixed wall clock. Updates are stamped `SENT_AT == NOW`, so a test about
    /// some other gate cannot pass or fail because of freshness.
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
        let (relay_sealed_join_tx, _relay_sealed_join_rx) = mpsc::channel(1);
        let sync_client = SyncClient::new(
            ctx_sync_tx,
            ns_sync_tx,
            ns_join_tx,
            open_subgroup_join_tx,
            relay_sealed_join_tx,
        );

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

    /// Seal `update` under `group_key` the way `publish_sealed` does.
    fn sealed(
        context_id: ContextId,
        key_id: [u8; 32],
        group_key: [u8; 32],
        update: &PresenceUpdate,
    ) -> EphemeralEnvelope {
        let (nonce, ciphertext) = SharedKey::from_sk(&PrivateKey::from(group_key))
            .encrypt(borsh::to_vec(update).expect("borsh"))
            .expect("encrypt");
        EphemeralEnvelope {
            context_id,
            key_id,
            nonce,
            ciphertext,
        }
    }

    /// A node's own update (no certificate), signed by `sk`.
    fn node_update(
        sk: &PrivateKey,
        context_id: ContextId,
        seq: u64,
        sent_at: u64,
    ) -> PresenceUpdate {
        PresenceUpdate::signed(sk, context_id, seq, sent_at, Some(b"cursor".to_vec()), None)
            .expect("sign")
    }

    async fn context_client(store: Store) -> (ContextClient, tempfile::TempDir) {
        let (node_client, _rx, tmp) = node_client_with_rx(fresh_store()).await;
        (
            ContextClient::new(store, node_client, LazyRecipient::new()),
            tmp,
        )
    }

    #[tokio::test]
    async fn a_node_update_opens_with_no_account() {
        let store = fresh_store();
        let context_id = ContextId::from([0x01u8; 32]);
        let sk = PrivateKey::from([0x02u8; 32]);
        let (_group, key_id, group_key) = seed_group_key(&store, context_id);
        let envelope = sealed(
            context_id,
            key_id,
            group_key,
            &node_update(&sk, context_id, 1, SENT_AT),
        );
        let (ctx_client, _tmp) = context_client(store).await;

        let accepted = open_update(&ctx_client, envelope, NOW)
            .await
            .expect("opens");
        assert_eq!(accepted.author, sk.public_key());
        assert_eq!(accepted.account, None);
        assert_eq!(accepted.seq, 1);
        assert_eq!(accepted.state.as_deref(), Some(b"cursor".as_ref()));
    }

    #[tokio::test]
    async fn an_accepted_update_emits_an_event_with_its_account() {
        let (node_client, mut event_rx, _tmp) = node_client_with_rx(fresh_store()).await;
        let context_id = ContextId::from([0x03u8; 32]);
        let author = PrivateKey::from([0x04u8; 32]).public_key();
        let account = account_for(&author);
        let mut store = AwarenessStore::new();
        for diff in store.apply(context_id, author, Some(account), 1, b"x".to_vec(), 1_000) {
            emit_ephemeral_diff(&node_client, context_id, diff);
        }
        let NodeEvent::Context(event) = event_rx.try_recv().expect("event") else {
            panic!("expected a context event");
        };
        let ContextEventPayload::Ephemeral(payload) = event.payload else {
            panic!("expected an ephemeral payload");
        };
        assert_eq!(payload.author, author);
        assert_eq!(payload.account, Some(account));
        assert_eq!(payload.state.as_deref(), Some(b"x".as_ref()));
    }

    #[tokio::test]
    async fn an_update_for_another_context_is_dropped() {
        let store = fresh_store();
        let context_id = ContextId::from([0x05u8; 32]);
        let other = ContextId::from([0x06u8; 32]);
        let sk = PrivateKey::from([0x07u8; 32]);
        let (_group, key_id, group_key) = seed_group_key(&store, context_id);
        // Signed for `other`, sealed into an envelope for `context_id`.
        let envelope = sealed(
            context_id,
            key_id,
            group_key,
            &node_update(&sk, other, 1, SENT_AT),
        );
        let (ctx_client, _tmp) = context_client(store).await;
        assert!(open_update(&ctx_client, envelope, NOW).await.is_none());
    }

    #[tokio::test]
    async fn a_tampered_statement_is_dropped() {
        let store = fresh_store();
        let context_id = ContextId::from([0x08u8; 32]);
        let sk = PrivateKey::from([0x09u8; 32]);
        let (_group, key_id, group_key) = seed_group_key(&store, context_id);
        let mut update = node_update(&sk, context_id, 1, SENT_AT);
        // Claim another author under the same signature: the impersonation case.
        update.statement.author = PrivateKey::from([0x0Au8; 32]).public_key();
        let envelope = sealed(context_id, key_id, group_key, &update);
        let (ctx_client, _tmp) = context_client(store).await;
        assert!(open_update(&ctx_client, envelope, NOW).await.is_none());
    }

    #[tokio::test]
    async fn an_unknown_or_superseded_key_is_dropped() {
        let store = fresh_store();
        let context_id = ContextId::from([0x91u8; 32]);
        let group_id = ContextGroupId::from([0x92u8; 32]);
        register_context_in_group(&store, &group_id, &context_id).expect("register");
        let old_key = [0x93u8; 32];
        let old_key_id = GroupKeyring::new(&store, group_id)
            .store_key(&old_key)
            .expect("store old key");
        let _new = GroupKeyring::new(&store, group_id)
            .store_key_with_epoch(&[0x94u8; 32], 5)
            .expect("store new key");
        let sk = PrivateKey::from([0x95u8; 32]);
        let update = node_update(&sk, context_id, 1, SENT_AT);
        let (ctx_client, _tmp) = context_client(store).await;
        assert!(
            open_update(
                &ctx_client,
                sealed(context_id, old_key_id, old_key, &update),
                NOW
            )
            .await
            .is_none(),
            "a superseded key must be refused on the presence path"
        );
        assert!(
            open_update(
                &ctx_client,
                sealed(context_id, [0xEE; 32], old_key, &update),
                NOW
            )
            .await
            .is_none(),
            "an unknown key id is refused"
        );
    }

    #[tokio::test]
    async fn freshness_is_judged_on_the_signed_stamp() {
        let store = fresh_store();
        let context_id = ContextId::from([0x10u8; 32]);
        let sk = PrivateKey::from([0x11u8; 32]);
        let (_group, key_id, group_key) = seed_group_key(&store, context_id);
        let (ctx_client, _tmp) = context_client(store).await;
        let skew = crate::handlers::ephemeral::PRESENCE_MAX_SKEW_MS;
        for (sent_at, fresh) in [
            (NOW - skew, true),
            (NOW + skew, true),
            (NOW - skew - 1, false),
            (NOW + skew + 1, false),
        ] {
            let envelope = sealed(
                context_id,
                key_id,
                group_key,
                &node_update(&sk, context_id, 1, sent_at),
            );
            assert_eq!(
                open_update(&ctx_client, envelope, NOW).await.is_some(),
                fresh,
                "sent_at {sent_at} against now {NOW}"
            );
        }
    }

    #[tokio::test]
    async fn an_account_update_from_a_member_names_the_account() {
        let store = fresh_store();
        let context_id = ContextId::from([0x20u8; 32]);
        let (group, key_id, group_key) = seed_group_key(&store, context_id);
        let device = PrivateKey::from([0x21u8; 32]);
        let account = account_for(&device.public_key());
        MembershipRepository::new(&store)
            .add_member(&group, &account, GroupMemberRole::Member)
            .expect("seat");
        let update = PresenceUpdate::signed(
            &device,
            context_id,
            1,
            SENT_AT,
            Some(b"typing".to_vec()),
            Some(*real_join_account(&device.public_key())),
        )
        .expect("sign");
        let (ctx_client, _tmp) = context_client(store).await;
        let accepted = open_update(
            &ctx_client,
            sealed(context_id, key_id, group_key, &update),
            NOW,
        )
        .await
        .expect("a member's update opens");
        assert_eq!(accepted.account, Some(account));
    }

    #[tokio::test]
    async fn an_account_update_from_a_non_member_is_dropped() {
        let store = fresh_store();
        let context_id = ContextId::from([0x30u8; 32]);
        let (_group, key_id, group_key) = seed_group_key(&store, context_id);
        let device = PrivateKey::from([0x31u8; 32]);
        let update = PresenceUpdate::signed(
            &device,
            context_id,
            1,
            SENT_AT,
            Some(b"typing".to_vec()),
            Some(*real_join_account(&device.public_key())),
        )
        .expect("sign");
        let (ctx_client, _tmp) = context_client(store).await;
        assert!(
            open_update(
                &ctx_client,
                sealed(context_id, key_id, group_key, &update),
                NOW
            )
            .await
            .is_none(),
            "holding the key is not enough for an account: it must be a member"
        );
    }

    #[tokio::test]
    async fn an_oversized_ciphertext_is_dropped_before_decrypt() {
        let store = fresh_store();
        let context_id = ContextId::from([0x40u8; 32]);
        let (_group, key_id, _group_key) = seed_group_key(&store, context_id);
        let (ctx_client, _tmp) = context_client(store).await;
        let envelope = EphemeralEnvelope {
            context_id,
            key_id,
            nonce: [0x11u8; calimero_crypto::NONCE_LEN],
            ciphertext: vec![0xAAu8; EPHEMERAL_MAX_CIPHERTEXT_BYTES + 1],
        };
        assert!(open_update(&ctx_client, envelope, NOW).await.is_none());
    }

    #[test]
    fn same_bytes_higher_seq_extends_liveness_no_upsert() {
        let context_id = ContextId::from([0x04u8; 32]);
        let author = PrivateKey::from([0x05u8; 32]).public_key();
        let mut store = AwarenessStore::new();
        assert!(matches!(
            store
                .apply(context_id, author, None, 1, b"p".to_vec(), 1_000)
                .as_slice(),
            [Diff::Upsert { .. }]
        ));
        assert!(store
            .apply(context_id, author, None, 2, b"p".to_vec(), 2_000)
            .is_empty());
        assert_eq!(
            store.snapshot(context_id, 2_000)[0].3,
            0,
            "liveness refreshed"
        );
        assert!(store.sweep(context_id, 1_500, 3_000).is_empty());
    }
}
