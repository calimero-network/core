//! A delegated delta, produced the way the executor produces one and consumed
//! the way a peer consumes one.
//!
//! Every other test of this feature checks one layer. The unit tests in
//! `calimero-account` prove a bundle verifies, the ones in `delta_auth` prove an
//! envelope verifies, and the ones in `warrant_gate` prove the cut admits it.
//! None of them prove the layers AGREE — and that is the failure this feature is
//! most exposed to, because the producer and the verifier build the signed
//! preimage in different files and a drift between them is invisible until a
//! peer refuses a delta the author's own node accepted.
//!
//! So this test calls the real functions on both sides: the preimage builder the
//! execute path uses to sign, and the single entry point every receive path uses
//! to verify. If those two ever disagree, this fails and nothing else does.
//!
//! It stops short of running WASM. Producing a delta needs a runtime, a module
//! and a scope key, and none of that is what could silently break — the seam is
//! the preimage and the gate, and both are exercised here against real
//! credentials and real governance rows.

use calimero_store::key::GroupTarget;
use std::sync::Arc;

use calimero_account::{
    AccountGenesis, AccountId, AccountProof, Delegation, DeviceCert, DeviceId, KemPublicKey,
    Warrant, WarrantTerms,
};
use calimero_context_config::types::ContextGroupId;
use calimero_context_config::MemberCapabilities;
use calimero_governance_store::warrant_gate::{
    check_delegated_delta, spend_warrant_nonce, WarrantRefusal,
};
use calimero_governance_store::{
    AccountBindingRepository, AdmissionCut, CapabilitiesRepository, MembershipRepository,
    MetaRepository,
};
use calimero_node_primitives::sync::delta_auth::{
    delegated_delta_signature_payload, verify_delta_envelope, VerifiedEnvelope,
};
use calimero_primitives::context::{ContextId, GroupMemberRole};
use calimero_primitives::identity::PrivateKey;
use calimero_storage::logical_clock::{HybridTimestamp, Timestamp, ID, NTP64};
use calimero_store::db::InMemoryDB;
use calimero_store::key::GroupMetaValue;
use calimero_store::Store;
use core::num::NonZeroU64;

const GROUP: [u8; 32] = [0x11; 32];
const CONTEXT: [u8; 32] = [0x12; 32];
const DELTA_ID: [u8; 32] = [0x13; 32];

fn store() -> Store {
    Store::new(Arc::new(InMemoryDB::owned()))
}

fn hlc() -> HybridTimestamp {
    HybridTimestamp::new(Timestamp::new(
        NTP64(1_700_000_000),
        ID::from(NonZeroU64::new(1).unwrap()),
    ))
}

fn meta(admin: AccountId) -> GroupMetaValue {
    GroupMetaValue {
        target: GroupTarget {
            application_id: calimero_primitives::application::ApplicationId::from([0xCC; 32]),
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

/// One party with a real account: a root key, the account it addresses, one
/// device under it, and the root-signed certificate proving the binding.
struct Party {
    account: AccountId,
    device_sk: PrivateKey,
    proof: Box<AccountProof<DeviceCert>>,
}

/// Build a party whose certificate a verifier can check from the account id
/// alone — the property that lets a device which never joined the group author.
fn party(root_seed: u8, device_seed: u8, nonce: u8) -> Party {
    let root = PrivateKey::from([root_seed; 32]);
    let genesis = AccountGenesis::new(root.public_key());
    let account = genesis.account_id();
    let device = DeviceId::mint(account, [nonce; 16]);
    let device_sk = PrivateKey::from([device_seed; 32]);
    let cert = DeviceCert::sign(
        &root,
        account,
        device,
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

struct World {
    store: Store,
    group: ContextGroupId,
    context: ContextId,
    author: Party,
    relay: Party,
    delegation: Delegation,
}

/// A group in which the author is a member and the relay may author for them —
/// the state a delegated write is supposed to succeed in.
fn world(nonce: u64) -> World {
    let store = store();
    let group = ContextGroupId::from(GROUP);
    let context = ContextId::from(CONTEXT);

    let admin_key = PrivateKey::from([0xEE; 32]).public_key();
    let admin = calimero_context::test_support::enrol(&store, &group, &admin_key);
    MetaRepository::new(&store)
        .save(&group, &meta(admin))
        .expect("save the group meta");
    MembershipRepository::new(&store)
        .add_member(&group, &admin, GroupMemberRole::Admin)
        .expect("add the admin");
    calimero_governance_store::register_context_in_group(&store, &group, &context)
        .expect("register the context");

    let author = party(0x21, 0x22, 0x01);
    let relay = party(0x31, 0x32, 0x02);

    // Both accounts are real members, and both bindings are real: the author's
    // certificate is what a peer resolves, and the relay's is what proves the
    // key that signs the envelope belongs to the operator.
    let bindings = AccountBindingRepository::new(&store);
    for p in [&author, &relay] {
        bindings
            .record_endorser(&group, p.account, &admin)
            .expect("record the endorser");
        let _admitted = bindings
            .apply_link(&group, &p.proof.genesis, &[], &p.proof.statement, 0)
            .expect("apply the link");
        MembershipRepository::new(&store)
            .add_member(&group, &p.account, GroupMemberRole::Member)
            .expect("add the member");
    }

    CapabilitiesRepository::new(&store)
        .set_member_capability(
            &group,
            &relay.account,
            MemberCapabilities::CAN_AUTHOR_ON_BEHALF.bits(),
        )
        .expect("grant authorship");

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

    let delegation = Delegation {
        warrant: Box::new(warrant),
        author_proof: author.proof.clone(),
        executor_proof: relay.proof.clone(),
        executor_key: relay.device_sk.public_key(),
    };

    World {
        store,
        group,
        context,
        author,
        relay,
        delegation,
    }
}

/// Sign the envelope exactly as the execute path does: the AUTHOR's device is
/// named, and THIS node's key signs.
fn produce(w: &World) -> [u8; 64] {
    let payload = delegated_delta_signature_payload(
        w.context,
        DELTA_ID,
        w.author.device_sk.public_key(),
        &w.delegation,
        None,
        hlc(),
    )
    .expect("the preimage must serialize");
    w.relay
        .device_sk
        .sign(&payload)
        .expect("the relay must be able to sign")
        .to_bytes()
}

/// Verify it exactly as every receive path does.
fn receive(w: &World, signature: &[u8; 64]) -> eyre::Result<VerifiedEnvelope> {
    verify_delta_envelope(
        w.context,
        DELTA_ID,
        w.author.device_sk.public_key(),
        Some(&w.delegation),
        None,
        None,
        hlc(),
        signature,
    )
}

/// The whole point: what the executor signs is what a peer accepts, and the peer
/// learns from it that the change belongs to the author rather than the signer.
#[test]
fn a_delta_the_executor_signs_is_accepted_and_attributed_to_the_author() {
    let w = world(7);
    let signature = produce(&w);

    let envelope = receive(&w, &signature).expect("a peer must accept what the executor signed");

    match envelope {
        VerifiedEnvelope::Delegated(warrant) => {
            assert_eq!(
                warrant.author_account, w.author.account,
                "the change must be attributed to the author, not the signer"
            );
            assert_eq!(
                warrant.executor, w.relay.account,
                "and the operator that acted must be named"
            );
        }
        VerifiedEnvelope::SelfAuthored | VerifiedEnvelope::Tee(_) => {
            panic!("a delta carrying a delegation must be read as delegated")
        }
    }

    // Envelope accepted; now the at-cut half, which is a separate decision and
    // the one that can refuse a perfectly authentic delta.
    check_delegated_delta(&w.store, &w.context, &w.delegation, AdmissionCut::live())
        .expect("an authorized relay writing for a member must be admitted");
}

/// The full sequence a peer actually runs, in order, including the spend — and
/// then the same delta again, which is what a relay replaying would produce.
#[test]
fn one_warrant_admits_one_delta() {
    let w = world(7);
    let signature = produce(&w);

    receive(&w, &signature).expect("envelope");
    check_delegated_delta(&w.store, &w.context, &w.delegation, AdmissionCut::live()).expect("cut");
    spend_warrant_nonce(&w.store, &w.context, &w.delegation).expect("spend");

    // The signature is still valid — replay is not a forgery, which is exactly
    // why the envelope check cannot be what stops it.
    receive(&w, &signature).expect("the envelope still verifies on a replay");

    let err = check_delegated_delta(&w.store, &w.context, &w.delegation, AdmissionCut::live())
        .expect_err("the cut must refuse a warrant already spent");
    assert_eq!(
        err.downcast_ref::<WarrantRefusal>(),
        Some(&WarrantRefusal::NonceAlreadySpent)
    );
}

/// The grant is load-bearing. Same authentic delta, no authorship, refused —
/// and refused at the cut, not at the envelope, because authenticity is
/// unaffected by whether the operator was allowed.
#[test]
fn a_delta_is_refused_when_the_relay_holds_no_grant() {
    let w = world(7);
    let signature = produce(&w);
    CapabilitiesRepository::new(&w.store)
        .set_member_capability(
            &w.group,
            &w.relay.account,
            MemberCapabilities::empty().bits(),
        )
        .expect("withdraw authorship");

    receive(&w, &signature).expect("authenticity is unaffected by the grant");

    let err = check_delegated_delta(&w.store, &w.context, &w.delegation, AdmissionCut::live())
        .expect_err("a relay with no grant must not author");
    assert_eq!(
        err.downcast_ref::<WarrantRefusal>(),
        Some(&WarrantRefusal::ExecutorMayNotAuthor)
    );
}

/// **Receive-path regression.** A delta relayed by a TEE replica is authentic
/// and still refused at the cut — even holding `CAN_AUTHOR_ON_BEHALF`, so a
/// patched replica that skipped its own `/intents` check cannot get one
/// applied. Before this change the bit alone admitted it.
#[test]
fn a_delta_relayed_by_a_tee_replica_is_refused_at_the_cut() {
    let w = world(7);
    let signature = produce(&w);
    MembershipRepository::new(&w.store)
        .set_role(&w.group, &w.relay.account, GroupMemberRole::ReadOnlyTee)
        .expect("make the relay a TEE replica");

    receive(&w, &signature).expect("authenticity is unaffected by the role");

    let err = check_delegated_delta(&w.store, &w.context, &w.delegation, AdmissionCut::live())
        .expect_err("a TEE replica must not relay, whatever it holds");
    assert_eq!(
        err.downcast_ref::<WarrantRefusal>(),
        Some(&WarrantRefusal::ExecutorIsTeeReplica)
    );
}

/// And a TEE relay's delta is admitted by its role, with the grant withdrawn.
#[test]
fn a_delta_relayed_by_a_tee_relay_is_admitted_without_the_grant() {
    let w = world(7);
    let signature = produce(&w);
    MembershipRepository::new(&w.store)
        .set_role(&w.group, &w.relay.account, GroupMemberRole::RelayTee)
        .expect("make the relay a TEE relay");
    CapabilitiesRepository::new(&w.store)
        .set_member_capability(
            &w.group,
            &w.relay.account,
            MemberCapabilities::empty().bits(),
        )
        .expect("withdraw the bit");

    receive(&w, &signature).expect("envelope");
    check_delegated_delta(&w.store, &w.context, &w.delegation, AdmissionCut::live())
        .expect("a TEE relay authors by its role");
}

/// A read-only author's delegated write is refused at the cut too, so peers
/// agree with the relay that refused it rather than one peer applying what
/// another drops.
#[test]
fn a_delta_whose_author_is_read_only_is_refused_at_the_cut() {
    let w = world(7);
    let signature = produce(&w);
    MembershipRepository::new(&w.store)
        .set_role(&w.group, &w.author.account, GroupMemberRole::ReadOnly)
        .expect("demote the author");

    receive(&w, &signature).expect("envelope");
    let err = check_delegated_delta(&w.store, &w.context, &w.delegation, AdmissionCut::live())
        .expect_err("a read-only author may not write through a relay");
    assert_eq!(
        err.downcast_ref::<WarrantRefusal>(),
        Some(&WarrantRefusal::AuthorIsReadOnly)
    );
}

/// A relay cannot promote itself by presenting a warrant for an author who is
/// not a member here. The author's ACCOUNT is what is checked — the device never
/// joined this group and could not be.
#[test]
fn a_delta_is_refused_when_the_author_is_not_a_member() {
    let w = world(7);
    let signature = produce(&w);
    MembershipRepository::new(&w.store)
        .remove_member(&w.group, &w.author.account)
        .expect("remove the author");

    receive(&w, &signature).expect("authenticity is unaffected by membership");

    let err = check_delegated_delta(&w.store, &w.context, &w.delegation, AdmissionCut::live())
        .expect_err("a non-member cannot be written for");
    assert_eq!(
        err.downcast_ref::<WarrantRefusal>(),
        Some(&WarrantRefusal::AuthorNotAMember)
    );
}

/// Revoking the author's device stops the relay writing for it, which is the
/// offboarding path working: the certificate stays valid forever, so revocation
/// is the only thing that can withdraw it and it has to be read at the cut.
#[test]
fn a_delta_is_refused_once_the_author_device_is_revoked() {
    let w = world(7);
    let signature = produce(&w);

    AccountBindingRepository::new(&w.store)
        .apply_revocation(&w.group, w.delegation.author_proof.statement.device)
        .expect("revoke the author's device");

    receive(&w, &signature)
        .expect("the certificate still verifies — a revocation is not a forgery");

    let err = check_delegated_delta(&w.store, &w.context, &w.delegation, AdmissionCut::live())
        .expect_err("a revoked device must not be written for");
    assert_eq!(
        err.downcast_ref::<WarrantRefusal>(),
        Some(&WarrantRefusal::AuthorDeviceRevoked)
    );
}

/// And the drift this file exists to catch: a signature over the self-authored
/// preimage must not be accepted for a delta carrying a delegation. If the two
/// preimages ever converge, a relay could strip the warrant — or forget to
/// attach one — and the result would still apply.
#[test]
fn the_self_authored_preimage_is_not_accepted_for_a_delegated_delta() {
    let w = world(7);

    let self_payload = calimero_node_primitives::sync::delta_auth::delta_signature_payload(
        w.context,
        DELTA_ID,
        w.author.device_sk.public_key(),
        None,
        hlc(),
    )
    .expect("the preimage must serialize");
    let signed_by_author = w
        .author
        .device_sk
        .sign(&self_payload)
        .expect("sign")
        .to_bytes();

    let _refused = receive(&w, &signed_by_author)
        .expect_err("a self-authored signature must not carry a delegated delta");
}

/// A delta an account writes through a relay, in the context of a subgroup it
/// created through the same relay, is admitted at the cut the delta cites.
///
/// That is a DM started from a thin client: the relay publishes the subgroup's
/// creation on the account's behalf, registers the context, and then relays
/// the account's first message. The apply seats the creator with an `Admin`
/// row; the fold records it as the subgroup's genesis admin and keeps no row.
/// The at-cut standing read looked only for the row, so every peer refused the
/// message as "not a member of the group owning this context" while its
/// member list named the account the subgroup's Admin.
#[test]
fn a_delta_in_a_subgroup_its_author_created_is_admitted_at_the_cut() {
    use std::sync::RwLock;

    use calimero_context::scope_projection::ScopeProjections;
    use calimero_context::ProjectionAuthorizer;
    use calimero_governance_store::unified_op_decode::op_from_namespace_op;
    use calimero_governance_store::NamespaceRepository;
    use calimero_governance_types::{
        EncryptedGroupOp, GroupOp, NamespaceOp, RootOp, SignedNamespaceOp,
    };

    let store = store();
    let namespace = [0x41; 32];
    let subgroup = ContextGroupId::from([0x42; 32]);
    let context = ContextId::from([0x43; 32]);
    NamespaceRepository::new(&store)
        .nest(&ContextGroupId::from(namespace), &subgroup)
        .expect("nest the subgroup under the namespace");
    calimero_governance_store::register_context_in_group(&store, &subgroup, &context)
        .expect("register the context");

    let author = party(0x51, 0x52, 0x03);
    let relay = party(0x61, 0x62, 0x04);
    let relay_key = relay.device_sk.public_key();

    let signed = |op: NamespaceOp| SignedNamespaceOp {
        version: 1,
        namespace_id: namespace.into(),
        parent_op_hashes: Vec::new(),
        signer: relay_key,
        nonce: 0,
        op,
        signature: [0u8; 64],
        admitter_endorsement: None,
    };
    let in_subgroup = || {
        signed(NamespaceOp::Group {
            group_id: subgroup.to_bytes().into(),
            key_id: [0u8; 32].into(),
            encrypted: EncryptedGroupOp {
                nonce: [0u8; 12],
                ciphertext: Vec::new(),
            },
            key_rotation: None,
        })
    };

    // The relay publishes the creation; the admin it names is the author.
    let created = op_from_namespace_op(
        &signed(NamespaceOp::Root(RootOp::GroupCreated {
            admin: author.account,
            group_id: subgroup.to_bytes().into(),
            parent_id: namespace.into(),
            restricted: true,
            salt: [0; 32],
        })),
        None,
        [0xE1; 32],
        hlc(),
        &[],
    );
    // And may author in it, as the relay a creation seats would.
    let seated = op_from_namespace_op(
        &in_subgroup(),
        Some(&GroupOp::MemberAdded {
            member: relay.account,
            role: GroupMemberRole::Member,
        }),
        [0xE2; 32],
        hlc(),
        &[[0xE1; 32]],
    );
    let granted = op_from_namespace_op(
        &in_subgroup(),
        Some(&GroupOp::MemberCapabilitySet {
            member: relay.account,
            capabilities: MemberCapabilities::CAN_AUTHOR_ON_BEHALF,
        }),
        [0xE3; 32],
        hlc(),
        &[[0xE2; 32]],
    );
    let mut folded = ScopeProjections::new();
    for op in [&created, &seated, &granted] {
        folded.ingest_op(op);
    }
    let projections = RwLock::new(folded);

    let warrant = Warrant::sign(
        &author.device_sk,
        WarrantTerms {
            context,
            author_account: author.account,
            executor: relay.account,
            executor_key: relay_key,
            release_bytecode_id: [0u8; 32],
            release_version: String::new(),
            method: "send_message".to_owned(),
            intent_hash: Warrant::intent_hash("send_message", br#"{"text":"hi"}"#),
            account_heads: vec![],
            governance_floor: vec![],
            nonce: 1,
            not_after: u64::MAX,
        },
    )
    .expect("the warrant must sign");
    let delegation = Delegation {
        warrant: Box::new(warrant),
        author_proof: author.proof.clone(),
        executor_proof: relay.proof.clone(),
        executor_key: relay_key,
    };

    let authorizer = ProjectionAuthorizer::new(&projections, &store);
    check_delegated_delta(
        &store,
        &context,
        &delegation,
        AdmissionCut::at(&authorizer, &[[0xE3; 32]]),
    )
    .expect("the subgroup's creator must be written for in its own context");
}
