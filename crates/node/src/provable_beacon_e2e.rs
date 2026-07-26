//! Node-level receive-path tests for the provable non-member beacon.
//!
//! These boot a real `NodeManager` over the shared in-process harness and hand
//! it a synthesized gossipsub `NetworkEvent`, so they cover the wiring the pure
//! unit tests in `handlers::network_event::readiness` cannot reach: which peer
//! the unlocked pull targets, that a stale beacon unlocks nothing, and that a
//! beacon accepted on this path never enters the readiness cache.
use std::time::Duration;

use calimero_context::group_store::{now_millis, MembershipRepository};
use calimero_context_client::local_governance::{NamespaceTopicMsg, SignedReadinessBeacon};
use calimero_context_config::types::{
    ContextGroupId, GroupInvitationFromAdmin, SignedGroupOpenInvitation, SignerId,
};
use calimero_network_primitives::messages::{IdentTopic, Message, MessageId, NetworkEvent};
use calimero_node_primitives::sync::BroadcastMessage;
use calimero_primitives::context::GroupMemberRole;
use calimero_primitives::identity::PrivateKey;
use calimero_store::Store;
use libp2p::PeerId;
use sha2::{Digest, Sha256};
use tokio::time::sleep;

use crate::test_node_harness::{boot_test_node, TestNode};

const NS: [u8; 32] = [42u8; 32];

/// Long enough for the actor to run the receive handler and for the future it
/// spawns to complete its two store reads plus the stubbed stream open.
const SETTLE: Duration = Duration::from_millis(300);

fn signed_invitation(
    inviter_sk: &PrivateKey,
    group_id: ContextGroupId,
) -> SignedGroupOpenInvitation {
    let invitation = GroupInvitationFromAdmin {
        inviter_identity: SignerId::from(*inviter_sk.public_key().digest()),
        group_id,
        expiration_timestamp: 0,
        invitation_nonce: [0x42; 32],
        invited_role: 1,
    };
    let bytes = borsh::to_vec(&invitation).expect("borsh");
    let signature = inviter_sk
        .sign(&Sha256::digest(&bytes))
        .expect("sign invitation");
    SignedGroupOpenInvitation {
        invitation,
        inviter_signature: hex::encode(signature.to_bytes()),
        application_id: None,
        app_key: None,
    }
}

fn signed_beacon(
    sk: &PrivateKey,
    ts_millis: u64,
    admission_proof: Option<SignedGroupOpenInvitation>,
) -> SignedReadinessBeacon {
    let mut beacon = SignedReadinessBeacon {
        namespace_id: NS.into(),
        peer_pubkey: sk.public_key(),
        dag_head: [9u8; 32],
        applied_through: 17,
        ts_millis,
        strong: true,
        admission_proof,
        signature: [0u8; 64],
    };
    beacon.signature = sk
        .sign(&beacon.signable_bytes().expect("signable_bytes"))
        .expect("sign beacon")
        .to_bytes();
    beacon
}

/// Make this node an established member of `NS` with an inviting admin: an
/// Admin member row (so the invitation's inviter passes the permission gate)
/// plus a non-zero governance head (so the divergence check sees local state).
fn seed_established_namespace(store: &Store, admin_sk: &PrivateKey) {
    MembershipRepository::new(store)
        .add_member(
            &ContextGroupId::from(NS),
            &admin_sk.public_key(),
            GroupMemberRole::Admin,
        )
        .expect("seed admin member row");

    let mut handle = store.handle();
    handle
        .put(
            &calimero_store::key::NamespaceGovHead::new(NS),
            &calimero_store::key::NamespaceGovHeadValue {
                sequence: 1,
                dag_heads: vec![[1u8; 32]],
            },
        )
        .expect("seed namespace governance head");
}

/// Wrap `beacon` exactly as the publisher does and deliver it to the running
/// `Handler<NetworkEvent>` from `source`.
async fn deliver(node: &TestNode, source: PeerId, beacon: SignedReadinessBeacon) {
    let payload =
        borsh::to_vec(&NamespaceTopicMsg::ReadinessBeacon(beacon)).expect("borsh beacon msg");
    let envelope = BroadcastMessage::NamespaceGovernanceDelta {
        namespace_id: NS,
        delta_id: [0u8; 32],
        parent_ids: Vec::new(),
        payload,
    };
    let event = NetworkEvent::Message {
        id: MessageId(b"test-beacon".to_vec()),
        message: Message {
            source: Some(source),
            data: borsh::to_vec(&envelope).expect("borsh envelope"),
            sequence_number: Some(1),
            topic: IdentTopic::new(format!("ns/{}", hex::encode(NS))).hash(),
        },
    };
    node.node_addr
        .send(event)
        .await
        .expect("deliver NetworkEvent to node actor");
    sleep(SETTLE).await;
}

fn stream_opens(node: &TestNode) -> Vec<PeerId> {
    node.stream_opens
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone()
}

#[actix::test]
async fn provable_beacon_pulls_from_its_signer_and_never_caches() {
    let node = boot_test_node().await;
    let admin_sk = PrivateKey::random(&mut rand::thread_rng());
    let joiner_sk = PrivateKey::random(&mut rand::thread_rng());
    seed_established_namespace(&node.store, &admin_sk);

    let inv = signed_invitation(&admin_sk, ContextGroupId::from(NS));
    let joiner_peer = PeerId::random();

    // A beacon whose wall-clock is a minute-plus behind ours is a replay
    // candidate, so it must unlock nothing - even carrying the same valid
    // invitation the fresh beacon below is accepted with.
    let stale = signed_beacon(
        &joiner_sk,
        now_millis().saturating_sub(61_000),
        Some(inv.clone()),
    );
    deliver(&node, joiner_peer, stale).await;
    assert!(
        stream_opens(&node).is_empty(),
        "a stale beacon must not trigger a pull"
    );

    // Same beacon, current clock: the pull fires, and it goes to the beacon's
    // own publisher - the only peer holding its not-yet-gossiped join op.
    let fresh = signed_beacon(&joiner_sk, now_millis(), Some(inv));
    deliver(&node, joiner_peer, fresh).await;
    assert_eq!(
        stream_opens(&node),
        vec![joiner_peer],
        "the pull must target the beacon's signer, not an arbitrary mesh peer"
    );

    // The whole point of the path: it unlocks a pull and writes nothing. A
    // non-member's advertised state must never become peer readiness state.
    assert!(
        node.readiness_cache
            .fresh_peers(NS, Duration::from_secs(60))
            .is_empty(),
        "a provable non-member beacon must never enter the readiness cache"
    );
}

#[actix::test]
async fn unprovable_beacon_pulls_nothing() {
    let node = boot_test_node().await;
    let admin_sk = PrivateKey::random(&mut rand::thread_rng());
    let stranger_sk = PrivateKey::random(&mut rand::thread_rng());
    seed_established_namespace(&node.store, &admin_sk);

    // Correctly signed, current clock, but no admission proof: indistinguishable
    // from today's silent drop.
    let beacon = signed_beacon(&stranger_sk, now_millis(), None);
    deliver(&node, PeerId::random(), beacon).await;

    assert!(
        stream_opens(&node).is_empty(),
        "a beacon without an admission proof must not trigger a pull"
    );
    assert!(
        node.readiness_cache
            .fresh_peers(NS, Duration::from_secs(60))
            .is_empty(),
        "an unverifiable beacon must not enter the readiness cache"
    );
}
