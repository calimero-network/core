//! A device the namespace withdrew authors nothing in it, subgroups included.
//!
//! Revocations and scope floors are recorded under the namespace, so a check
//! that looks them up under the context's own group never finds them.

use std::sync::Arc;

use calimero_account::{
    AccountGenesis, AccountId, AccountProof, Delegation, DeviceCert, DeviceId, KemPublicKey,
    Warrant, WarrantTerms,
};
use calimero_context_config::types::ContextGroupId;
use calimero_context_config::MemberCapabilities;
use calimero_governance_store::warrant_gate::{check_delegated_delta, WarrantRefusal};
use calimero_governance_store::{
    AccountBindingRepository, AdmissionCut, CapabilitiesRepository, MembershipRepository,
    MetaRepository, NamespaceRepository,
};
use calimero_primitives::application::ApplicationId;
use calimero_primitives::context::{ContextId, GroupMemberRole};
use calimero_primitives::identity::PrivateKey;
use calimero_store::db::InMemoryDB;
use calimero_store::key::{GroupMetaValue, GroupTarget};
use calimero_store::Store;

fn store() -> Store {
    Store::new(Arc::new(InMemoryDB::owned()))
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
            executor_key: relay.device_sk.public_key(),
            release_bytecode_id: [0u8; 32],
            release_version: String::new(),
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
    check_delegated_delta(&w.store, &w.context, &w.delegation, AdmissionCut::live())
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
    assert!(
        narrowed,
        "precondition: the device was bound, and now is not"
    );

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

    check_delegated_delta(&w.store, &w.context, &delegation, AdmissionCut::live())
        .expect("an unbound device of a member still authors");
}
