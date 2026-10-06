//! Delegated context creation over the wire: a relay publishes
//! `ContextRegisteredOnBehalf` signed with its own key, and two independent
//! replicas decide the same thing about it.
//!
//! The unit tests in `calimero-governance-store` pin the gate; this pins the
//! property that matters on a network, which no single-store test can see:
//! **every replica reaches the same verdict** — both accept a member's creation
//! carried by a relay that could not create for itself, and both refuse the
//! ways a relay could try to bend what the member signed. A registration one
//! peer applied and another refused would leave the group disagreeing about
//! which contexts it has.
//!
//! Each peer applies the borsh-encoded `SignedGroupOp` the way inbound gossip
//! does (`apply_local_signed_group_op`), from its own store.

use std::sync::Arc;

use calimero_account::{
    AccountId, ContextCreationDelegation, ContextCreationTerms, ContextCreationWarrant,
};
use calimero_context_client::local_governance::{GroupOp, SignedGroupOp};
use calimero_context_config::types::ContextGroupId;
use calimero_context_config::MemberCapabilities;
use calimero_governance_store::creation_gate::CreationRefusal;
use calimero_governance_store::{
    apply_local_signed_group_op, get_group_for_context, CapabilitiesRepository,
    MembershipRepository, MetaRepository, MetadataRepository,
};
use calimero_primitives::application::ApplicationId;
use calimero_primitives::blobs::BlobId;
use calimero_primitives::context::{ContextId, GroupMemberRole};
use calimero_primitives::identity::PrivateKey;
use calimero_store::db::InMemoryDB;
use calimero_store::key::{GroupMetaValue, GroupTarget};
use calimero_store::Store;

const GROUP: [u8; 32] = [0x7C; 32];
const SEED: [u8; 32] = [0x5E; 32];
const APP: [u8; 32] = [0xAA; 32];
const INIT: &[u8] = b"{}";

fn empty_store() -> Store {
    Store::new(Arc::new(InMemoryDB::owned()))
}

fn meta(admin: AccountId) -> GroupMetaValue {
    GroupMetaValue {
        target: GroupTarget {
            application_id: ApplicationId::from(APP),
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

/// How the relay stands in the group, on every replica alike.
#[derive(Clone, Copy, Debug)]
enum Standing {
    Granted,
    RelayTee,
    Ungranted,
}

struct Network {
    peers: [Store; 2],
    group: ContextGroupId,
    author_sk: PrivateKey,
    author: AccountId,
    relay_sk: PrivateKey,
    relay: AccountId,
}

/// Two replicas holding the same folded group: an admin, an author who is a
/// `Member` (with `CAN_CREATE_CONTEXT` when `author_may_create`), and a relay
/// that holds NO create rights of its own.
fn network(relay_standing: Standing, author_may_create: bool) -> Network {
    let group = ContextGroupId::from(GROUP);
    let admin_sk = PrivateKey::from([0x11; 32]);
    let author_sk = PrivateKey::from([0x44; 32]);
    let relay_sk = PrivateKey::from([0x22; 32]);
    let peers = [empty_store(), empty_store()];

    let mut author = None;
    let mut relay = None;
    for store in &peers {
        let admin = calimero_context::test_support::enrol(store, &group, &admin_sk.public_key());
        MetaRepository::new(store)
            .save(&group, &meta(admin))
            .expect("meta");
        let membership = MembershipRepository::new(store);
        membership
            .add_member(&group, &admin, GroupMemberRole::Admin)
            .expect("admin");

        let a = calimero_context::test_support::enrol(store, &group, &author_sk.public_key());
        membership
            .add_member(&group, &a, GroupMemberRole::Member)
            .expect("author");
        if author_may_create {
            CapabilitiesRepository::new(store)
                .set_member_capability(&group, &a, MemberCapabilities::CAN_CREATE_CONTEXT.bits())
                .expect("author may create");
        }

        let r = calimero_context::test_support::enrol(store, &group, &relay_sk.public_key());
        let (role, caps) = match relay_standing {
            Standing::Granted => (
                GroupMemberRole::Member,
                MemberCapabilities::CAN_AUTHOR_ON_BEHALF.bits(),
            ),
            Standing::RelayTee => (GroupMemberRole::RelayTee, 0),
            Standing::Ungranted => (GroupMemberRole::Member, 0),
        };
        membership.add_member(&group, &r, role).expect("relay");
        CapabilitiesRepository::new(store)
            .set_member_capability(&group, &r, caps)
            .expect("relay caps");

        author = Some(a);
        relay = Some(r);
    }

    Network {
        peers,
        group,
        author_sk,
        author: author.expect("enrolled"),
        relay_sk,
        relay: relay.expect("enrolled"),
    }
}

impl Network {
    fn terms(&self) -> ContextCreationTerms {
        ContextCreationTerms {
            group: self.group.to_bytes(),
            seed: SEED,
            author_account: self.author,
            executor: self.relay,
            executor_key: self.relay_sk.public_key(),
            application_id: ApplicationId::from(APP),
            service_name: None,
            name: Some("general".to_owned()),
            init_hash: ContextCreationWarrant::init_hash(INIT),
            account_heads: vec![],
            governance_floor: vec![],
            nonce: 1,
            not_after: u64::MAX,
        }
    }

    fn delegation_with(&self, terms: ContextCreationTerms) -> ContextCreationDelegation {
        ContextCreationDelegation {
            warrant: Box::new(ContextCreationWarrant::sign(&self.author_sk, terms).expect("sign")),
            author_proof: calimero_context::test_support::credential(&self.author_sk.public_key()),
            executor_proof: calimero_context::test_support::credential(&self.relay_sk.public_key()),
            executor_key: self.relay_sk.public_key(),
        }
    }

    fn registration(
        &self,
        context_id: ContextId,
        name: Option<&str>,
        delegation: ContextCreationDelegation,
    ) -> GroupOp {
        GroupOp::ContextRegisteredOnBehalf {
            context_id,
            application_id: ApplicationId::from(APP),
            blob_id: BlobId::from([0xBB; 32]),
            source: String::new(),
            service_name: None,
            package: "com.example.chat".to_owned(),
            version: "1.0.0".to_owned(),
            name: name.map(str::to_owned),
            delegation: Box::new(delegation),
        }
    }

    /// The op as the relay would publish it: signed by its own key, then
    /// borsh-encoded for the wire.
    fn published_by(&self, signer: &PrivateKey, nonce: u64, op: GroupOp) -> Vec<u8> {
        let signed = SignedGroupOp::sign(signer, self.group.to_bytes().into(), vec![], nonce, op)
            .expect("sign the op");
        borsh::to_vec(&signed).expect("encode")
    }

    /// Apply on every replica, returning each one's verdict.
    fn deliver(&self, payload: &[u8]) -> Vec<eyre::Result<()>> {
        self.peers
            .iter()
            .map(|store| {
                let op: SignedGroupOp = borsh::from_slice(payload).expect("decode");
                apply_local_signed_group_op(store, &op).map(|_| ())
            })
            .collect()
    }

    fn registered_everywhere(&self, context_id: &ContextId) -> [bool; 2] {
        self.peers.each_ref().map(|store| {
            get_group_for_context(store, context_id).expect("read") == Some(self.group)
        })
    }
}

fn context() -> ContextId {
    ContextId::from_seed(SEED)
}

/// Both replicas refused, for the same typed reason.
fn assert_refused_everywhere(verdicts: &[eyre::Result<()>], expected: &CreationRefusal) {
    for (i, verdict) in verdicts.iter().enumerate() {
        let err = verdict
            .as_ref()
            .expect_err(&format!("replica {i} must refuse"));
        assert_eq!(
            err.downcast_ref::<CreationRefusal>(),
            Some(expected),
            "replica {i}: {err:?}"
        );
    }
}

#[test]
fn both_replicas_accept_a_members_creation_carried_by_a_relay() {
    for standing in [Standing::Granted, Standing::RelayTee] {
        let net = network(standing, true);
        let payload = net.published_by(
            &net.relay_sk,
            1,
            net.registration(context(), Some("general"), net.delegation_with(net.terms())),
        );
        for (i, verdict) in net.deliver(&payload).into_iter().enumerate() {
            verdict.unwrap_or_else(|err| panic!("{standing:?} replica {i}: {err:?}"));
        }
        assert_eq!(
            net.registered_everywhere(&context()),
            [true, true],
            "{standing:?}"
        );
        for store in &net.peers {
            let name = MetadataRepository::new(store)
                .context_metadata(&net.group, &context())
                .expect("read")
                .and_then(|record| record.name);
            assert_eq!(name.as_deref(), Some("general"), "{standing:?}");
        }
    }
}

#[test]
fn both_replicas_refuse_an_author_without_create_rights() {
    let net = network(Standing::Granted, false);
    let payload = net.published_by(
        &net.relay_sk,
        1,
        net.registration(context(), Some("general"), net.delegation_with(net.terms())),
    );
    assert_refused_everywhere(&net.deliver(&payload), &CreationRefusal::AuthorMayNotCreate);
    assert_eq!(net.registered_everywhere(&context()), [false, false]);
}

#[test]
fn both_replicas_refuse_a_relay_without_standing() {
    let net = network(Standing::Ungranted, true);
    let payload = net.published_by(
        &net.relay_sk,
        1,
        net.registration(context(), Some("general"), net.delegation_with(net.terms())),
    );
    let verdicts = net.deliver(&payload);
    for (i, verdict) in verdicts.iter().enumerate() {
        let err = verdict.as_ref().expect_err("refused");
        assert!(
            matches!(
                err.downcast_ref::<CreationRefusal>(),
                Some(CreationRefusal::Executor(_))
            ),
            "replica {i}: {err:?}"
        );
    }
    assert_eq!(net.registered_everywhere(&context()), [false, false]);
}

/// The relay renames the context after the member signed: every replica
/// catches it, because the name is re-checked against the warrant.
#[test]
fn both_replicas_refuse_a_relay_that_rewrites_the_name() {
    let net = network(Standing::Granted, true);
    let payload = net.published_by(
        &net.relay_sk,
        1,
        net.registration(context(), Some("renamed"), net.delegation_with(net.terms())),
    );
    assert_refused_everywhere(&net.deliver(&payload), &CreationRefusal::NameMismatch);
}

/// The relay registers a context id of its own choosing under the member's
/// warrant: refused, because every replica derives the id from the seed.
#[test]
fn both_replicas_refuse_a_context_id_the_seed_does_not_derive() {
    let net = network(Standing::Granted, true);
    let chosen = ContextId::from([0x42; 32]);
    let payload = net.published_by(
        &net.relay_sk,
        1,
        net.registration(chosen, Some("general"), net.delegation_with(net.terms())),
    );
    assert_refused_everywhere(&net.deliver(&payload), &CreationRefusal::ContextIdMismatch);
    assert_eq!(net.registered_everywhere(&chosen), [false, false]);
}

/// A captured bundle republished by someone other than the relay it names.
#[test]
fn both_replicas_refuse_the_bundle_published_by_another_key() {
    let net = network(Standing::Granted, true);
    let payload = net.published_by(
        &net.author_sk,
        1,
        net.registration(context(), Some("general"), net.delegation_with(net.terms())),
    );
    assert_refused_everywhere(
        &net.deliver(&payload),
        &CreationRefusal::SignerIsNotExecutor,
    );
}

/// The relay replays the warrant in a second op (a fresh op nonce, so the op
/// itself is new): refused everywhere once the first registration applied.
#[test]
fn both_replicas_refuse_a_replayed_creation_warrant() {
    let net = network(Standing::Granted, true);
    let delegation = net.delegation_with(net.terms());
    let first = net.published_by(
        &net.relay_sk,
        1,
        net.registration(context(), Some("general"), delegation.clone()),
    );
    for verdict in net.deliver(&first) {
        verdict.expect("first registration");
    }
    let replay = net.published_by(
        &net.relay_sk,
        2,
        net.registration(context(), Some("general"), delegation),
    );
    assert_refused_everywhere(&net.deliver(&replay), &CreationRefusal::AlreadySpent);
}

/// And the plain op stays closed to the relay: `ContextRegistered` signed by a
/// relay with no create rights is refused on every replica — the gap the
/// delegated op exists to close without widening.
#[test]
fn both_replicas_refuse_the_relay_registering_in_its_own_name() {
    let net = network(Standing::RelayTee, true);
    let plain = GroupOp::ContextRegistered {
        context_id: context(),
        application_id: ApplicationId::from(APP),
        blob_id: BlobId::from([0xBB; 32]),
        source: String::new(),
        service_name: None,
        package: String::new(),
        version: String::new(),
    };
    let payload = net.published_by(&net.relay_sk, 1, plain);
    for (i, verdict) in net.deliver(&payload).into_iter().enumerate() {
        let _refused = verdict.expect_err(&format!("replica {i} must refuse"));
    }
    assert_eq!(net.registered_everywhere(&context()), [false, false]);
}
