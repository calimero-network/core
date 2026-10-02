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
/// Not [`enrol`]: that derives a stand-in account from the key, so every gate
/// asking "does this signer speak for this account" then refuses.
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

/// The root-signed scope a registry row - and every link made under it - carries.
/// `root_sk` is the root that signed `cert`, so it names the same genesis.
///
/// # Panics
///
/// Panics if the root refuses to sign, which in a test means the fixture is wrong.
#[must_use]
pub fn device_scope(
    root_sk: &PrivateKey,
    cert: &calimero_account::DeviceCert,
    applications: &[calimero_primitives::application::ApplicationId],
    scope_epoch: u32,
) -> calimero_account::AccountProof<calimero_account::DeviceScope> {
    calimero_account::AccountProof {
        genesis: calimero_account::AccountGenesis::new(root_sk.public_key()),
        chain: vec![],
        statement: calimero_account::DeviceScope::sign(
            root_sk,
            cert.account,
            cert.device,
            applications.to_vec(),
            scope_epoch,
            0,
        )
        .expect("the account root signs the device's scope"),
    }
}

/// This node as a DEVICE of an account whose root lives elsewhere: the state
/// pairing leaves behind, scoped to `applications`.
///
/// A device and not a holder on purpose - `account_namespace` answers a holder
/// from its root derivation, which is the wrong read. The root comes back beside
/// the device because it lives nowhere in this store.
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
            .record(
                &proof,
                &device_scope(&root_sk, &proof.statement, applications, 0),
            )
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
            &device_scope(root.signing_key(), &proof.statement, applications, 0),
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
            &device_scope(root_sk, &proof.statement, applications, scope_epoch),
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

/// A namespace whose root owns one context and whose members all joined before any rotation,
/// for tests of who may write a `SharedStorage` cell at a governance cut.
///
/// Ops are folded into [`Self::projections`] as the node's fold would, so an at-cut read
/// through it answers as production does; the world only builds what a rotation rests on
/// (members in standing) and publishes rotations signed by them.
pub struct RotationWorld {
    /// The store the group, the context and the governance heads live in.
    pub store: Store,
    /// The projections a node would hold, fed by the ops this world builds.
    pub projections: std::sync::Arc<std::sync::RwLock<crate::scope_projection::ScopeProjections>>,
    /// The namespace's root group, which owns the context.
    pub group: ContextGroupId,
    /// The context registered in the group.
    pub context: calimero_primitives::context::ContextId,
    namespace: [u8; 32],
    accounts: std::collections::BTreeMap<PublicKey, AccountId>,
    joined: Vec<[u8; 32]>,
}

impl RotationWorld {
    /// A world in which each of `members` joined the root group as a member.
    ///
    /// # Panics
    ///
    /// Panics if the context cannot be registered, which in a test means the fixture is wrong.
    #[must_use]
    pub fn new(members: &[PublicKey]) -> Self {
        Self::for_context(
            calimero_primitives::context::ContextId::from([0x44; 32]),
            members,
        )
    }

    /// [`Self::new`], with the context the rotations name being `context`, for a test whose
    /// node holds that context.
    ///
    /// # Panics
    ///
    /// Panics if the context cannot be registered, which in a test means the fixture is wrong.
    #[must_use]
    pub fn for_context(
        context: calimero_primitives::context::ContextId,
        members: &[PublicKey],
    ) -> Self {
        use calimero_context_client::local_governance::{NamespaceOp, RootOp, SignedNamespaceOp};
        use calimero_context_config::types::{GroupInvitationFromAdmin, SignedGroupOpenInvitation};
        use calimero_storage::logical_clock::{HybridTimestamp, Timestamp, ID, NTP64};

        let namespace = [0x81; 32];
        let group = ContextGroupId::from(namespace);
        let store = Store::new(std::sync::Arc::new(calimero_store::db::InMemoryDB::owned()));
        calimero_governance_store::register_context_in_group(&store, &group, &context)
            .expect("register the context in the root group");
        let mut projections = crate::scope_projection::ScopeProjections::new();
        let mut accounts = std::collections::BTreeMap::new();
        let mut joined = Vec::new();
        for (n, key) in members.iter().enumerate() {
            let credential = credential(key);
            let account = credential.statement.account;
            let join = RootOp::MemberJoined {
                member: account,
                signed_invitation: SignedGroupOpenInvitation {
                    inviter_account: None,
                    invitation: GroupInvitationFromAdmin {
                        inviter_identity: [0xA1; 32].into(),
                        group_id: group,
                        expiration_timestamp: 1_700_000_000,
                        invitation_nonce: [1; 32],
                        invited_role: 1,
                        admitters: Vec::new(),
                    },
                    inviter_signature: "deadbeef".to_owned(),
                    application_id: None,
                    bytecode_id: None,
                    admitter_addrs: Vec::new(),
                },
                account: credential,
            };
            let signed = SignedNamespaceOp {
                version: 1,
                namespace_id: namespace.into(),
                parent_op_hashes: Vec::new(),
                signer: *key,
                nonce: 0,
                op: NamespaceOp::Root(join),
                signature: [0u8; 64],
                admitter_endorsement: None,
            };
            // Not a repeated byte, so a test's own op ids never collide with a join's.
            let mut id = [0xE0; 32];
            id[31] = u8::try_from(n).expect("few members");
            let clock = HybridTimestamp::new(Timestamp::new(
                NTP64(0),
                ID::from(core::num::NonZeroU64::MIN),
            ));
            projections.ingest_op(
                &calimero_governance_store::unified_op_decode::op_from_namespace_op(
                    &signed,
                    None,
                    id,
                    clock,
                    &[],
                ),
            );
            let _previous = accounts.insert(*key, account);
            joined.push(id);
        }
        let world = Self {
            store,
            projections: std::sync::Arc::new(std::sync::RwLock::new(projections)),
            group,
            context,
            namespace,
            accounts,
            joined,
        };
        world.set_current_heads(&world.joined.clone());
        world
    }

    /// The account `member` speaks for.
    ///
    /// # Panics
    ///
    /// Panics if `member` is not one of the world's members.
    #[must_use]
    pub fn account(&self, member: &PublicKey) -> AccountId {
        self.accounts[member]
    }

    /// The cut at which every member has joined and nothing has rotated.
    #[must_use]
    pub fn joined(&self) -> Vec<[u8; 32]> {
        self.joined.clone()
    }

    /// Record `heads` as the governance heads this node holds now.
    ///
    /// # Panics
    ///
    /// Panics if the store refuses the write.
    pub fn set_current_heads(&self, heads: &[[u8; 32]]) {
        self.store
            .handle()
            .put(
                &calimero_store::key::NamespaceGovHead::new(self.namespace),
                &calimero_store::key::NamespaceGovHeadValue {
                    sequence: 1,
                    dag_heads: heads.to_vec(),
                },
            )
            .expect("record the namespace heads");
    }

    /// `signer` rotating `cell` from `prior` to `new` in the root group, as op `id` on `parents`.
    /// The step counts only if `prior` is the set the cell id binds (or the one in effect)
    /// and gives `signer`'s account `ADMIN`.
    ///
    /// # Panics
    ///
    /// Panics if `signer` is not one of the world's members.
    #[expect(clippy::too_many_arguments, reason = "a step is all of these")]
    pub fn rotate(
        &self,
        signer: &PublicKey,
        cell: calimero_storage::address::Id,
        id: [u8; 32],
        parents: &[[u8; 32]],
        prior: calimero_storage::shared_writers::Writers,
        nonce: u64,
        new: calimero_storage::shared_writers::Writers,
    ) {
        use calimero_context_client::local_governance::{GroupOp, NamespaceOp, SignedNamespaceOp};
        use calimero_storage::logical_clock::{HybridTimestamp, Timestamp, ID, NTP64};

        let signed = SignedNamespaceOp {
            version: 1,
            namespace_id: self.namespace.into(),
            parent_op_hashes: Vec::new(),
            signer: *signer,
            nonce: 0,
            op: NamespaceOp::Group {
                group_id: self.group.to_bytes().into(),
                key_id: [0u8; 32].into(),
                encrypted: calimero_governance_types::EncryptedGroupOp {
                    nonce: [0u8; 12],
                    ciphertext: Vec::new(),
                },
                key_rotation: None,
            },
            signature: [0u8; 64],
            admitter_endorsement: None,
        };
        let clock = HybridTimestamp::new(Timestamp::new(
            NTP64(0),
            ID::from(core::num::NonZeroU64::MIN),
        ));
        let op = calimero_governance_store::unified_op_decode::op_from_namespace_op_with_binding(
            &signed,
            Some(&GroupOp::SharedWritersRotated {
                context_id: self.context,
                cell,
                prior,
                nonce,
                new,
            }),
            None,
            Some((
                self.accounts[signer],
                calimero_account::DeviceId::from([0x3E; 32]),
            )),
            id,
            clock,
            parents,
        );
        self.projections
            .write()
            .expect("the projections lock is not poisoned")
            .ingest_op(&op);
    }
}

#[cfg(test)]
mod rotation_world_tests {
    use calimero_storage::entities::full_mask;
    use calimero_storage::shared_writers::CellWriters;

    use super::*;

    #[test]
    fn a_members_rotation_takes_effect_at_its_cut_and_not_before() {
        let (alice, bob) = (PublicKey::from([1; 32]), PublicKey::from([2; 32]));
        let world = RotationWorld::new(&[alice, bob]);
        let (a, b) = (world.account(&alice), world.account(&bob));
        let genesis = full_mask([a, b].into_iter().collect());
        let rotated = full_mask([a].into_iter().collect());
        let cell = calimero_storage::collections::cell_id(
            calimero_storage::address::Id::new([0x40; 32]),
            &genesis,
        );
        world.rotate(
            &alice,
            cell,
            [0xD1; 32],
            &world.joined(),
            genesis,
            1,
            rotated.clone(),
        );

        let at = |heads: &[[u8; 32]]| {
            world.projections.read().unwrap().shared_writers_at_cut(
                &world.store,
                &world.context,
                cell,
                heads,
            )
        };
        assert_eq!(at(&world.joined()), Ok(CellWriters::Genesis));
        assert_eq!(at(&[[0xD1; 32]]), Ok(CellWriters::Rotated(rotated)));
    }

    #[test]
    fn the_nodes_current_heads_are_the_ones_last_recorded() {
        let world = RotationWorld::new(&[PublicKey::from([1; 32])]);
        let group = world.group;
        let heads = |store: &Store| {
            crate::scope_projection::ScopeProjections::namespace_current_heads(store, group)
        };
        assert_eq!(heads(&world.store), Some(world.joined()));
        world.set_current_heads(&[[0xD1; 32]]);
        assert_eq!(heads(&world.store), Some(vec![[0xD1; 32]]));
    }
}

/// `joiner_sk`'s node joins `ns` as a `Member` by an invitation `admin_sk`
/// signed, through the governance apply, and the account it joined as.
#[cfg(test)]
pub(crate) fn join_namespace(
    store: &Store,
    ns: &ContextGroupId,
    admin_sk: &PrivateKey,
    admin: AccountId,
    joiner_sk: &PrivateKey,
) -> AccountId {
    use calimero_context_client::local_governance::SignedNamespaceOp;
    use calimero_context_config::types::{
        GroupInvitationFromAdmin, SignedGroupOpenInvitation, SignerId,
    };
    use sha2::{Digest, Sha256};

    let nonce = [0x42; 32];
    let invitation = GroupInvitationFromAdmin {
        inviter_identity: SignerId::from(*admin_sk.public_key().digest()),
        group_id: *ns,
        expiration_timestamp: 0,
        invitation_nonce: nonce,
        invited_role: 1,
        admitters: vec![admin],
    };
    let signature = admin_sk
        .sign(&Sha256::digest(
            borsh::to_vec(&invitation).expect("encode invitation"),
        ))
        .expect("sign invitation");
    let account = crate::join_credential::build(store, ns, &joiner_sk.public_key())
        .expect("the joiner's credential");
    let member = account.statement.account;
    let join = calimero_context_client::local_governance::RootOp::MemberJoinedAt {
        member,
        signed_invitation: SignedGroupOpenInvitation {
            inviter_account: None,
            invitation,
            inviter_signature: hex::encode(signature.to_bytes()),
            application_id: None,
            bytecode_id: None,
            admitter_addrs: Vec::new(),
        },
        joined_at: 1,
        account,
    };
    let parents = calimero_governance_store::NamespaceDagService::new(store, ns.to_bytes().into())
        .read_head_record()
        .expect("read the governance head")
        .parent_hashes;
    let mut signed = SignedNamespaceOp::sign(
        joiner_sk,
        ns.to_bytes().into(),
        parents,
        1,
        published_join(store, ns, join),
    )
    .expect("sign the join");
    signed.admitter_endorsement = Some(Box::new(
        calimero_governance_types::AdmitterEndorsement::sign(
            admin_sk,
            &ns.to_bytes(),
            &member,
            &nonce,
        )
        .expect("endorse the join"),
    ));
    let _ = calimero_governance_store::apply_signed_namespace_op(store, &signed)
        .expect("the join applies");
    member
}

/// A namespace an account with no node joined through an admitter: the
/// [`relayed_join`] fixture's result.
pub struct RelayedJoin {
    /// The namespace root the account joined.
    pub namespace: ContextGroupId,
    /// The envelope the admitter published, as the namespace DAG holds it.
    pub envelope: calimero_context_client::local_governance::SignedNamespaceOp,
    /// The envelope's id: the governance head a delta written after the join cites.
    pub envelope_id: [u8; 32],
    /// The joiner's device key, the key its state deltas are signed with.
    pub device_key: PublicKey,
    /// The account the joiner's credential certifies.
    pub account: AccountId,
}

/// Apply a join that an admitter relayed for an account with no node.
///
/// This is the route `POST /admin-api/namespaces/:id/admit` takes: the joiner
/// signs its own `MemberJoinedAt`, and the admitter seals that signed op under
/// the namespace key and publishes it as `NamespaceOp::RootRelaySealed`, an
/// envelope it signs itself. So the envelope's signer is the admitter, and only
/// the op inside it names the joiner — which is what a fold has to read.
///
/// # Panics
///
/// Panics if any row cannot be written or the join does not apply, which in a
/// test means the fixture is wrong rather than the code under test.
#[must_use]
pub fn relayed_join(store: &Store) -> RelayedJoin {
    use calimero_context_client::local_governance::{NamespaceOp, RootOp, SignedNamespaceOp};
    use calimero_context_config::types::{
        GroupInvitationFromAdmin, SignedGroupOpenInvitation, SignerId,
    };
    use calimero_governance_store::{
        GroupKeyring, MembershipRepository, MetaRepository, NamespaceGovernance,
        NamespaceRepository,
    };
    use calimero_primitives::context::GroupMemberRole;

    let namespace_bytes = [0x6D; 32];
    let namespace = ContextGroupId::from(namespace_bytes);

    // The namespace admin, who is also the admitter publishing the join.
    let admin_sk = PrivateKey::from([0x61; 32]);
    let admin = admin_sk.public_key();
    let admin_account = enrol(store, &namespace, &admin);
    MetaRepository::new(store)
        .save(
            &namespace,
            &calimero_store::key::GroupMetaValue {
                target: calimero_store::key::GroupTarget {
                    application_id: calimero_primitives::application::ApplicationId::from(
                        [0xCC; 32],
                    ),
                    bytecode_id: [0xBB; 32],
                    package: Box::default(),
                    version: Box::default(),
                },
                created_at: 1_700_000_000,
                admin_identity: admin_account,
                owner_identity: admin_account,
                migration: None,
                auto_join: true,
            },
        )
        .expect("save the namespace meta");
    MembershipRepository::new(store)
        .add_member(&namespace, &admin_account, GroupMemberRole::Admin)
        .expect("seat the admin");
    NamespaceRepository::new(store)
        .store_identity(&namespace, &admin, &[0x62; 32])
        .expect("store the admin's namespace identity");
    // The key a relayed join is sealed under. Production keys a namespace at
    // creation.
    let _ = GroupKeyring::new(store, namespace)
        .store_key(&[0x63; 32])
        .expect("mint the namespace key");

    let invitation = |nonce: [u8; 32]| {
        let invitation = GroupInvitationFromAdmin {
            inviter_identity: SignerId::from(*admin.digest()),
            group_id: namespace,
            // 0 is the "no expiry" sentinel.
            expiration_timestamp: 0,
            invitation_nonce: nonce,
            invited_role: 1,
            admitters: vec![admin_account],
        };
        let inviter_signature = admin_sk
            .sign(&<sha2::Sha256 as sha2::Digest>::digest(
                borsh::to_vec(&invitation).expect("borsh the invitation"),
            ))
            .expect("the admin signs the invitation");
        SignedGroupOpenInvitation {
            inviter_account: None,
            invitation,
            inviter_signature: hex::encode(inviter_signature.to_bytes()),
            application_id: None,
            bytecode_id: None,
            admitter_addrs: Vec::new(),
        }
    };
    // A join signed by `joiner_sk`, endorsed by the admitter, for `nonce`.
    let signed_join =
        |joiner_sk: &PrivateKey, parents: Vec<[u8; 32]>, nonce: u64, invitation_nonce: [u8; 32]| {
            let account_credential = credential(&joiner_sk.public_key());
            let member = account_credential.statement.account;
            let mut join = SignedNamespaceOp::sign(
                joiner_sk,
                namespace_bytes.into(),
                parents,
                nonce,
                NamespaceOp::Root(RootOp::MemberJoinedAt {
                    member,
                    signed_invitation: invitation(invitation_nonce),
                    joined_at: 0,
                    account: account_credential,
                }),
            )
            .expect("the joiner signs its join");
            join.admitter_endorsement = Some(Box::new(
                calimero_governance_types::AdmitterEndorsement::sign(
                    &admin_sk,
                    &namespace_bytes,
                    &member,
                    &invitation_nonce,
                )
                .expect("the admitter endorses the join"),
            ));
            (join, member)
        };
    let governance = NamespaceGovernance::new(store, namespace_bytes.into());

    // A member that joined from a node of its own, first. Its membership is in
    // the fold, so the namespace is not one the projection has never seen a
    // member of — the state every real namespace with a peer is in, and the one
    // where the projection's verdict is the only one consulted.
    let head = governance.read_head_record().expect("read the head");
    let (peer_join, _) = signed_join(
        &PrivateKey::from([0x66; 32]),
        head.parent_hashes,
        head.next_nonce,
        [0x67; 32],
    );
    governance
        .apply_signed_op(&peer_join)
        .expect("the peer's own join applies");

    // The joiner: a device key and a credential for it, and no node. It signs
    // its own join; nonce 1 is the first a relay accepts.
    let joiner_sk = PrivateKey::from([0x64; 32]);
    let device_key = joiner_sk.public_key();
    let (inner, account) = signed_join(&joiner_sk, vec![], 1, [0x65; 32]);

    // The admitter seals the joiner's op and signs the envelope.
    let (key_id, key) = GroupKeyring::new(store, namespace)
        .load_current_key()
        .expect("read the namespace keyring")
        .expect("the namespace key was just minted");
    let sealed = NamespaceOp::RootRelaySealed {
        key_id: key_id.into(),
        encrypted: GroupKeyring::encrypt_relayed_op(&key, &inner).expect("seal the relay"),
    };
    let head = governance.read_head_record().expect("read the head");
    let envelope = SignedNamespaceOp::sign(
        &admin_sk,
        namespace_bytes.into(),
        head.parent_hashes,
        head.next_nonce,
        sealed,
    )
    .expect("the admitter signs the envelope");
    governance
        .apply_signed_op(&envelope)
        .expect("the relayed join applies");
    let envelope_id = envelope.content_hash().expect("hash the envelope");

    RelayedJoin {
        namespace,
        envelope,
        envelope_id,
        device_key,
        account,
    }
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
    use std::collections::BTreeSet;
    use std::sync::{Arc, Mutex};

    use actix::{Actor, Addr, AsyncContext, Context, Handler};
    use calimero_context_client::client::ContextClient;
    use calimero_network_primitives::client::NetworkClient;
    use calimero_network_primitives::messages::{MessageId, NetworkMessage};
    use calimero_node_primitives::client::NodeClient;
    use calimero_node_primitives::test_fixtures::node_client_over;
    use calimero_store::Store;
    use calimero_utils_actix::LazyRecipient;
    use tempfile::TempDir;
    use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};
    use tokio::sync::Semaphore;

    use crate::ContextManager;

    /// Answers the four commands the pairing and governance paths issue, and
    /// records the topics. Any other is dropped, which panics its caller.
    struct StubNetwork {
        subscribed: UnboundedSender<String>,
        unsubscribed: UnboundedSender<String>,
        broadcast: UnboundedSender<String>,
        /// The topics subscribed to and not since dropped, as the swarm would hold them.
        live: Arc<Mutex<BTreeSet<String>>>,
        /// A topic whose subscribe is not answered until the semaphore hands out a
        /// permit, so a test can hold a caller part-way through its work.
        held: Option<(String, Arc<Semaphore>)>,
    }

    impl Actor for StubNetwork {
        type Context = Context<Self>;
    }

    impl Handler<NetworkMessage> for StubNetwork {
        type Result = ();

        fn handle(&mut self, msg: NetworkMessage, ctx: &mut Self::Context) {
            match msg {
                NetworkMessage::Subscribe { request, outcome } => {
                    let topic = request.0.to_string();
                    let _ignored = self.subscribed.send(topic.clone());
                    let gate = self
                        .held
                        .as_ref()
                        .filter(|(held, _)| *held == topic)
                        .map(|(_, gate)| Arc::clone(gate));
                    let live = Arc::clone(&self.live);
                    let answer = move || {
                        let _ = live.lock().expect("live topics").insert(topic);
                        let _ignored = outcome.send(Ok(request.0));
                    };
                    match gate {
                        None => answer(),
                        Some(gate) => {
                            let held = async move {
                                gate.acquire()
                                    .await
                                    .expect("the gate is never closed")
                                    .forget();
                                answer();
                            };
                            let _handle = ctx.spawn(actix::fut::wrap_future(held));
                        }
                    }
                }
                NetworkMessage::Unsubscribe { request, outcome } => {
                    let topic = request.0.to_string();
                    let _ = self.live.lock().expect("live topics").remove(&topic);
                    let _ignored = self.unsubscribed.send(topic);
                    let _ignored = outcome.send(Ok(request.0));
                }
                NetworkMessage::MeshPeerCount { request, outcome } => {
                    let _ignored = self.broadcast.send(request.0.to_string());
                    let _ignored = outcome.send(0);
                }
                NetworkMessage::SubscribedPeers { outcome, .. } => {
                    let _ignored = outcome.send(Vec::new());
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
        live: Arc<Mutex<BTreeSet<String>>>,
        // The blob filesystem and the node's data root outlive the manager.
        _dirs: (TempDir, TempDir),
        _network: Addr<StubNetwork>,
    }

    impl Harness {
        /// Puts one op in the namespace's governance log, as the group's creation op would
        /// be, so a run's governance cut is not the empty one. `signer` must be a member.
        pub(crate) async fn seed_governance_log(
            &self,
            store: &Store,
            group: &calimero_context_config::types::ContextGroupId,
            signer: &calimero_primitives::identity::PrivateKey,
        ) {
            let report = calimero_governance_store::sign_apply_and_publish(
                store,
                &self.node_client,
                self.context_client.ack_router(),
                group,
                signer,
                calimero_context_client::local_governance::GroupOp::Noop,
            )
            .await
            .expect("the seed op publishes");
            assert!(report.is_some(), "the seed op reached the op log");
        }

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

        /// The topics subscribed to now: every subscribe answered, less every
        /// unsubscribe since. Unlike the recorders it does not drain.
        pub(crate) fn live_topics(&self) -> BTreeSet<String> {
            self.live.lock().expect("live topics").clone()
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

    /// [`over`], with every subscribe to `topic` held unanswered until the
    /// returned semaphore is given a permit for it. The request is still recorded
    /// as it arrives, so a test can wait for the caller to reach it.
    pub(crate) async fn over_holding_subscribe(
        store: Store,
        topic: String,
    ) -> (Harness, Arc<Semaphore>) {
        let gate = Arc::new(Semaphore::new(0));
        let harness = build(store, None, None, Some((topic, Arc::clone(&gate)))).await;
        (harness, gate)
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
        build(store, bundle, None, None).await
    }

    /// [`over`], with full-text search turned on.
    pub(crate) async fn over_with_search(
        store: Store,
        search: std::sync::Arc<calimero_search::SearchService>,
    ) -> Harness {
        build(store, None, Some(search), None).await
    }

    async fn build(
        store: Store,
        bundle: Option<calimero_node_primitives::join_bundle::JoinBundle>,
        search: Option<std::sync::Arc<calimero_search::SearchService>>,
        held: Option<(String, Arc<Semaphore>)>,
    ) -> Harness {
        let (subscribed_tx, subscribed) = unbounded_channel();
        let (unsubscribed_tx, unsubscribed) = unbounded_channel();
        let (broadcast_tx, broadcast) = unbounded_channel();
        let live = Arc::new(Mutex::new(BTreeSet::new()));
        let network = LazyRecipient::<NetworkMessage>::new();
        let recipient = network.clone();
        let stub_live = Arc::clone(&live);
        let stub = StubNetwork::create(move |ctx| {
            assert!(recipient.init(ctx), "network recipient init");
            StubNetwork {
                subscribed: subscribed_tx,
                unsubscribed: unsubscribed_tx,
                broadcast: broadcast_tx,
                live: stub_live,
                held,
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
        let manager = match search {
            Some(search) => manager.with_search(search),
            None => manager,
        };
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
            live,
            _dirs: (data_dir, blob_dir),
            _network: stub,
        }
    }
}
