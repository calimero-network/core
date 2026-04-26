use calimero_primitives::identity::PrivateKey;

use super::*;

fn dummy_ack(op_hash: [u8; 32]) -> SignedAck {
    SignedAck {
        op_hash,
        signer_pubkey: PrivateKey::random(&mut rand::thread_rng()).public_key(),
        signature: [0u8; 64],
    }
}

#[tokio::test]
async fn ack_router_subscribe_then_route_delivers() {
    let router = AckRouter::default();
    let mut rx = router.subscribe([1u8; 32]);
    let routed = router.route(dummy_ack([1u8; 32]));
    assert!(routed);
    let got = rx.recv().await.expect("ack received");
    assert_eq!(got.op_hash, [1u8; 32]);
}

#[tokio::test]
async fn ack_router_route_with_no_subscriber_returns_false() {
    let router = AckRouter::default();
    let routed = router.route(dummy_ack([2u8; 32]));
    assert!(!routed);
}

#[tokio::test]
async fn ack_router_release_drops_empty_entry() {
    let router = AckRouter::default();
    let rx = router.subscribe([3u8; 32]);
    router.release([3u8; 32], rx);
    assert!(router.inner.lock().unwrap().get(&[3u8; 32]).is_none());
}

#[tokio::test]
async fn ack_router_release_keeps_entry_when_other_receivers_alive() {
    // A second concurrent publish for the same op_hash must keep its
    // subscription alive after the first one releases.
    let router = AckRouter::default();
    let rx_a = router.subscribe([4u8; 32]);
    let _rx_b = router.subscribe([4u8; 32]);
    router.release([4u8; 32], rx_a);
    assert!(
        router.inner.lock().unwrap().get(&[4u8; 32]).is_some(),
        "entry must survive while another receiver is alive"
    );
}

#[tokio::test]
async fn ack_router_release_does_not_leak_when_caller_holds_rx() {
    // Regression: previously a `release(op_hash)` that did not consume
    // `rx` checked `receiver_count() == 0` while the caller's `rx` was
    // still on the stack, leaking one map entry per publish. The
    // current signature consumes `rx`, eliminating the leak.
    let router = AckRouter::default();
    for i in 0..16u8 {
        let key = [i; 32];
        let rx = router.subscribe(key);
        router.release(key, rx);
    }
    assert!(
        router.inner.lock().unwrap().is_empty(),
        "release must reap every entry; previously this map would have grown to 16"
    );
}
