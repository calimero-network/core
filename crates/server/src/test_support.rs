//! Test scaffolding shared across the server's transports.
//!
//! Lives at the crate root rather than under one transport because the same
//! node-actor stub backs the WS, SSE, and JSON-RPC tests: all three read the
//! node's ephemeral-presence snapshot, and none of them should stand up a real
//! `NodeManager` to do it.

use actix::{Actor, Context as ActixContext, Handler};
use calimero_blobstore::config::BlobStoreConfig;
use calimero_blobstore::{BlobManager as BlobStore, FileSystem};
use calimero_network_primitives::client::NetworkClient;
use calimero_node_primitives::client::{BlobManager, NodeClient, SyncClient};
use calimero_node_primitives::messages::NodeMessage;
use calimero_primitives::events::NodeEvent;
use calimero_primitives::hash::Hash;
use calimero_primitives::identity::PublicKey;
use calimero_store::Store;
use calimero_utils_actix::LazyRecipient;
use tempfile::TempDir;
use tokio::sync::{broadcast, mpsc};

/// One live presence entry as the node's awareness store reports it:
/// `(author, slice, age_ms)`.
pub(crate) type SnapshotEntry = calimero_node_primitives::presence::PresenceSnapshotEntry;

/// Stand-in for `NodeManager`, answering only the messages the presence paths
/// send. Anything else is dropped — its `oneshot` sender goes with it, so a
/// caller sees a closed channel rather than hanging.
///
/// Must be created inside an actix System (`#[actix::test]`).
pub(crate) struct StubNodeManager {
    /// The reply to every `GetEphemeralSnapshot`.
    snapshot: Vec<SnapshotEntry>,
    /// An event to broadcast *while* a snapshot request is being served, and
    /// before it is answered.
    ///
    /// This is how a test pins the subscribe/replay interleaving: the server
    /// must have the subscription live before it reads the snapshot, so an
    /// event emitted at exactly this moment has to reach the subscriber. If
    /// the order were ever flipped to snapshot-then-subscribe, this event would
    /// fall in the gap and the test that waits for it would fail.
    interleaved: Option<(broadcast::Sender<NodeEvent>, NodeEvent)>,
}

impl Actor for StubNodeManager {
    type Context = ActixContext<Self>;
}

impl Handler<NodeMessage> for StubNodeManager {
    type Result = ();

    fn handle(&mut self, msg: NodeMessage, _ctx: &mut Self::Context) {
        if let NodeMessage::GetEphemeralSnapshot { outcome, .. } = msg {
            if let Some((sender, event)) = &self.interleaved {
                // A send with no receivers is not an error the test cares
                // about — the assertion is on what the subscriber sees.
                let _ignored = sender.send(event.clone());
            }
            let _ignored = outcome.send(self.snapshot.clone());
        }
    }
}

/// Start a [`StubNodeManager`] answering snapshot reads with `snapshot`, and
/// hand back the recipient wired to it.
pub(crate) fn stub_node_manager(snapshot: Vec<SnapshotEntry>) -> LazyRecipient<NodeMessage> {
    stub_node_manager_full(snapshot, None)
}

/// As [`stub_node_manager`], but broadcasts `event` on `sender` while serving
/// each snapshot read — see [`StubNodeManager::interleaved`].
pub(crate) fn stub_node_manager_interleaving(
    snapshot: Vec<SnapshotEntry>,
    sender: broadcast::Sender<NodeEvent>,
    event: NodeEvent,
) -> LazyRecipient<NodeMessage> {
    stub_node_manager_full(snapshot, Some((sender, event)))
}

fn stub_node_manager_full(
    snapshot: Vec<SnapshotEntry>,
    interleaved: Option<(broadcast::Sender<NodeEvent>, NodeEvent)>,
) -> LazyRecipient<NodeMessage> {
    let recipient = LazyRecipient::<NodeMessage>::new();
    let handle = recipient.clone();
    let _addr = StubNodeManager::create(move |ctx| {
        assert!(handle.init(ctx), "node manager recipient init");
        StubNodeManager {
            snapshot,
            interleaved,
        }
    });
    recipient
}

/// Make `public_key` a member of `context_id` without an owned private key.
///
/// This is the row `ContextClient::has_member` reads on its fast path, so the
/// key passes the membership gate — while still resolving to *no owned
/// identity*, which keeps a handler from running off into the node actor.
pub(crate) fn seed_context_member(
    store: &calimero_store::Store,
    context_id: calimero_primitives::context::ContextId,
    public_key: PublicKey,
) {
    let key = calimero_store::key::ContextIdentity::new(context_id, public_key);
    let value = calimero_store::types::ContextIdentity { private_key: None };
    store.handle().put(&key, &value).expect("put identity");
}

/// A `NodeClient` over `store`, routing actor messages to `node_manager` and
/// publishing node events on `event_sender`.
///
/// The returned `TempDir` backs the blob store and must outlive the client —
/// dropping it deletes the directory out from under it.
pub(crate) async fn test_node_client(
    store: &Store,
    node_manager: LazyRecipient<NodeMessage>,
    event_sender: broadcast::Sender<NodeEvent>,
) -> (NodeClient, TempDir) {
    let blob_dir = TempDir::new().expect("tempdir");
    let blob_store = BlobStore::new(
        store.clone(),
        FileSystem::new(&BlobStoreConfig::new(
            blob_dir.path().to_path_buf().try_into().expect("utf8 path"),
        ))
        .await
        .expect("blob fs"),
    );

    let (ctx_sync_tx, _r0) = mpsc::channel(8);
    let (ns_sync_tx, _r1) = mpsc::channel(8);
    let (ns_join_tx, _r2) = mpsc::channel(8);
    let (open_subgroup_join_tx, _r3) = mpsc::channel(8);
    let (relay_sealed_join_tx, _r4) = mpsc::channel(8);
    let sync_client = SyncClient::new(
        ctx_sync_tx,
        ns_sync_tx,
        ns_join_tx,
        open_subgroup_join_tx,
        relay_sealed_join_tx,
    );

    let node_client = NodeClient::new(
        store.clone(),
        BlobManager::new(blob_store),
        NetworkClient::new(LazyRecipient::new()),
        node_manager,
        event_sender,
        sync_client,
        None,
    );

    (node_client, blob_dir)
}

/// An admin state over `store`, with this node's account root provisioned.
pub(crate) async fn admin_state(store: &Store) -> (std::sync::Arc<crate::AdminState>, TempDir) {
    calimero_governance_store::NodeDeviceRepository::new(store)
        .provision_account_root()
        .expect("this node's account root");
    let (event_sender, _rx) = broadcast::channel(16);
    let (node_client, blob_dir) =
        test_node_client(store, stub_node_manager(vec![]), event_sender).await;
    let ctx_client = calimero_context_client::client::ContextClient::new(
        store.clone(),
        node_client.clone(),
        LazyRecipient::new(),
    );
    let state = std::sync::Arc::new(crate::AdminState::new(
        store.clone(),
        ctx_client,
        node_client,
        std::sync::Arc::new(crate::NodeReadiness::new()),
        [0; 32],
        #[cfg(feature = "mock-attestation")]
        false,
    ));
    (state, blob_dir)
}

/// Seed a namespace with one Restricted subgroup and `caller` in `role`,
/// returning the namespace and subgroup ids as wire hashes plus the account
/// `caller`'s key resolves to - the principal every row below is keyed by,
/// and what the subscribe gate compares against.
pub(crate) fn seed_namespace_with_restricted_subgroup(
    store: &Store,
    caller: PublicKey,
    role: calimero_primitives::context::GroupMemberRole,
) -> (Hash, Hash, calimero_primitives::identity::AccountId) {
    use calimero_context_config::types::ContextGroupId;
    use calimero_context_config::VisibilityMode;
    use calimero_governance_store::{
        CapabilitiesRepository, MembershipRepository, NamespaceRepository,
    };

    let ns = ContextGroupId::from([0xC0u8; 32]);
    let subgroup = ContextGroupId::from([0xC1u8; 32]);

    // Enrolled at the namespace anchor, so the caller's key resolves there
    // and every row below names the account it resolves to.
    let account = calimero_context::test_support::enrol(store, &ns, &caller);

    MembershipRepository::new(store)
        .add_member(&ns, &account, role)
        .unwrap();
    NamespaceRepository::new(store)
        .nest(&ns, &subgroup)
        .unwrap();
    CapabilitiesRepository::new(store)
        .set_subgroup_visibility(&subgroup, VisibilityMode::Restricted)
        .unwrap();

    (
        Hash::from(ns.to_bytes()),
        Hash::from(subgroup.to_bytes()),
        account,
    )
}

/// Nest `subgroup` under `namespace` with `context` in it, and make each key a
/// member of both under its own account, bound as one device of that account.
pub(crate) fn seed_device_members<const N: usize>(
    store: &Store,
    namespace: &calimero_context_config::types::ContextGroupId,
    subgroup: &calimero_context_config::types::ContextGroupId,
    context: &calimero_primitives::context::ContextId,
    keys: [PublicKey; N],
) -> [(
    calimero_account::AccountId,
    calimero_primitives::identity::DeviceId,
); N] {
    use calimero_governance_store::test_fixtures::{enrol_member, real_join_account};
    use calimero_governance_store::{MembershipRepository, NamespaceRepository};
    use calimero_primitives::context::GroupMemberRole;

    NamespaceRepository::new(store)
        .nest(namespace, subgroup)
        .unwrap();
    calimero_governance_store::register_context_in_group(store, subgroup, context).unwrap();
    keys.map(|key| {
        let account = enrol_member(store, namespace, &key);
        for group in [namespace, subgroup] {
            MembershipRepository::new(store)
                .add_member(group, &account, GroupMemberRole::Member)
                .unwrap();
        }
        (account, real_join_account(&key).statement.device)
    })
}

/// Apply `op` to `namespace`, signed by `signer`, through the governance apply path.
pub(crate) fn apply_through_governance(
    store: &Store,
    namespace: &calimero_context_config::types::ContextGroupId,
    signer: &calimero_primitives::identity::PrivateKey,
    op: calimero_context_client::local_governance::GroupOp,
) {
    use calimero_governance_store::test_fixtures::test_meta;

    calimero_governance_store::MetaRepository::new(store)
        .save(namespace, &test_meta())
        .unwrap();
    let _applied =
        calimero_governance_store::sign_apply_local_group_op_borsh(store, namespace, signer, op)
            .expect("the op applies");
}

/// Withdraw `device` from `namespace` the way a peer's revocation lands: an
/// admin's signed `AccountDeviceUnlinked`, applied through the governance path.
pub(crate) fn revoke_through_governance(
    store: &Store,
    namespace: &calimero_context_config::types::ContextGroupId,
    account: calimero_account::AccountId,
    device: calimero_primitives::identity::DeviceId,
) {
    use calimero_governance_store::test_fixtures::enrol_member;
    use calimero_governance_store::MembershipRepository;
    use calimero_primitives::context::GroupMemberRole;
    use calimero_primitives::identity::PrivateKey;

    let admin_sk = PrivateKey::from([0xAD; 32]);
    let admin = enrol_member(store, namespace, &admin_sk.public_key());
    MembershipRepository::new(store)
        .add_member(namespace, &admin, GroupMemberRole::Admin)
        .unwrap();
    apply_through_governance(
        store,
        namespace,
        &admin_sk,
        calimero_context_client::local_governance::GroupOp::AccountDeviceUnlinked {
            account,
            device,
            proof: None,
        },
    );
}

/// The account whose root is `root` withdrawing its own `device`, by root-signed proof.
pub(crate) fn withdrawal_by_root(
    root: &calimero_primitives::identity::PrivateKey,
    device: calimero_primitives::identity::DeviceId,
) -> calimero_context_client::local_governance::GroupOp {
    let genesis = calimero_account::AccountGenesis::new(root.public_key());
    let account = genesis.account_id();
    calimero_context_client::local_governance::GroupOp::AccountDeviceUnlinked {
        account,
        device,
        proof: Some(calimero_account::SignedDeviceRevocation {
            genesis,
            chain: vec![],
            statement: calimero_account::DeviceRevocation::sign(root, account, device, 0)
                .expect("the root signs its own revocation"),
        }),
    }
}

/// The account whose root is `root` narrowing `device` to an application the
/// fixture namespace does not target.
pub(crate) fn descope_by_root(
    root: &calimero_primitives::identity::PrivateKey,
    device: calimero_primitives::identity::DeviceId,
) -> calimero_context_client::local_governance::GroupOp {
    let genesis = calimero_account::AccountGenesis::new(root.public_key());
    let account = genesis.account_id();
    let elsewhere = calimero_primitives::application::ApplicationId::from([0x11; 32]);
    calimero_context_client::local_governance::GroupOp::AccountDeviceDescoped {
        account,
        device,
        application: Some(
            calimero_governance_store::test_fixtures::test_meta()
                .target
                .application_id,
        ),
        scope: Box::new(calimero_account::AccountProof {
            genesis,
            chain: vec![],
            statement: calimero_account::DeviceScope::sign(
                root,
                account,
                device,
                vec![elsewhere],
                1,
                0,
            )
            .expect("the root signs its own device's scope"),
        }),
    }
}
