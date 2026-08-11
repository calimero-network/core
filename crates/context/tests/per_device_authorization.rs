//! A paired device may author, and a revoked one may not.
//!
//! Without this, the account feature fails at its whole point: a second device
//! can be handed scope keys and then have every op it writes refused. Its
//! signing key is its own namespace identity, which is a member of nothing —
//! the only thing that entitles it is the account its certificate binds it to.
//!
//! The two halves of the resolution are deliberately different in kind, and this
//! pins both. *Which account does this key speak for* is read from the
//! materialized account rows, because account ops do not reach the fold on the
//! governance bridge. *May that account write here* is resolved against the
//! folded view at the op's cut, like every other authority question — so
//! removing the endorser at the cut takes the device's ops with it.

use std::sync::Arc;

use calimero_account::{sign_device_cert, AccountGenesis, DeviceId, KemPublicKey};
use calimero_context::scope_projection::{op_from_namespace_op, ScopeProjections};
use calimero_context_client::local_governance::{
    EncryptedGroupOp, GroupOp, NamespaceOp, SignedNamespaceOp,
};
use calimero_context_config::types::ContextGroupId;
use calimero_crypto::X25519SecretKey;
use calimero_governance_store::AccountBindingRepository;
use calimero_governance_store::{GroupKeyring, MembershipRepository, MetaRepository};
use calimero_primitives::context::GroupMemberRole;
use calimero_primitives::identity::{PrivateKey, PublicKey};
use calimero_storage::logical_clock::{HybridTimestamp, Timestamp, ID, NTP64};
use calimero_store::db::InMemoryDB;
use calimero_store::key::GroupMetaValue;
use calimero_store::Store;
use core::num::NonZeroU128;
use rand::rngs::OsRng;

fn store() -> Store {
    Store::new(Arc::new(InMemoryDB::owned()))
}

fn hlc(ns: u64) -> HybridTimestamp {
    HybridTimestamp::new(Timestamp::new(
        NTP64(ns),
        ID::from(NonZeroU128::new(1).unwrap()),
    ))
}

fn meta(admin: PublicKey) -> GroupMetaValue {
    GroupMetaValue {
        app_key: [0xBB; 32],
        target_application_id: calimero_primitives::application::ApplicationId::from([0xCC; 32]),
        created_at: 1_700_000_000,
        admin_identity: admin,
        owner_identity: admin,
        migration: None,
        auto_join: true,
    }
}

/// A namespace whose `member` was added by an encrypted group op, folded into a
/// projection. Returns the projection, the cut, and the store.
fn namespace_with_member(member: PublicKey) -> (Store, ScopeProjections, ContextGroupId, [u8; 32]) {
    let store = store();
    let admin = PrivateKey::random(&mut OsRng).public_key();
    let ns = ContextGroupId::from([0x11; 32]);
    let ns_bytes = ns.to_bytes();

    MetaRepository::new(&store).save(&ns, &meta(admin)).unwrap();
    MembershipRepository::new(&store)
        .add_member(&ns, &admin, GroupMemberRole::Admin)
        .unwrap();
    let group_key = [0x5A; 32];
    let key_id = GroupKeyring::new(&store, ns).store_key(&group_key).unwrap();

    let inner = GroupOp::MemberAdded {
        member,
        role: GroupMemberRole::Member,
    };
    let encrypted: EncryptedGroupOp = GroupKeyring::encrypt_op(&group_key, &inner).unwrap();
    let signed = SignedNamespaceOp {
        version: 1,
        namespace_id: ns_bytes.into(),
        parent_op_hashes: Vec::new(),
        signer: admin,
        nonce: 1,
        op: NamespaceOp::Group {
            group_id: ns_bytes.into(),
            key_id: key_id.into(),
            encrypted,
            key_rotation: None,
        },
        signature: [0u8; 64],
    };
    let delta_id = signed.content_hash().unwrap();

    let mut proj = ScopeProjections::new();
    proj.ingest_op(&op_from_namespace_op(
        &signed,
        Some(&inner),
        delta_id,
        hlc(1),
        &[],
    ));

    (store, proj, ns, delta_id)
}

/// Link a device for an account rooted at a dedicated offline root, vouched for
/// by `endorser` — the shape production produces.
fn link_device(
    store: &Store,
    ns: ContextGroupId,
    endorser: &PublicKey,
    device_sign_pk: &PublicKey,
) -> DeviceId {
    let account_root = PrivateKey::from([0x42; 32]);
    let genesis = AccountGenesis::new(account_root.public_key(), [0xAB; 16]);
    let account = genesis.account_id();
    let device = DeviceId::mint(account, [0xAB; 16]);
    let kem_secret = X25519SecretKey::from([0x33; 32]);
    let cert = sign_device_cert(
        &account_root,
        account,
        device,
        device_sign_pk,
        &KemPublicKey::from(*kem_secret.public_key().as_bytes()),
        0,
        0,
    )
    .unwrap();

    let bindings = AccountBindingRepository::new(store);
    bindings.record_endorser(&ns, account, endorser).unwrap();
    bindings
        .apply_link(&ns, &genesis, &[], &cert)
        .unwrap()
        .expect("admitted");
    device
}

#[test]
fn a_paired_device_may_author_for_the_account_that_certified_it() {
    let member = PrivateKey::random(&mut OsRng).public_key();
    let (store, proj, ns, delta_id) = namespace_with_member(member);
    let heads = [delta_id];

    // The device signs with its own namespace identity, minted on its own node.
    // It is a member of nothing.
    let device_sign_pk = PrivateKey::random(&mut OsRng).public_key();

    assert_eq!(
        proj.member_at_cut(&store, ns, &member, &heads),
        Some(true),
        "sanity: the endorsing member must be a member at this cut"
    );
    assert_eq!(
        proj.member_at_cut(&store, ns, &device_sign_pk, &heads),
        Some(false),
        "an unlinked key must not author — the grant has to come from the link"
    );

    let _device = link_device(&store, ns, &member, &device_sign_pk);

    assert_eq!(
        proj.member_at_cut(&store, ns, &device_sign_pk, &heads),
        Some(true),
        "a live device of an endorsed account must be able to author"
    );
}

#[test]
fn revoking_a_device_withdraws_its_right_to_author() {
    // Revocation has to cut authorship, not only key delivery. A revoked device's
    // node still holds the member key and is still in the namespace, so if the
    // resolver kept granting on the binding the device would keep writing.
    let member = PrivateKey::random(&mut OsRng).public_key();
    let (store, proj, ns, delta_id) = namespace_with_member(member);
    let heads = [delta_id];
    let device_sign_pk = PrivateKey::random(&mut OsRng).public_key();

    let device = link_device(&store, ns, &member, &device_sign_pk);
    assert_eq!(
        proj.member_at_cut(&store, ns, &device_sign_pk, &heads),
        Some(true),
        "precondition: the device authors before it is revoked"
    );

    AccountBindingRepository::new(&store)
        .apply_revocation(&ns, device)
        .unwrap();

    assert_eq!(
        proj.member_at_cut(&store, ns, &device_sign_pk, &heads),
        Some(false),
        "a revoked device must lose the right to author, not merely stop receiving keys"
    );
}

#[test]
fn a_device_whose_endorser_is_not_a_member_may_not_author() {
    // The authority half. The device→account mapping is materialized, but the
    // account's entitlement is resolved at the cut — so a vouch from someone who
    // is not a member at that cut grants nothing, and a device cannot be smuggled
    // in by endorsing its account with an unrelated key.
    let member = PrivateKey::random(&mut OsRng).public_key();
    let (store, proj, ns, delta_id) = namespace_with_member(member);
    let heads = [delta_id];
    let device_sign_pk = PrivateKey::random(&mut OsRng).public_key();

    let stranger = PrivateKey::random(&mut OsRng).public_key();
    let _device = link_device(&store, ns, &stranger, &device_sign_pk);

    assert_eq!(
        proj.member_at_cut(&store, ns, &device_sign_pk, &heads),
        Some(false),
        "an endorsement from a non-member must not confer authorship"
    );
}

/// The divergence the acceptance scenario hit: node-2 publishes a SECOND device
/// link and records it; node-1 refuses the same op with "account is not a member
/// of this group" and the two `scope_root`s part company for good.
///
/// The gate that refused is `endorser_is_member`, resolved against the projection
/// at the op's cut — and the only structural difference between the link that was
/// accepted and the one that was refused is that the second one's cut CONTAINS the
/// first link. So the question this pins is exactly that: does a cut whose
/// ancestry includes a `DeviceLinked` op still resolve an ordinary member?
#[test]
fn a_cut_containing_a_device_link_still_resolves_an_ordinary_member() {
    let member_sk = PrivateKey::random(&mut OsRng);
    let member = member_sk.public_key();
    let (store, mut proj, ns, member_cut) = namespace_with_member(member);

    // Baseline: at the cut that added them, the member is a member.
    assert_eq!(
        proj.member_at_cut(&store, ns, &member, &[member_cut]),
        Some(true),
        "baseline: the member must resolve at the cut that added them"
    );

    // Fold a device link on top, exactly as the receive path does.
    let account_root = PrivateKey::from([0x42; 32]);
    let genesis = AccountGenesis::new(account_root.public_key(), [0xAB; 16]);
    let account = genesis.account_id();
    let device_sign_pk = PrivateKey::from([0x77; 32]).public_key();
    let cert = sign_device_cert(
        &account_root,
        account,
        DeviceId::mint(account, [0xAB; 16]),
        &device_sign_pk,
        &KemPublicKey::from(*X25519SecretKey::from([0x33; 32]).public_key().as_bytes()),
        0,
        0,
    )
    .unwrap();
    let link = GroupOp::AccountDeviceLinked {
        genesis,
        chain: vec![],
        cert,
        endorsement: calimero_account::sign_account_endorsement(&member_sk, account).unwrap(),
    };
    let ns_bytes = ns.to_bytes();
    let group_key = [0x5A; 32];
    let encrypted: EncryptedGroupOp = GroupKeyring::encrypt_op(&group_key, &link).unwrap();
    let signed = SignedNamespaceOp {
        version: 1,
        namespace_id: ns_bytes.into(),
        parent_op_hashes: vec![member_cut],
        signer: member,
        nonce: 2,
        op: NamespaceOp::Group {
            group_id: ns_bytes.into(),
            key_id: GroupKeyring::new(&store, ns)
                .load_current_key()
                .unwrap()
                .expect("the fixture stored a key")
                .0
                .into(),
            encrypted,
            key_rotation: None,
        },
        signature: [0u8; 64],
    };
    let link_cut = signed.content_hash().unwrap();
    proj.ingest_op(&op_from_namespace_op(
        &signed,
        Some(&link),
        link_cut,
        hlc(2),
        &[member_cut],
    ));

    // THE assertion. A second link cites this cut, and its endorser gate asks
    // exactly this question. If the answer is not `Some(true)`, the publisher
    // (which answers from live) and every receiver (which answers from the
    // projection) decide the same op differently — a `scope_root` split with no
    // later op able to reconcile it.
    assert_eq!(
        proj.member_at_cut(&store, ns, &member, &[link_cut]),
        Some(true),
        "an ordinary member must still resolve at a cut whose ancestry contains a \
         device link — otherwise the second link is refused by receivers and \
         accepted by its publisher"
    );
}

/// The production shape, and the divergence the acceptance scenario hit.
///
/// `account create` records the device's `sign_pk` as the node's NAMESPACE
/// IDENTITY — the very key the group's membership row is keyed under. So a member
/// who enrols a device becomes, at every cut that contains its own link, a key that
/// `account_for_author` resolves to the account's real `AccountId` — while
/// membership on this plane is keyed by `legacy_account_id`. The member disappears.
///
/// The blast radius is every at-cut authority read for that key, not just the
/// device-link gate: the endorser check refuses their next link, and the cross-DAG
/// check refuses their devices' state deltas. Worse, the publisher decides its own
/// op from LIVE state and accepts, so receivers refuse an op the publisher recorded
/// and `scope_root` parts company with no later op able to reconcile it.
///
/// The device binding is a FALLTHROUGH for keys that are members of nothing — never
/// an override for a key that is a member in its own right.
#[test]
fn a_member_who_enrols_a_device_is_still_a_member_at_later_cuts() {
    let member_sk = PrivateKey::random(&mut OsRng);
    let member = member_sk.public_key();
    let (store, mut proj, ns, member_cut) = namespace_with_member(member);

    assert_eq!(
        proj.member_at_cut(&store, ns, &member, &[member_cut]),
        Some(true),
        "baseline: a member resolves at the cut that added them"
    );

    // `account create`: the member enrols its own device, and the certificate names
    // the member's own namespace identity as the device's signing key.
    let account_root = PrivateKey::from([0x42; 32]);
    let genesis = AccountGenesis::new(account_root.public_key(), [0xAB; 16]);
    let account = genesis.account_id();
    let cert = sign_device_cert(
        &account_root,
        account,
        DeviceId::mint(account, [0xAB; 16]),
        &member,
        &KemPublicKey::from(*X25519SecretKey::from([0x33; 32]).public_key().as_bytes()),
        0,
        0,
    )
    .unwrap();
    let link = GroupOp::AccountDeviceLinked {
        genesis,
        chain: vec![],
        cert,
        endorsement: calimero_account::sign_account_endorsement(&member_sk, account).unwrap(),
    };
    let ns_bytes = ns.to_bytes();
    let group_key = [0x5A; 32];
    let signed = SignedNamespaceOp {
        version: 1,
        namespace_id: ns_bytes.into(),
        parent_op_hashes: vec![member_cut],
        signer: member,
        nonce: 2,
        op: NamespaceOp::Group {
            group_id: ns_bytes.into(),
            key_id: GroupKeyring::new(&store, ns)
                .load_current_key()
                .unwrap()
                .expect("the fixture stored a key")
                .0
                .into(),
            encrypted: GroupKeyring::encrypt_op(&group_key, &link).unwrap(),
            key_rotation: None,
        },
        signature: [0u8; 64],
    };
    let link_cut = signed.content_hash().unwrap();
    proj.ingest_op(&op_from_namespace_op(
        &signed,
        Some(&link),
        link_cut,
        hlc(2),
        &[member_cut],
    ));

    assert_eq!(
        proj.member_at_cut(&store, ns, &member, &[link_cut]),
        Some(true),
        "a member must not stop being a member because they enrolled a device — the \
         binding is a fallthrough for keys that are members of nothing, not an \
         override for one that is a member in its own right"
    );
}

/// The join's two planes must resolve a joiner's key to the SAME account.
///
/// A node's writer principal comes from the materialized binding rows
/// (`env::account_id()` → `account_for_group` → `binding_for_sign_pk`), while the
/// peer verifying that node's signature resolves it from the FOLDED projection
/// (`device_account_at_cut` → `AclView::devices`). Both are fed by a join, and
/// they have to agree: a writer set the joiner seeds names whatever the first
/// answers, and every peer matches signatures against whatever the second does.
///
/// Recording the binding at apply time without folding the credential broke
/// exactly this. The joiner wrote as its real account while every peer resolved
/// it to `legacy_account_id`, so the joiner's `Shared` writes matched no grant —
/// which surfaces far from the join, as data that silently never converges.
#[test]
fn a_joiners_writer_account_matches_what_its_peers_resolve() {
    use calimero_governance_store::{NamespaceGovernance, NamespaceRepository};

    let store = store();
    let mut rng = OsRng;
    let admin_sk = PrivateKey::random(&mut rng);
    let admin = admin_sk.public_key();
    let joiner_sk = PrivateKey::random(&mut rng);
    let joiner = joiner_sk.public_key();

    let ns_bytes = [0x5C; 32];
    let ns = ContextGroupId::from(ns_bytes);

    MetaRepository::new(&store).save(&ns, &meta(admin)).unwrap();
    MembershipRepository::new(&store)
        .add_member(&ns, &admin, GroupMemberRole::Admin)
        .unwrap();
    NamespaceRepository::new(&store)
        .store_identity(&ns, &admin, &[0x11; 32], &[0u8; 32])
        .unwrap();

    // A credential the joiner can actually present: certified by its own account
    // root, naming the joiner's namespace identity as the device's `sign_pk`.
    let account_root = PrivateKey::from([0x77; 32]);
    let genesis = AccountGenesis::new(account_root.public_key(), [0xCD; 16]);
    let real_account = genesis.account_id();
    let kem_secret = X25519SecretKey::from([0x34; 32]);
    let cert = sign_device_cert(
        &account_root,
        real_account,
        DeviceId::mint(real_account, [0xCD; 16]),
        &joiner,
        &KemPublicKey::from(*kem_secret.public_key().as_bytes()),
        0,
        0,
    )
    .unwrap();
    let credential = Box::new(
        calimero_context_client::local_governance::JoinAccountCredential {
            genesis,
            chain: vec![],
            cert,
        },
    );

    // 0 is the canonical "no expiry" sentinel: `MemberJoined` carries no
    // `joined_at`, so a non-zero expiration is rejected by the apply gate.
    let invitation_body = calimero_context_config::types::GroupInvitationFromAdmin {
        inviter_identity: calimero_context_config::types::SignerId::from(*admin.digest()),
        group_id: ns,
        expiration_timestamp: 0,
        invitation_nonce: [0x21; 32],
        invited_role: 1,
    };
    let inv_sig = admin_sk
        .sign(&<sha2::Sha256 as sha2::Digest>::digest(
            borsh::to_vec(&invitation_body).expect("borsh invitation"),
        ))
        .expect("admin signs the invitation");
    let invitation = calimero_context_config::types::SignedGroupOpenInvitation {
        invitation: invitation_body,
        inviter_signature: hex::encode(inv_sig.to_bytes()),
        application_id: None,
        app_key: None,
    };

    let gov = NamespaceGovernance::new(&store, ns_bytes.into());
    let head = gov.read_head_record().expect("read head");
    let join = SignedNamespaceOp::sign(
        &joiner_sk,
        ns_bytes.into(),
        head.parent_hashes.clone(),
        head.next_nonce,
        NamespaceOp::Root(
            calimero_context_client::local_governance::RootOp::MemberJoined {
                member: joiner,
                signed_invitation: invitation,
                account: credential,
            },
        ),
    )
    .expect("joiner signs its join");
    gov.apply_signed_op(&join).expect("the join applies");

    // Plane one: what the joiner itself writes as.
    let binding = AccountBindingRepository::new(&store)
        .binding_for_sign_pk(&ns, &joiner)
        .expect("read bindings")
        .map(|b| b.account);
    let writes_as = calimero_op_adapter::writer_account(binding, &joiner);
    assert_eq!(
        writes_as, real_account,
        "a join binds the device, so the joiner writes as its real account"
    );

    // Plane two: what a peer resolves that same key to, from the folded log.
    let delta_id = join.content_hash().unwrap();
    let mut proj = ScopeProjections::new();
    proj.ingest_op(&op_from_namespace_op(&join, None, delta_id, hlc(1), &[]));
    let peers_resolve = proj
        .device_account_at_cut(&store, ns, &joiner, &[delta_id])
        .expect("the cut is fully folded, so this is a settled answer");

    assert_eq!(
        peers_resolve, writes_as,
        "the writer plane and the projection must name the same principal for one \
         key, or every write the joiner makes is refused by every peer"
    );
}
