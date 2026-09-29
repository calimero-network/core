//! The sync responder, driven over an in-memory stream on a booted node: it
//! serves only the context its `Init` proved, admits Public writes only from an
//! initiator that may write that context, and proves who it serves as.

use std::time::Duration;

use calimero_context_config::types::ContextGroupId;
use calimero_governance_store::test_fixtures::{enrol_member, test_meta};
use calimero_governance_store::{
    register_context_in_group, MembershipRepository, MetaRepository, NodeDeviceRepository,
};
use calimero_network_primitives::stream::Stream;
use calimero_node_primitives::sync::{InitPayload, InitProof, MessagePayload, StreamMessage};
use calimero_primitives::context::{ContextId, GroupMemberRole};
use calimero_primitives::identity::PrivateKey;
use calimero_store::key::{
    ApplicationMeta as ApplicationMetaKey, ContextIdentity, ContextMeta as ContextMetaKey,
};
use calimero_store::types::ContextMeta;
use calimero_store::Store;
use libp2p::PeerId;
use rand::rand_core::UnwrapErr;
use rand::rngs::SysRng;
use serial_test::serial;

use crate::sync::helpers::generate_nonce;
use crate::sync::public_entries::PublicEntries;
use crate::sync::stream::{recv, send};
use crate::sync::SyncManager;
use crate::test_node_harness::boot_test_node;

const CONTEXT: [u8; 32] = [0xA0; 32];
const OTHER_CONTEXT: [u8; 32] = [0xB0; 32];
const REPLY_BUDGET: Duration = Duration::from_secs(5);

/// `CONTEXT` in a group, hosting a writing member and a context-only identity,
/// beside `OTHER_CONTEXT`; each context has one DAG head.
struct Hosted {
    writer: PrivateKey,
    outsider: PrivateKey,
    entries: PublicEntries,
}

fn host_contexts(store: &Store) -> Hosted {
    let mut rng = UnwrapErr(SysRng);
    let writer = PrivateKey::random(&mut rng);
    let outsider = PrivateKey::random(&mut rng);
    let group = ContextGroupId::from([0xA9; 32]);
    NodeDeviceRepository::new(store)
        .provision_account_root()
        .expect("the account root an initialised node has");
    MetaRepository::new(store)
        .save(&group, &test_meta())
        .expect("group meta");
    let account = enrol_member(store, &group, &writer.public_key());
    MembershipRepository::new(store)
        .add_member(&group, &account, GroupMemberRole::Member)
        .expect("writer row");

    let application = ApplicationMetaKey::new(test_meta().target.application_id);
    let mut handle = store.handle();
    for (context, head) in [(CONTEXT, [0xA1; 32]), (OTHER_CONTEXT, [0xB1; 32])] {
        let meta = ContextMeta::new(application, [0x01; 32], vec![head], None);
        handle
            .put(&ContextMetaKey::new(context.into()), &meta)
            .expect("context meta");
    }
    for key in [&writer, &outsider] {
        handle
            .put(
                &ContextIdentity::new(CONTEXT.into(), key.public_key()),
                &calimero_store::types::ContextIdentity {
                    private_key: Some(*key.as_bytes()),
                },
            )
            .expect("context identity");
    }
    register_context_in_group(store, &group, &CONTEXT.into()).expect("register");
    Hosted {
        writer,
        outsider,
        entries: PublicEntries::seed(store, CONTEXT.into()),
    }
}

/// Dial `manager` as `party` proven for `CONTEXT`, send `payload` and then
/// `follow_up`, and return the first reply.
async fn exchange(
    manager: &SyncManager,
    party: &PrivateKey,
    payload: InitPayload,
    follow_up: Option<InitPayload>,
) -> Option<StreamMessage<'static>> {
    let peer = PeerId::random();
    let context_id = ContextId::from(CONTEXT);
    let party_id = party.public_key();
    let proof = InitProof {
        signature: party
            .sign(&InitProof::message(
                &context_id,
                &party_id,
                &peer.to_bytes(),
            ))
            .expect("sign proof")
            .to_bytes(),
    };
    let init = |payload| StreamMessage::Init {
        context_id,
        party_id,
        payload,
        next_nonce: generate_nonce(),
        pop: Some(proof),
    };
    let (responder, mut dialer) = Stream::test_pair();
    let dial = async move {
        send(&mut dialer, &init(payload), None)
            .await
            .expect("send init");
        let reply = recv(&mut dialer, None, REPLY_BUDGET).await.ok().flatten();
        if let Some(payload) = follow_up {
            send(&mut dialer, &init(payload), None)
                .await
                .expect("send follow-up");
            let _ack = recv(&mut dialer, None, REPLY_BUDGET).await;
        }
        reply
    };
    let ((), reply) = tokio::join!(
        manager.handle_opened_stream(peer, Box::new(responder)),
        dial
    );
    reply
}

/// The DAG heads served for a request naming `requested`, or `None` when refused.
async fn dag_heads(
    manager: &SyncManager,
    party: &PrivateKey,
    requested: [u8; 32],
) -> Option<Vec<[u8; 32]>> {
    let payload = InitPayload::DagHeadsRequest {
        context_id: requested.into(),
    };
    match exchange(manager, party, payload, None).await {
        Some(StreamMessage::Message {
            payload: MessagePayload::DagHeadsResponse { dag_heads, .. },
            ..
        }) => Some(dag_heads),
        Some(StreamMessage::OpaqueError) => None,
        other => panic!("unexpected reply: {other:?}"),
    }
}

#[tokio::test]
#[serial(boot_test_node)]
async fn a_payload_naming_another_context_is_refused_not_served() {
    let node = boot_test_node().await;
    let hosted = host_contexts(&node.store);

    assert_eq!(
        dag_heads(&node.sync_manager, &hosted.writer, CONTEXT).await,
        Some(vec![[0xA1; 32]]),
        "control: the Init's own context is served"
    );
    assert_eq!(
        dag_heads(&node.sync_manager, &hosted.writer, OTHER_CONTEXT).await,
        None,
        "a payload naming another context must be refused"
    );
}

#[tokio::test]
#[serial(boot_test_node)]
async fn responders_admit_public_tombstones_only_from_a_writing_initiator() {
    for (protocol, first) in [
        (
            "LevelWise",
            InitPayload::LevelWiseRequest {
                context_id: CONTEXT.into(),
                level: 0,
                parent_ids: None,
            },
        ),
        (
            "HashComparison",
            InitPayload::TreeNodeRequest {
                context_id: CONTEXT.into(),
                node_id: CONTEXT,
                max_depth: Some(1),
            },
        ),
    ] {
        let node = boot_test_node().await;
        let hosted = host_contexts(&node.store);
        for (party, entry) in [(&hosted.writer, 0), (&hosted.outsider, 1)] {
            let _reply = exchange(
                &node.sync_manager,
                party,
                first.clone(),
                Some(InitPayload::EntityDeletePush {
                    context_id: CONTEXT.into(),
                    deletions: vec![hosted.entries.tombstone(entry)],
                }),
            )
            .await;
        }
        assert_eq!(
            [hosted.entries.deleted(0), hosted.entries.deleted(1)],
            [true, false],
            "{protocol}: a writing member's tombstone applies; a non-writer's is dropped"
        );
    }
}

/// A responder hosting a writer and a non-writer proves the writer, every time, so
/// an initiator that never heard it on gossip attributes the session to a writer.
#[tokio::test]
#[serial(boot_test_node)]
async fn a_dag_heads_reply_proves_the_identity_the_responder_serves_as() {
    let node = boot_test_node().await;
    let hosted = host_contexts(&node.store);
    let context_id = ContextId::from(CONTEXT);
    let own_peer = node.sync_manager.local_peer_id().await.to_bytes();

    for _ in 0..16 {
        let payload = InitPayload::DagHeadsRequest {
            context_id: CONTEXT.into(),
        };
        let reply = exchange(&node.sync_manager, &hosted.writer, payload, None).await;
        let Some(StreamMessage::Message {
            payload: MessagePayload::DagHeadsResponse { responder, .. },
            ..
        }) = reply
        else {
            panic!("unexpected reply: {reply:?}");
        };
        let proof = responder.expect("the reply names the identity the responder serves as");
        assert_eq!(
            proof.attributed_party(&context_id, &own_peer),
            Some(hosted.writer.public_key()),
            "the proof names the writing identity, not the non-writer beside it"
        );
        assert_eq!(
            proof.attributed_party(&context_id, &PeerId::random().to_bytes()),
            None,
            "a proof presented from another peer attributes nothing"
        );
        assert_eq!(
            proof.attributed_party(&OTHER_CONTEXT.into(), &own_peer),
            None,
            "a proof for one context attributes nothing in another"
        );
    }
}
