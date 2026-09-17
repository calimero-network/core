//! Fixtures the context crate's tests share.
//!
//! Enrolling a signing key so tests can name the account it speaks for.
//!
//! Governance rows name accounts, and an account is a one-way hash of a root
//! this crate never sees — so a test cannot simply derive one from a key. It has
//! to enrol the key the way a real join does, and read the account back.
//!
//! Deriving a stand-in instead (`AccountId::from(*pk)`) compiles and is always
//! wrong: both are 32 bytes, so the row lands under a principal that resolves to
//! nobody, and the gate the test meant to exercise refuses for a reason that has
//! nothing to do with what is under test.
//!
//! Available outside `cfg(test)` so the integration suites in `tests/` can use
//! them too; they write only test rows and are never called from a handler. The
//! actor harness below is the exception: it needs the dev-dependencies.

use calimero_account::AccountId;
use calimero_context_config::types::ContextGroupId;
use calimero_primitives::identity::{PrivateKey, PublicKey};
use calimero_store::Store;

/// The credential `sign_pk` would present, derived deterministically from the
/// key so the same key always speaks for the same account.
fn credential_for(
    sign_pk: &PublicKey,
) -> (
    calimero_account::AccountGenesis,
    calimero_account::DeviceCert,
) {
    let root_sk = PrivateKey::from(*(*sign_pk));
    let genesis = calimero_account::AccountGenesis::new(root_sk.public_key());
    let cert = calimero_account::DeviceCert::sign(
        &root_sk,
        genesis.account_id(),
        // The device id is derived from the signing key rather than fixed: a
        // constant would make every credential claim the same device, and the
        // second enrolment in any store would be refused as a reassignment.
        calimero_account::DeviceId::from(*(*sign_pk)),
        sign_pk,
        &calimero_account::KemPublicKey::from([0x2B; 32]),
        0,
        0,
    )
    .expect("the account root signs its own device cert");
    (genesis, cert)
}

/// The credential `sign_pk` presents when it joins.
///
/// Use this wherever an op carries an `account` beside a `member`: the two have
/// to name the same account, and a filler credential is refused before the op
/// reaches whatever the test is aiming at.
#[must_use]
pub fn credential(
    sign_pk: &PublicKey,
) -> Box<calimero_context_client::local_governance::JoinAccountCredential> {
    let (genesis, cert) = credential_for(sign_pk);
    Box::new(
        calimero_context_client::local_governance::JoinAccountCredential {
            genesis,
            chain: vec![],
            statement: cert,
        },
    )
}

/// The account `sign_pk` will speak for once enrolled.
#[must_use]
pub fn account_for(sign_pk: &PublicKey) -> AccountId {
    credential_for(sign_pk).1.account
}

/// Bind `sign_pk` to its account in `namespace`, and return that account.
///
/// Writes both rows a real join writes: the device binding and the endorser
/// entry the member->account direction is read through.
///
/// # Panics
///
/// Panics if the rows cannot be written, which in a test means the fixture is
/// wrong rather than the code under test.
pub fn enrol(store: &Store, namespace: &ContextGroupId, sign_pk: &PublicKey) -> AccountId {
    let (genesis, cert) = credential_for(sign_pk);
    let account = cert.account;
    let bindings = calimero_governance_store::AccountBindingRepository::new(store);
    bindings
        .record_endorser(namespace, account, &account)
        .expect("record the endorser");
    let _ = bindings
        .apply_link(namespace, &genesis, &[], &cert, 0)
        .expect("record the binding");
    account
}

/// Bind this node's OWN device to its own account in `namespace`, as founding or
/// joining it does, and return that account.
///
/// Not [`enrol`]: that derives a stand-in account from the key, so the node's
/// signing key there speaks for nobody its account root owns - and every gate
/// that asks "does this signer speak for this account" then refuses.
///
/// # Panics
///
/// Panics if the credential cannot be built or the rows cannot be written, which
/// in a test means the fixture is wrong rather than the code under test.
pub fn enrol_holder(store: &Store, namespace: &ContextGroupId, sign_pk: &PublicKey) -> AccountId {
    let credential =
        crate::join_credential::build(store, namespace, sign_pk).expect("this node's credential");
    let account = credential.statement.account;
    let bindings = calimero_governance_store::AccountBindingRepository::new(store);
    bindings
        .record_endorser(namespace, account, &account)
        .expect("record the endorser");
    let _ = bindings
        .apply_link(
            namespace,
            &credential.genesis,
            &credential.chain,
            &credential.statement,
            calimero_governance_store::JOIN_SCOPE_EPOCH,
        )
        .expect("record the binding");
    account
}

/// A second device of this node's account, certified by its root exactly as
/// `pair_device_complete` would, recorded in the account namespace's registry
/// and scoped to `applications` (empty is every application).
///
/// The id is `seed` repeated rather than minted, so the store's key-ordered scan
/// visits these devices in a known order.
///
/// # Panics
///
/// Panics if the root cannot be resolved or the certificate cannot be signed,
/// which in a test means the fixture is wrong rather than the code under test.
pub fn certify_device(
    store: &Store,
    seed: u8,
    applications: &[calimero_primitives::application::ApplicationId],
) -> calimero_account::DeviceId {
    let devices = calimero_governance_store::NodeDeviceRepository::new(store);
    let root = devices
        .provision_account_root()
        .expect("this node's account root");
    let device = calimero_account::DeviceId::from([seed; 32]);
    let proof = calimero_account::AccountProof {
        genesis: root.genesis(),
        chain: vec![],
        statement: calimero_account::DeviceCert::sign(
            root.signing_key(),
            root.account(),
            device,
            &PrivateKey::from([seed; 32]).public_key(),
            &calimero_account::KemPublicKey::from([seed ^ 0xFF; 32]),
            0,
            0,
        )
        .expect("the account root signs its own device cert"),
    };
    // The registry lives in the account namespace, which a node holding a root
    // names from that root before anything has created it.
    let namespace = devices
        .account_namespace()
        .expect("read the account namespace")
        .expect("a store with an account root names one");
    let scope = crate::account_namespace::next_device_scope(
        store,
        Some(namespace),
        &root,
        &proof,
        applications,
    )
    .expect("the account root signs its own device scope");
    let _recorded = calimero_governance_store::AccountDeviceRegistry::new(store, namespace)
        .record(&proof, &scope)
        .expect("record the device in the account namespace");
    device
}

/// The root-signed scope a registry row carries, from the same root as `proof`.
///
/// # Panics
///
/// Panics if the root refuses to sign, which in a test means the fixture is wrong.
#[must_use]
pub fn device_scope(
    root_sk: &PrivateKey,
    proof: &calimero_account::AccountProof<calimero_account::DeviceCert>,
    applications: &[calimero_primitives::application::ApplicationId],
    scope_epoch: u32,
) -> calimero_account::AccountProof<calimero_account::DeviceScope> {
    calimero_account::AccountProof {
        genesis: proof.genesis,
        chain: vec![],
        statement: calimero_account::DeviceScope::sign(
            root_sk,
            proof.statement.account,
            proof.statement.device,
            applications.to_vec(),
            scope_epoch,
            0,
        )
        .expect("the account root signs the device's scope"),
    }
}

/// This node as a DEVICE of an account whose root lives elsewhere, scoped to
/// `applications` (empty is every application): the account namespace recorded by
/// pairing and taken part in, this node's device minted under the account's
/// genesis, and its own certified row folded - the state a device is in once
/// pairing has settled.
///
/// A device and not a holder on purpose. `NodeDeviceRepository::account_namespace`
/// answers a holder from its root derivation and only falls through to the stored
/// row otherwise, so a holder fixture would exercise the wrong read. The account
/// root comes back beside the device because it lives nowhere in this store, and a
/// caller that has to sign for the account has no other source.
///
/// # Panics
///
/// Panics if any of the rows cannot be written, which in a test means the fixture
/// is wrong rather than the code under test.
pub fn paired_device_scoped_to(
    store: &Store,
    account_namespace: &ContextGroupId,
    applications: &[calimero_primitives::application::ApplicationId],
) -> (calimero_account::DeviceId, PrivateKey) {
    let root_sk = PrivateKey::from([0x70; 32]);
    let genesis = calimero_account::AccountGenesis::new(root_sk.public_key());
    let account = genesis.account_id();

    let devices = calimero_governance_store::NodeDeviceRepository::new(store);
    devices
        .store_account_namespace(account_namespace)
        .expect("record what the pairing named");
    let held = devices
        .ensure_enrolled_into(&[*account_namespace], genesis)
        .expect("mint this node's device");

    let proof = calimero_account::AccountProof {
        genesis,
        chain: vec![],
        statement: calimero_account::DeviceCert::sign(
            &root_sk,
            account,
            held.device(),
            &PrivateKey::from([0x71; 32]).public_key(),
            &calimero_account::KemPublicKey::from([0x72; 32]),
            0,
            0,
        )
        .expect("the account root signs this device's certificate"),
    };
    let _recorded =
        calimero_governance_store::AccountDeviceRegistry::new(store, *account_namespace)
            .record(&proof, &device_scope(&root_sk, &proof, applications, 0))
            .expect("record this device in its account's registry");
    let _identity = calimero_governance_store::NamespaceRepository::new(store)
        .participate_in(account_namespace)
        .expect("this node takes part in its own account namespace");
    (held.device(), root_sk)
}

/// This node as the HOLDER of its account: its own root, its own device row and
/// its own registry row, scoped to `applications` (empty is every application).
///
/// The counterpart of [`paired_device_scoped_to`], for the reads that answer
/// differently on a node holding the root its statements are signed by.
///
/// # Panics
///
/// Panics if any of the rows cannot be written, which in a test means the fixture
/// is wrong rather than the code under test.
pub fn holder_device_scoped_to(
    store: &Store,
    applications: &[calimero_primitives::application::ApplicationId],
) -> (ContextGroupId, calimero_account::DeviceId) {
    let devices = calimero_governance_store::NodeDeviceRepository::new(store);
    let root = devices
        .provision_account_root()
        .expect("this node's account root");
    let account_namespace = root.account_namespace();
    devices
        .store_account_namespace(&account_namespace)
        .expect("name the account namespace");
    let (_namespace, signer_pk, _signer_sk) =
        calimero_governance_store::NamespaceRepository::new(store)
            .participate_in(&account_namespace)
            .expect("this node takes part in its own account namespace");
    let credential = crate::join_credential::build(store, &account_namespace, &signer_pk)
        .expect("mint and certify this node's own device");
    let proof = calimero_account::AccountProof {
        genesis: credential.genesis,
        chain: credential.chain.clone(),
        statement: credential.statement,
    };
    let device = proof.statement.device;
    let _recorded = calimero_governance_store::AccountDeviceRegistry::new(store, account_namespace)
        .record(
            &proof,
            &device_scope(root.signing_key(), &proof, applications, 0),
        )
        .expect("record the holder's own device in its registry");
    (account_namespace, device)
}

/// Replace this node's own scope with `applications` at `scope_epoch`, as folding
/// the account holder's `AccountDeviceCertified` does.
///
/// # Panics
///
/// Panics if the registry row cannot be read or written.
pub fn rescope_paired_device(
    store: &Store,
    account_namespace: &ContextGroupId,
    device: calimero_account::DeviceId,
    root_sk: &PrivateKey,
    applications: &[calimero_primitives::application::ApplicationId],
    scope_epoch: u32,
) {
    let registry = calimero_governance_store::AccountDeviceRegistry::new(store, *account_namespace);
    let proof = registry
        .device(device)
        .expect("read the registry row")
        .expect("the device was certified first")
        .proof;
    let _recorded = registry
        .record(
            &proof,
            &device_scope(root_sk, &proof, applications, scope_epoch),
        )
        .expect("record the replacement scope");
}

/// Wrap a root op the way its publisher does: sealed under the namespace key
/// when [`calimero_governance_types::root_op_is_sealable`] says the variant
/// travels that way, cleartext when it does not.
///
/// Apply refuses a sealable root op that arrives in the clear, so a test that
/// hand-builds `NamespaceOp::Root(..)` for one of those variants is constructing
/// something no peer accepts — and it fails for that reason rather than the one
/// the test is about.
///
/// Mints the namespace key when the fixture has not. Production keys a namespace
/// at creation (its root is a group, and `create_group` keys whatever group it
/// creates), so a fixture without one is under-built rather than exercising a
/// real state.
///
/// # Panics
///
/// Panics if the keyring cannot be read or written, or if the op cannot be
/// sealed — in a test that means the fixture is wrong.
#[must_use]
pub fn published_root(
    store: &Store,
    namespace: &ContextGroupId,
    op: calimero_context_client::local_governance::RootOp,
) -> calimero_context_client::local_governance::NamespaceOp {
    let keyring = calimero_governance_store::GroupKeyring::new(store, *namespace);
    if keyring
        .load_current_key()
        .expect("read the namespace keyring")
        .is_none()
    {
        let _ = keyring
            .store_key(&[0x5Au8; 32])
            .expect("mint the namespace key the fixture omitted");
    }
    calimero_governance_store::seal_root_op_for_publish(store, namespace.to_bytes().into(), op)
        .expect("seal a root op for a test")
}

/// The wire form production publishes for a JOIN, given the group its
/// invitation targets, **as a joiner that holds the covering key publishes it**.
///
/// [`published_root`] is not the helper for this: `seal_root_op_for_publish`
/// answers `Root(op)` for `MemberJoined` / `MemberJoinedAt`, because
/// `root_op_is_sealable` says those two are not sealable under the NAMESPACE
/// key — a namespace-root joiner holds no key, and its key arrives only in
/// answer to the join it is publishing.
///
/// That cleartext answer is what a RECEIVER accepts, and it is the right fixture
/// for an apply-path test. It is no longer what an unkeyed joiner puts on the
/// wire: since #3904 such a joiner hands its signed op to the admitter, which
/// publishes it as `NamespaceOp::RootRelaySealed`. A test about that route wants
/// `relay_seal_for_test`-shaped state, not this.
///
/// A **subgroup**-targeted join is different: its bundle delivers that group's
/// key, `join_group` stores it before publishing, and the apply refuses a
/// cleartext one (#3858). So a fixture that publishes it in the clear is
/// under-building the state rather than exercising a real one, and its test
/// fails on the fixture instead of on its subject.
///
/// The covering key is minted when the store has none, which is what the join
/// bundle would have delivered.
///
/// # Panics
///
/// Panics if the keyring cannot be read or written, or if the op cannot be
/// sealed — in a test that means the fixture is wrong.
#[must_use]
pub fn published_join(
    store: &Store,
    namespace: &ContextGroupId,
    op: calimero_context_client::local_governance::RootOp,
) -> calimero_context_client::local_governance::NamespaceOp {
    use calimero_context_client::local_governance::{NamespaceOp, RootOp};

    let target = match &op {
        RootOp::MemberJoined {
            signed_invitation, ..
        }
        | RootOp::MemberJoinedAt {
            signed_invitation, ..
        } => signed_invitation.invitation.group_id,
        // Not a join; nothing here applies.
        _ => return NamespaceOp::Root(op),
    };
    if target.to_bytes() == namespace.to_bytes() {
        return NamespaceOp::Root(op);
    }

    let covering = calimero_governance_store::key_covering_group(store, &target)
        .expect("resolve the covering group");
    let keyring = calimero_governance_store::GroupKeyring::new(store, covering);
    if keyring
        .load_current_key()
        .expect("read the covering keyring")
        .is_none()
    {
        let _ = keyring
            .store_key(&[0x5Bu8; 32])
            .expect("mint the key the join bundle would have delivered");
    }
    calimero_governance_store::seal_root_op_for_group_if_keyed(store, target, &op)
        .expect("seal a subgroup-targeted join for a test")
        .expect("the covering key was just ensured, so the seal must produce a sealed op")
}

/// The [`RootOp`] a signed namespace op carries, opened if it arrived sealed.
///
/// The projection folds the OPENED root — `scope_projection` decrypts a
/// `NamespaceOp::RootSealed` and hands the inner op to
/// `op_from_namespace_op_with_binding` — so a test that feeds the sealed
/// envelope alone folds a `Noop` and proves nothing about the op it built.
///
/// `None` for a cleartext root op (which needs no opening) and for a group op.
///
/// # Panics
///
/// Panics if the keyring cannot be read or the sealed op will not open, which in
/// a test means the fixture sealed under a key it then did not keep.
#[must_use]
pub fn opened_root(
    store: &Store,
    namespace: &ContextGroupId,
    signed: &calimero_context_client::local_governance::SignedNamespaceOp,
) -> Option<calimero_context_client::local_governance::RootOp> {
    // Both sealed shapes, each resolved in the keyring that can open it. A
    // subgroup-sealed join resolves in the group the envelope names, not the
    // namespace's — reading it there would miss the key and panic on a fixture
    // that is in fact correct.
    let (keyring_group, key_id, encrypted) = match &signed.op {
        calimero_context_client::local_governance::NamespaceOp::RootSealed {
            key_id,
            encrypted,
        } => (*namespace, key_id, encrypted),
        calimero_context_client::local_governance::NamespaceOp::RootSealedForGroup {
            group_id,
            key_id,
            encrypted,
        } => (*group_id, key_id, encrypted),
        _ => return None,
    };
    let key = calimero_governance_store::GroupKeyring::new(store, keyring_group)
        .load_key_by_id(key_id.as_bytes())
        .expect("read the keyring the envelope names")
        .expect("the fixture kept the key it sealed under");
    Some(
        calimero_governance_store::GroupKeyring::decrypt_root_op(&key, encrypted)
            .expect("open a root op this fixture sealed"),
    )
}

/// Poll `read` until it answers true, bounded: a gain with no target yet is
/// announced off the caller, so reading straight after is a race.
#[cfg(test)]
pub(crate) async fn eventually(mut read: impl FnMut() -> bool) -> bool {
    for _ in 0..100 {
        if read() {
            return true;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    false
}

/// A live [`ContextManager`](crate::ContextManager) over a caller-supplied
/// store, for handler logic that only an actor can reach.
///
/// The whole of the missing piece is the network: the node fixture leaves its
/// recipient unbound, and an unbound recipient queues rather than declines, so a
/// handler that subscribes or publishes never returns without an actor in front
/// of it. Everything else is
/// [`calimero_node_primitives::test_fixtures::node_client_over`].
#[cfg(test)]
pub(crate) mod actor {
    use actix::{Actor, Addr, Context, Handler};
    use calimero_context_client::client::ContextClient;
    use calimero_network_primitives::client::NetworkClient;
    use calimero_network_primitives::messages::{MessageId, NetworkMessage};
    use calimero_node_primitives::client::NodeClient;
    use calimero_node_primitives::test_fixtures::node_client_over;
    use calimero_store::Store;
    use calimero_utils_actix::LazyRecipient;
    use tempfile::TempDir;
    use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};

    use crate::ContextManager;

    /// Answers the four commands the pairing and governance paths issue, and
    /// records the topics. Any other is dropped, which panics its caller.
    struct StubNetwork {
        subscribed: UnboundedSender<String>,
        unsubscribed: UnboundedSender<String>,
        broadcast: UnboundedSender<String>,
    }

    impl Actor for StubNetwork {
        type Context = Context<Self>;
    }

    impl Handler<NetworkMessage> for StubNetwork {
        type Result = ();

        fn handle(&mut self, msg: NetworkMessage, _ctx: &mut Self::Context) {
            match msg {
                NetworkMessage::Subscribe { request, outcome } => {
                    let _ignored = self.subscribed.send(request.0.to_string());
                    let _ignored = outcome.send(Ok(request.0));
                }
                NetworkMessage::Unsubscribe { request, outcome } => {
                    let _ignored = self.unsubscribed.send(request.0.to_string());
                    let _ignored = outcome.send(Ok(request.0));
                }
                NetworkMessage::MeshPeerCount { request, outcome } => {
                    let _ignored = self.broadcast.send(request.0.to_string());
                    let _ignored = outcome.send(0);
                }
                NetworkMessage::Publish { request, outcome } => {
                    let _ignored = self.broadcast.send(request.topic.to_string());
                    let _ignored = outcome.send(Ok(MessageId(b"stub".to_vec())));
                }
                _ => {}
            }
        }
    }

    /// A started `ContextManager`, and a client routed to it for the paths that
    /// are plain functions. Seed the store, drive the path, then assert on the
    /// rows it wrote.
    pub(crate) struct Harness {
        pub manager: Addr<ContextManager>,
        pub node_client: NodeClient,
        pub context_client: ContextClient,
        subscribed: UnboundedReceiver<String>,
        unsubscribed: UnboundedReceiver<String>,
        broadcast: UnboundedReceiver<String>,
        // The blob filesystem and the node's data root outlive the manager.
        _dirs: (TempDir, TempDir),
        _network: Addr<StubNetwork>,
    }

    impl Harness {
        /// Every topic subscribed so far, in the order the handler asked for
        /// them.
        pub(crate) fn subscribed(&mut self) -> Vec<String> {
            drain(&mut self.subscribed)
        }

        /// Every topic unsubscribed from so far. Drains, so a caller polling
        /// for one has to accumulate what it takes.
        pub(crate) fn unsubscribed(&mut self) -> Vec<String> {
            drain(&mut self.unsubscribed)
        }

        /// Every topic a governance broadcast reached. The mesh-count probe counts,
        /// so an op that only got as far as trying still shows up.
        pub(crate) fn broadcast_topics(&mut self) -> Vec<String> {
            drain(&mut self.broadcast)
        }
    }

    /// Everything a recorder holds, in the order it arrived.
    fn drain(rx: &mut UnboundedReceiver<String>) -> Vec<String> {
        let mut topics = Vec::new();
        while let Ok(topic) = rx.try_recv() {
            topics.push(topic);
        }
        topics
    }

    /// Start a manager over `store`, with no peer answering join requests.
    pub(crate) async fn over(store: Store) -> Harness {
        over_answering_joins(store, None).await
    }

    /// [`over`], with a peer that answers every namespace-join request with
    /// `bundle`.
    ///
    /// Needed by anything that asserts on what a *successful* join wrote: with
    /// no responder the join cannot obtain an admitter's endorsement, and a
    /// join without one is refused rather than recorded locally.
    pub(crate) async fn over_answering_joins(
        store: Store,
        bundle: Option<calimero_node_primitives::join_bundle::JoinBundle>,
    ) -> Harness {
        let (subscribed_tx, subscribed) = unbounded_channel();
        let (unsubscribed_tx, unsubscribed) = unbounded_channel();
        let (broadcast_tx, broadcast) = unbounded_channel();
        let network = LazyRecipient::<NetworkMessage>::new();
        let recipient = network.clone();
        let stub = StubNetwork::create(move |ctx| {
            assert!(recipient.init(ctx), "network recipient init");
            StubNetwork {
                subscribed: subscribed_tx,
                unsubscribed: unsubscribed_tx,
                broadcast: broadcast_tx,
            }
        });

        let (node_client, data_dir, blob_dir) = match bundle {
            Some(bundle) => {
                calimero_node_primitives::test_fixtures::node_client_over_answering_joins(
                    store.clone(),
                    NetworkClient::new(network),
                    bundle,
                )
                .await
            }
            None => node_client_over(store.clone(), NetworkClient::new(network)).await,
        };

        // Wired rather than left unbound: a handler that routes back through the
        // client (the join path applies its catch-up ops that way) would
        // otherwise queue against nobody.
        let context = LazyRecipient::new();
        let recipient = context.clone();
        let context_client = ContextClient::new(store.clone(), node_client.clone(), context);
        let manager = ContextManager::new(store, node_client.clone(), context_client.clone(), None);
        let manager = ContextManager::create(move |ctx| {
            assert!(recipient.init(ctx), "context recipient init");
            manager
        });

        Harness {
            manager,
            node_client,
            context_client,
            subscribed,
            unsubscribed,
            broadcast,
            _dirs: (data_dir, blob_dir),
            _network: stub,
        }
    }
}
