//! Whose devices may act: an account's devices act only where the account itself
//! is a member, and stop acting where the namespace withdrew them.
//!
//! Two properties, pinned where they show:
//!
//! * A device is an author only through its own account. A member endorsing some
//!   other account's device is a statement about that other account, and it adds
//!   nothing to it: the account is not a member, so neither is its device.
//! * A device withdrawn in a namespace (revoked, or narrowed out of it) is
//!   withdrawn in every context of the namespace, subgroups included. The rows
//!   that record it are keyed by the namespace, so a check that looks them up
//!   under the context's own group never finds them.

use std::sync::Arc;

use calimero_account::{
    AccountGenesis, AccountId, AccountProof, Delegation, DeviceCert, DeviceId, KemPublicKey,
    Warrant, WarrantTerms,
};
use calimero_context::scope_projection::{op_from_namespace_op, ScopeProjections};
use calimero_context_client::local_governance::{
    EncryptedGroupOp, GroupOp, NamespaceOp, SignedNamespaceOp,
};
use calimero_context_config::types::ContextGroupId;
use calimero_context_config::MemberCapabilities;
use calimero_crypto::X25519SecretKey;
use calimero_governance_store::warrant_gate::{check_delegated_delta, WarrantRefusal};
use calimero_governance_store::{
    AccountBindingRepository, CapabilitiesRepository, GroupKeyring, MembershipRepository,
    MetaRepository, NamespaceRepository,
};
use calimero_primitives::application::ApplicationId;
use calimero_primitives::context::{ContextId, GroupMemberRole};
use calimero_primitives::identity::{PrivateKey, PublicKey};
use calimero_storage::logical_clock::{HybridTimestamp, Timestamp, ID, NTP64};
use calimero_store::db::InMemoryDB;
use calimero_store::key::{GroupMetaValue, GroupTarget};
use calimero_store::Store;
use core::num::NonZeroU128;
use rand::rand_core::UnwrapErr;
use rand::rngs::SysRng;

fn store() -> Store {
    Store::new(Arc::new(InMemoryDB::owned()))
}

fn hlc(ns: u64) -> HybridTimestamp {
    HybridTimestamp::new(Timestamp::new(
        NTP64(ns),
        ID::from(NonZeroU128::new(1).unwrap()),
    ))
}

fn meta(admin: AccountId) -> GroupMetaValue {
    GroupMetaValue {
        target: GroupTarget {
            application_id: ApplicationId::from([0xCC; 32]),
            bytecode_id: [0xBB; 32],
            package: Box::default(),
            version: Box::default(),
        },
        created_at: 1_700_000_000,
        admin_identity: admin,
        owner_identity: admin,
        migration: None,
        auto_join: true,
    }
}

/// A namespace whose `member` was added by an encrypted group op, folded into a
/// projection. Returns the store, the projection, the namespace and the cut.
fn namespace_with_member(member: PublicKey) -> (Store, ScopeProjections, ContextGroupId, [u8; 32]) {
    let store = store();
    let admin = PrivateKey::random(&mut UnwrapErr(SysRng)).public_key();
    let ns = ContextGroupId::from([0x11; 32]);
    let ns_bytes = ns.to_bytes();

    let admin_account = calimero_context::test_support::enrol(&store, &ns, &admin);
    MetaRepository::new(&store)
        .save(&ns, &meta(admin_account))
        .unwrap();
    MembershipRepository::new(&store)
        .add_member(&ns, &admin_account, GroupMemberRole::Admin)
        .unwrap();
    let group_key = [0x5A; 32];
    let key_id = GroupKeyring::new(&store, ns).store_key(&group_key).unwrap();

    let inner = GroupOp::MemberAdded {
        member: calimero_context::test_support::enrol(&store, &ns, &member),
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
        admitter_endorsement: None,
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

/// A device of an account rooted at an offline key that is NOT a member of the
/// namespace, bound as an apply of a link endorsed by `endorser` records it.
fn link_device_of_outside_account(
    store: &Store,
    ns: ContextGroupId,
    endorser: &PublicKey,
    device_sign_pk: &PublicKey,
) -> AccountId {
    let account_root = PrivateKey::from([0x42; 32]);
    let genesis = AccountGenesis::new(account_root.public_key());
    let account = genesis.account_id();
    let cert = DeviceCert::sign(
        &account_root,
        account,
        DeviceId::mint(account, [0xAB; 16]),
        device_sign_pk,
        &KemPublicKey::from(*X25519SecretKey::from([0x33; 32]).public_key().as_bytes()),
        0,
        0,
    )
    .unwrap();

    let bindings = AccountBindingRepository::new(store);
    bindings
        .record_endorser(
            &ns,
            account,
            &calimero_context::test_support::account_for(endorser),
        )
        .unwrap();
    bindings
        .apply_link(&ns, &genesis, &[], &cert, 0)
        .unwrap()
        .expect("admitted");
    account
}

#[test]
fn a_member_cannot_make_a_non_member_accounts_device_an_author() {
    let member = PrivateKey::random(&mut UnwrapErr(SysRng)).public_key();
    let (store, proj, ns, delta_id) = namespace_with_member(member);
    let heads = [delta_id];
    let device_sign_pk = PrivateKey::random(&mut UnwrapErr(SysRng)).public_key();

    assert_eq!(
        proj.member_at_cut(&store, ns, &member, &heads),
        Some(true),
        "precondition: the endorsing key is a member at this cut"
    );

    let outside = link_device_of_outside_account(&store, ns, &member, &device_sign_pk);
    assert!(
        !MembershipRepository::new(&store)
            .is_member(&ns, &outside)
            .unwrap(),
        "precondition: the account the device speaks for is not a member"
    );

    assert_eq!(
        proj.member_at_cut(&store, ns, &device_sign_pk, &heads),
        Some(false),
        "a device speaks with its own account's standing, not with its endorser's"
    );
    assert_eq!(
        calimero_governance_store::member_account_for_device_key(&store, &ns, &device_sign_pk)
            .unwrap(),
        None,
        "the node must not pick a non-member account's device as an executing identity"
    );
}

const NS: [u8; 32] = [0x21; 32];
const SUB: [u8; 32] = [0x22; 32];
const CONTEXT: [u8; 32] = [0x23; 32];

/// One party with a real account, a device under it and the certificate for it.
struct Party {
    account: AccountId,
    device_sk: PrivateKey,
    proof: Box<AccountProof<DeviceCert>>,
}

fn party(root_seed: u8, device_seed: u8, nonce: u8) -> Party {
    let root = PrivateKey::from([root_seed; 32]);
    let genesis = AccountGenesis::new(root.public_key());
    let account = genesis.account_id();
    let device_sk = PrivateKey::from([device_seed; 32]);
    let cert = DeviceCert::sign(
        &root,
        account,
        DeviceId::mint(account, [nonce; 16]),
        &device_sk.public_key(),
        &KemPublicKey::from([nonce; 32]),
        0,
        0,
    )
    .expect("the certificate must sign");
    Party {
        account,
        device_sk,
        proof: Box::new(AccountProof {
            genesis,
            chain: vec![],
            statement: cert,
        }),
    }
}

/// A namespace with one subgroup context, an author who is a member of the
/// subgroup, and a relay that may author for them. Both devices are bound in the
/// namespace, which is where bindings and revocations live.
struct World {
    store: Store,
    ns: ContextGroupId,
    context: ContextId,
    author: Party,
    relay: Party,
    delegation: Delegation,
}

fn bind(store: &Store, ns: &ContextGroupId, endorser: AccountId, party: &Party) {
    let bindings = AccountBindingRepository::new(store);
    bindings
        .record_endorser(ns, party.account, &endorser)
        .expect("record the endorser");
    let _admitted = bindings
        .apply_link(ns, &party.proof.genesis, &[], &party.proof.statement, 0)
        .expect("apply the link");
}

fn world() -> World {
    let store = store();
    let ns = ContextGroupId::from(NS);
    let sub = ContextGroupId::from(SUB);
    let context = ContextId::from(CONTEXT);

    let admin_key = PrivateKey::from([0xEE; 32]).public_key();
    let admin = calimero_context::test_support::enrol(&store, &ns, &admin_key);
    MetaRepository::new(&store)
        .save(&ns, &meta(admin))
        .expect("save the namespace meta");
    MembershipRepository::new(&store)
        .add_member(&ns, &admin, GroupMemberRole::Admin)
        .expect("add the admin");
    NamespaceRepository::new(&store)
        .nest(&ns, &sub)
        .expect("nest the subgroup");
    calimero_governance_store::register_context_in_group(&store, &sub, &context)
        .expect("register the context in the subgroup");

    let author = party(0x31, 0x32, 0x01);
    let relay = party(0x41, 0x42, 0x02);
    for p in [&author, &relay] {
        bind(&store, &ns, admin, p);
        MembershipRepository::new(&store)
            .add_member(&sub, &p.account, GroupMemberRole::Member)
            .expect("add the member to the subgroup");
    }
    CapabilitiesRepository::new(&store)
        .set_member_capability(
            &sub,
            &relay.account,
            MemberCapabilities::CAN_AUTHOR_ON_BEHALF.bits(),
        )
        .expect("grant authorship");

    let delegation = delegation_for(&author, &relay, context, 7);
    World {
        store,
        ns,
        context,
        author,
        relay,
        delegation,
    }
}

fn delegation_for(author: &Party, relay: &Party, context: ContextId, nonce: u64) -> Delegation {
    let warrant = Warrant::sign(
        &author.device_sk,
        WarrantTerms {
            context,
            author_account: author.account,
            executor: relay.account,
            app_version: ApplicationId::from([0u8; 32]),
            method: "send_message".to_owned(),
            intent_hash: Warrant::intent_hash("send_message", br#"{"text":"on my way"}"#),
            account_heads: vec![],
            governance_floor: vec![],
            nonce,
            not_after: u64::MAX,
        },
    )
    .expect("the warrant must sign");
    Delegation {
        warrant: Box::new(warrant),
        author_proof: author.proof.clone(),
        executor_proof: relay.proof.clone(),
        executor_key: relay.device_sk.public_key(),
    }
}

fn refusal(w: &World) -> Option<WarrantRefusal> {
    check_delegated_delta(&w.store, &w.context, &w.delegation)
        .err()
        .and_then(|err| err.downcast_ref::<WarrantRefusal>().copied())
}

#[test]
fn a_relay_may_write_for_an_author_in_a_subgroup_context() {
    let w = world();
    assert_eq!(refusal(&w), None, "precondition: the world admits a write");
}

#[test]
fn a_device_revoked_in_the_namespace_cannot_author_in_a_subgroup_context() {
    let w = world();
    AccountBindingRepository::new(&w.store)
        .apply_revocation(&w.ns, w.author.proof.statement.device)
        .expect("revoke the author's device in the namespace");

    assert_eq!(
        refusal(&w),
        Some(WarrantRefusal::AuthorDeviceRevoked),
        "a revocation recorded for the namespace covers its subgroup contexts"
    );
}

#[test]
fn a_relay_device_revoked_in_the_namespace_cannot_relay_into_a_subgroup_context() {
    let w = world();
    AccountBindingRepository::new(&w.store)
        .apply_revocation(&w.ns, w.relay.proof.statement.device)
        .expect("revoke the relay's device in the namespace");

    assert_eq!(refusal(&w), Some(WarrantRefusal::ExecutorDeviceRevoked));
}

#[test]
fn a_descoped_device_cannot_author_in_the_application_it_lost() {
    let w = world();
    let cert = &w.author.proof.statement;
    let narrowed = AccountBindingRepository::new(&w.store)
        .narrow(&w.ns, cert.account, cert.device, 1)
        .expect("narrow the device out of the namespace");
    assert!(narrowed, "precondition: the device was bound, and now is not");

    assert_eq!(
        refusal(&w),
        Some(WarrantRefusal::AuthorDeviceRevoked),
        "a device narrowed out of the namespace's application authors nothing there"
    );
}

#[test]
fn a_relay_device_that_lost_the_application_cannot_relay() {
    let w = world();
    let cert = &w.relay.proof.statement;
    AccountBindingRepository::new(&w.store)
        .narrow(&w.ns, cert.account, cert.device, 1)
        .expect("narrow the relay's device out of the namespace");

    assert_eq!(refusal(&w), Some(WarrantRefusal::ExecutorDeviceRevoked));
}

#[test]
fn a_device_widened_again_after_being_narrowed_out_authors_again() {
    let w = world();
    let cert = &w.author.proof.statement;
    let bindings = AccountBindingRepository::new(&w.store);
    bindings
        .narrow(&w.ns, cert.account, cert.device, 1)
        .expect("narrow");
    assert!(refusal(&w).is_some(), "precondition: narrowed out");

    bindings
        .apply_link(&w.ns, &w.author.proof.genesis, &[], cert, 2)
        .expect("apply the link")
        .expect("a link under a newer scope is admitted");

    assert_eq!(refusal(&w), None);
}

#[test]
fn a_device_no_namespace_row_names_still_authors() {
    // A thin client's device is certified by its account and bound nowhere. Rows
    // that withdraw a device exist only where a revocation or a narrowing was
    // recorded, so their absence is not a reason to refuse.
    let w = world();
    let bystander = party(0x51, 0x52, 0x03);
    MembershipRepository::new(&w.store)
        .add_member(
            &ContextGroupId::from(SUB),
            &bystander.account,
            GroupMemberRole::Member,
        )
        .expect("add the bystander");
    let delegation = delegation_for(&bystander, &w.relay, w.context, 8);

    check_delegated_delta(&w.store, &w.context, &delegation)
        .expect("an unbound device of a member still authors");
}
