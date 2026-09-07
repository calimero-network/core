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
        .apply_link(namespace, &genesis, &[], &cert)
        .expect("record the binding");
    account
}

/// A second device of this node's account, certified by its root exactly as
/// `pair_device_complete` would, and scoped to `applications` (empty is every
/// application).
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
    devices
        .remember_device_cert(&proof, applications)
        .expect("remember the device");
    device
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
    let calimero_context_client::local_governance::NamespaceOp::RootSealed { key_id, encrypted } =
        &signed.op
    else {
        return None;
    };
    let key = calimero_governance_store::GroupKeyring::new(store, *namespace)
        .load_key_by_id(key_id.as_bytes())
        .expect("read the namespace keyring")
        .expect("the fixture kept the key it sealed under");
    Some(
        calimero_governance_store::GroupKeyring::decrypt_root_op(&key, encrypted)
            .expect("open a root op this fixture sealed"),
    )
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
    use calimero_node_primitives::test_fixtures::node_client_over;
    use calimero_store::Store;
    use calimero_utils_actix::LazyRecipient;
    use tempfile::TempDir;
    use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};

    use crate::ContextManager;

    /// Answers the three commands the pairing and governance paths issue, and
    /// records the topics. Any other command is dropped, which fails the
    /// caller's `rx.await` rather than hanging it: add the variant when a path
    /// under test starts issuing one.
    struct StubNetwork {
        subscribed: UnboundedSender<String>,
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
                NetworkMessage::MeshPeerCount { outcome, .. } => {
                    let _ignored = outcome.send(0);
                }
                NetworkMessage::Publish { outcome, .. } => {
                    let _ignored = outcome.send(Ok(MessageId(b"stub".to_vec())));
                }
                _ => {}
            }
        }
    }

    /// A started `ContextManager` and the store it reads. Seed the store, send
    /// the request, then assert on the rows the handler wrote.
    pub(crate) struct Harness {
        pub manager: Addr<ContextManager>,
        subscribed: UnboundedReceiver<String>,
        // The blob filesystem and the node's data root outlive the manager.
        _dirs: (TempDir, TempDir),
        _network: Addr<StubNetwork>,
    }

    impl Harness {
        /// Every topic subscribed so far, in the order the handler asked for
        /// them.
        pub(crate) fn subscribed(&mut self) -> Vec<String> {
            let mut topics = Vec::new();
            while let Ok(topic) = self.subscribed.try_recv() {
                topics.push(topic);
            }
            topics
        }
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
        let network = LazyRecipient::<NetworkMessage>::new();
        let recipient = network.clone();
        let stub = StubNetwork::create(move |ctx| {
            assert!(recipient.init(ctx), "network recipient init");
            StubNetwork {
                subscribed: subscribed_tx,
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
        let manager = ContextManager::new(store, node_client, context_client, None);
        let manager = ContextManager::create(move |ctx| {
            assert!(recipient.init(ctx), "context recipient init");
            manager
        });

        Harness {
            manager,
            subscribed,
            _dirs: (data_dir, blob_dir),
            _network: stub,
        }
    }
}
