//! Routing guard for the ephemeral-presence inbound path.
//!
//! Unlike the helper-only unit tests in `handlers::ephemeral::inbound`
//! (which call `resolve_and_decrypt` / `emit_ephemeral_diff` directly),
//! this module drives a real `BroadcastMessage::Ephemeral` through the
//! **production `Handler<NetworkEvent>` match** in `handlers::network_event`
//! — the exact entrypoint a gossipsub message takes.
//!
//! The critical property: if the explicit `BroadcastMessage::Ephemeral =>
//! handle_ephemeral_broadcast(...)` arm is deleted or moved *below* the
//! `_ =>` wildcard, an `Ephemeral` message falls through to the
//! "unknown broadcast" debug arm, no decrypt / store-apply / emit runs, and
//! the awaited `ContextEventPayload::Ephemeral` never arrives — so this
//! test times out and FAILS. That is the guard the brief required and the
//! helper-only tests could not provide.

use std::time::Duration;

use calimero_context_config::types::ContextGroupId;
use calimero_crypto::{SharedKey, NONCE_LEN};
use calimero_governance_store::{register_context_in_group, GroupKeyring};
use calimero_network_primitives::messages::{IdentTopic, Message, MessageId, NetworkEvent};
use calimero_node_primitives::sync::BroadcastMessage;
use calimero_primitives::context::ContextId;
use calimero_primitives::events::{ContextEventPayload, NodeEvent};
use calimero_primitives::identity::{PrivateKey, PublicKey};
use futures_util::StreamExt;
use serial_test::serial;

use crate::test_node_harness::boot_test_node;

/// Milliseconds since the UNIX epoch — the same reading the outbound path
/// stamps and the inbound freshness gate compares against.
fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Borsh-encode a `BroadcastMessage::Ephemeral` and wrap it in a
/// `NetworkEvent::Message` on `topic`, exactly as the gossipsub layer would
/// hand it to the node actor.
fn ephemeral_network_event(
    source: libp2p::PeerId,
    topic: &str,
    context_id: ContextId,
    author: PublicKey,
    seq: u64,
    key_id: [u8; 32],
    sent_at_ms: u64,
    nonce: [u8; NONCE_LEN],
    ciphertext: Vec<u8>,
    signature: [u8; 64],
) -> NetworkEvent {
    let payload = BroadcastMessage::Ephemeral {
        context_id,
        author,
        seq,
        key_id,
        sent_at_ms,
        nonce,
        ciphertext: ciphertext.into(),
        signature,
    };
    let data = borsh::to_vec(&payload).expect("borsh encode Ephemeral");

    NetworkEvent::Message {
        id: MessageId(b"test-ephemeral".to_vec()),
        message: Message {
            source: Some(source),
            data,
            sequence_number: Some(1),
            topic: IdentTopic::new(topic.to_owned()).hash(),
        },
    }
}

/// A decryptable `BroadcastMessage::Ephemeral`, routed through the real
/// `Handler<NetworkEvent>` dispatch, must surface a decrypted
/// `ContextEventPayload::Ephemeral` on the node's event sink.
///
/// This exercises the explicit match arm — the routing guard. Deleting the
/// arm (so `Ephemeral` hits the `_ =>` wildcard) makes the awaited event
/// never arrive and this test times out.
#[tokio::test]
#[serial(boot_test_node)]
async fn ephemeral_broadcast_routes_to_awareness_store_and_emits_event() {
    let node = boot_test_node().await;

    let context_id = ContextId::from([0xE1u8; 32]);
    let author_sk = PrivateKey::from([0xE2u8; 32]);
    let author = author_sk.public_key();
    let seq = 1u64;
    // The receive path checks `sent_at_ms` against its own wall clock, so a
    // routing test has to stamp a real "now" — a fixed literal would age out
    // of the freshness window and turn this into a silent no-event failure.
    let sent_at_ms = now_ms();

    // Seed the group key into the SAME store the actor's context_client reads,
    // and register the context into the group so `get_group_for_context`
    // resolves the group id inside the inbound handler.
    let group_id = ContextGroupId::from([0xE3u8; 32]);
    register_context_in_group(&node.store, &group_id, &context_id)
        .expect("register_context_in_group");
    let group_key_bytes = [0x42u8; 32];
    let key_id = GroupKeyring::new(&node.store, group_id)
        .store_key(&group_key_bytes)
        .expect("store_key");

    // Encrypt a known slice under the seeded group key.
    let slice = b"cursor={x:7,y:3}";
    let nonce = [0x11u8; NONCE_LEN];
    let sk = PrivateKey::from(group_key_bytes);
    let ciphertext = SharedKey::from_sk(&sk)
        .encrypt_with_nonce(slice.to_vec(), nonce)
        .expect("encrypt");

    // Sign the envelope over the ciphertext that will actually go on the
    // wire, the same way the outbound path does — a literal signature would
    // fail verification now that the receive path checks it.
    let signature_payload = crate::handlers::ephemeral::auth::ephemeral_signature_payload(
        context_id,
        author,
        seq,
        key_id,
        sent_at_ms,
        &ciphertext,
    )
    .expect("signature payload");
    let signature = author_sk.sign(&signature_payload).expect("sign").to_bytes();

    // Subscribe to the node event sink BEFORE dispatching, so the emit from
    // the (async, ctx.spawn'd) handler is observed. `receive_events()`
    // subscribes eagerly at call time.
    let mut events = Box::pin(node.node_client.receive_events());

    // Deliver the Ephemeral broadcast through the production dispatch.
    let topic = format!("context/{}", hex::encode(context_id.as_ref()));
    let event = ephemeral_network_event(
        libp2p::PeerId::random(),
        &topic,
        context_id,
        author,
        seq,
        key_id,
        sent_at_ms,
        nonce,
        ciphertext,
        signature,
    );
    node.node_addr
        .send(event)
        .await
        .expect("deliver Ephemeral NetworkEvent to node actor");

    // Await the decrypted presence event on the sink. If the routing arm is
    // missing, nothing is emitted and this times out → the test fails.
    let received = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            match events.next().await {
                Some(NodeEvent::Context(ctx_event)) => {
                    if let ContextEventPayload::Ephemeral(payload) = ctx_event.payload {
                        assert_eq!(ctx_event.context_id, context_id);
                        break payload;
                    }
                    // Ignore any unrelated context events.
                }
                // Non-context events (e.g. GroupMembership) are not this
                // test's concern — keep waiting for the presence event.
                Some(_) => {}
                None => panic!("event stream closed before an Ephemeral event arrived"),
            }
        }
    })
    .await
    .expect(
        "expected a ContextEventPayload::Ephemeral on the sink within 5s — \
         the BroadcastMessage::Ephemeral match arm routed to handle_ephemeral_broadcast",
    );

    assert_eq!(received.author, author, "author must match the sender");
    assert_eq!(
        received.state.as_deref(),
        Some(slice.as_ref()),
        "decrypted slice must reach the client event"
    );
    assert!(!received.removed, "an upsert must not be marked removed");
}

/// A forged `author` driven through the production dispatch must produce no
/// presence event. Companion to the positive routing test above: that one
/// fails if the match arm is deleted, this one fails if the signature check
/// is — it is the negative twin, proving the rejection survives the real
/// `Handler<NetworkEvent>` dispatch, not just the unit-level
/// `resolve_and_decrypt` call in `handlers::ephemeral::inbound`.
#[tokio::test]
#[serial(boot_test_node)]
async fn forged_author_produces_no_presence_event() {
    let node = boot_test_node().await;

    let context_id = ContextId::from([0xF1u8; 32]);
    let group_id = ContextGroupId::from([0xF3u8; 32]);
    register_context_in_group(&node.store, &group_id, &context_id)
        .expect("register_context_in_group");
    let group_key_bytes = [0x42u8; 32];
    let key_id = GroupKeyring::new(&node.store, group_id)
        .store_key(&group_key_bytes)
        .expect("store_key");

    // Ciphertext decrypts cleanly under the current group key — the only
    // thing wrong with this message is the authorship claim.
    let slice = b"cursor={x:1,y:1}";
    let nonce = [0x11u8; NONCE_LEN];
    let sk = PrivateKey::from(group_key_bytes);
    let ciphertext = SharedKey::from_sk(&sk)
        .encrypt_with_nonce(slice.to_vec(), nonce)
        .expect("encrypt");

    // Attacker signs with their own key but claims the victim as author.
    let attacker = PrivateKey::from([0xF4u8; 32]);
    let victim = PrivateKey::from([0xF5u8; 32]).public_key();
    let seq = 1u64;
    let sent_at_ms = now_ms();
    let payload = crate::handlers::ephemeral::auth::ephemeral_signature_payload(
        context_id,
        victim,
        seq,
        key_id,
        sent_at_ms,
        &ciphertext,
    )
    .expect("payload");
    let signature = attacker.sign(&payload).expect("sign").to_bytes();

    let mut events = Box::pin(node.node_client.receive_events());

    let topic = format!("context/{}", hex::encode(context_id.as_ref()));
    let event = ephemeral_network_event(
        libp2p::PeerId::random(),
        &topic,
        context_id,
        victim,
        seq,
        key_id,
        sent_at_ms,
        nonce,
        ciphertext,
        signature,
    );
    node.node_addr
        .send(event)
        .await
        .expect("deliver Ephemeral NetworkEvent to node actor");

    // No presence event must arrive. A short window is enough: the positive
    // test above observes its event well within this budget.
    let got = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            match events.next().await {
                Some(NodeEvent::Context(ctx_event)) => {
                    if let ContextEventPayload::Ephemeral(payload) = ctx_event.payload {
                        break payload;
                    }
                }
                Some(_) => {}
                None => panic!("event stream closed"),
            }
        }
    })
    .await;

    assert!(
        got.is_err(),
        "a forged author must not produce a presence event, got {got:?}"
    );
}
