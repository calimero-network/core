//! A forged legacy genesis racing the real one to a joiner (#2932).
//!
//! # What this covers
//!
//! A namespace founded by derivation has `id = founded_namespace_id(founder,
//! salt)` and a `NamespaceCreatedV2` genesis naming that pair. A legacy
//! `NamespaceCreated` carries no salt, so a replica with no governance state
//! cannot tell a derived id from a random one. An attacker who signs a
//! self-consistent legacy genesis naming itself for the victim's id, and gets
//! it to a joiner first, used to establish the namespace there — and genesis is
//! a no-op once an admin exists, so the real genesis arriving next could never
//! repair it.
//!
//! The joiner's invitation now carries the pair (unsigned; it verifies against
//! the id). The joiner records it before syncing, and genesis apply then
//! refuses the legacy forgery with `Err`, which leaves the DAG head empty for
//! the real genesis.
//!
//! # How it is driven
//!
//! Each node is a real `calimero-store` over the real governance apply
//! (`NamespaceGovernance::apply_signed_op`). The invitation crosses to the
//! joiner as JSON, the way it does in production (pasted into `join`), and the
//! joiner adopts the hint with the same call `join_group` makes. Governance
//! ops are delivered in the adversarial order — forgery first — and redelivered
//! until nothing more applies, which is what the backfill path does with an op
//! that failed to apply.

use calimero_context_client::local_governance::{NamespaceOp, SignedNamespaceOp};
use calimero_context_config::types::{
    ContextGroupId, GroupInvitationFromAdmin, SignedGroupOpenInvitation,
};
use calimero_governance_store::test_fixtures::{
    namespace_genesis_for, namespace_genesis_v2_for, test_store,
};
use calimero_governance_store::{MetaRepository, NamespaceFoundingRepository, NamespaceGovernance};
use calimero_primitives::identity::PrivateKey;
use calimero_store::Store;
use rand::rand_core::UnwrapErr;
use rand::rngs::SysRng;

const SALT: [u8; 32] = [0x5e; 32];

/// Everything the scenario needs: the victim's derived namespace, its real
/// genesis, and the attacker's forged legacy genesis for the same id.
struct Race {
    namespace_id: [u8; 32],
    founder: calimero_account::AccountId,
    attacker: calimero_account::AccountId,
    real: SignedNamespaceOp,
    forged: SignedNamespaceOp,
    /// The founder's node, after applying its own genesis.
    founder_store: Store,
}

fn race() -> Race {
    let mut rng = UnwrapErr(SysRng);
    let founder_sk = PrivateKey::random(&mut rng);
    let attacker_sk = PrivateKey::random(&mut rng);

    let (real, founder, namespace_id) = namespace_genesis_v2_for(&founder_sk, SALT);
    let (forged, attacker): (NamespaceOp, _) = namespace_genesis_for(&attacker_sk);

    let real = SignedNamespaceOp::sign(&founder_sk, namespace_id.into(), vec![], 0, real)
        .expect("sign the real genesis");
    // Self-consistent: the attacker signs, names itself, and presents its own
    // credential, so `signer == founder` holds.
    let forged = SignedNamespaceOp::sign(&attacker_sk, namespace_id.into(), vec![], 0, forged)
        .expect("sign the forged genesis");

    let founder_store = test_store();
    let _ = NamespaceGovernance::new(&founder_store, namespace_id.into())
        .apply_signed_op(&real)
        .expect("the founder applies its own genesis");

    Race {
        namespace_id,
        founder,
        attacker,
        real,
        forged,
        founder_store,
    }
}

/// The invitation the founder's node hands out, as the joiner receives it:
/// serialized to JSON and parsed back. `with_hint = false` is an invitation
/// from a node that predates the field.
fn invitation_as_received(race: &Race, with_hint: bool) -> SignedGroupOpenInvitation {
    let ns_gid = ContextGroupId::from(race.namespace_id);
    let founding = if with_hint {
        NamespaceFoundingRepository::new(&race.founder_store)
            .invitation_hint(&ns_gid)
            .expect("read the founder's record")
    } else {
        None
    };
    assert_eq!(
        founding.is_some(),
        with_hint,
        "precondition: the founder's node holds the pair it founded with"
    );
    let minted = SignedGroupOpenInvitation {
        invitation: GroupInvitationFromAdmin {
            inviter_identity: [0x11; 32].into(),
            group_id: ns_gid,
            expiration_timestamp: 0,
            invitation_nonce: [0x22; 32],
            invited_role: 1,
            admitters: Vec::new(),
        },
        inviter_signature: String::new(),
        inviter_account: Some(race.founder),
        admitter_addrs: Vec::new(),
        application_id: Some([0x33; 32]),
        bytecode_id: None,
        founding,
    };
    let json = serde_json::to_string(&minted).expect("serialize the invitation");
    serde_json::from_str(&json).expect("parse the invitation")
}

/// A fresh joiner: adopts the invitation's hint (as `join_group` does, before
/// subscribing to anything), then receives `ops` in the order given, redelivering
/// the ones that failed until a round applies nothing new.
fn join_and_sync(
    race: &Race,
    invitation: &SignedGroupOpenInvitation,
    ops: &[&SignedNamespaceOp],
) -> Store {
    let store = test_store();
    let ns_gid = ContextGroupId::from(race.namespace_id);
    if let Some(hint) = &invitation.founding {
        let _ = NamespaceFoundingRepository::new(&store)
            .adopt_invitation_hint(&ns_gid, hint)
            .expect("adopt the hint");
    }

    let gov = NamespaceGovernance::new(&store, race.namespace_id.into());
    let mut pending: Vec<&SignedNamespaceOp> = ops.to_vec();
    loop {
        let before = pending.len();
        pending.retain(|op| gov.apply_signed_op(op).is_err());
        if pending.is_empty() || pending.len() == before {
            break;
        }
    }
    store
}

fn admin_of(store: &Store, namespace_id: [u8; 32]) -> Option<calimero_account::AccountId> {
    MetaRepository::new(store)
        .load(&ContextGroupId::from(namespace_id))
        .expect("read meta")
        .map(|meta| meta.admin_identity)
}

#[test]
fn a_joiner_holding_the_founding_pair_converges_to_the_real_founder() {
    let race = race();
    let invitation = invitation_as_received(&race, true);

    let joiner = join_and_sync(&race, &invitation, &[&race.forged, &race.real]);

    assert_eq!(
        admin_of(&joiner, race.namespace_id),
        Some(race.founder),
        "the forged legacy genesis arrived first; the joiner must still end up \
         with the real founder, not the attacker"
    );
    assert_eq!(
        admin_of(&joiner, race.namespace_id),
        admin_of(&race.founder_store, race.namespace_id),
        "joiner and founder agree on who founded the namespace"
    );
    assert_ne!(admin_of(&joiner, race.namespace_id), Some(race.attacker));
}

#[test]
fn delivery_order_does_not_matter_to_a_joiner_holding_the_pair() {
    let race = race();
    let invitation = invitation_as_received(&race, true);

    let joiner = join_and_sync(&race, &invitation, &[&race.real, &race.forged]);

    assert_eq!(admin_of(&joiner, race.namespace_id), Some(race.founder));
}

/// The residual this does not close, pinned so a change to it is deliberate:
/// an invitation without the pair (from a node that predates the field, or one
/// a relayer stripped it from) leaves the joiner on the legacy rules, where
/// whichever genesis lands first wins.
#[test]
fn a_joiner_without_the_pair_keeps_the_legacy_first_genesis_wins_behavior() {
    let race = race();
    let invitation = invitation_as_received(&race, false);

    let joiner = join_and_sync(&race, &invitation, &[&race.forged, &race.real]);

    assert_eq!(admin_of(&joiner, race.namespace_id), Some(race.attacker));
}

/// The hint rides JSON only. Borsh — the join request, and the `MemberJoined`
/// op that embeds the invitation — never carries it, so op bytes, op ids and
/// mixed-version decoding are exactly what they were.
#[test]
fn the_founding_hint_never_reaches_the_borsh_wire() {
    let race = race();
    let with = invitation_as_received(&race, true);
    let without = invitation_as_received(&race, false);

    let bytes = borsh::to_vec(&with).expect("borsh");
    assert_eq!(bytes, borsh::to_vec(&without).expect("borsh"));
    let decoded: SignedGroupOpenInvitation = borsh::from_slice(&bytes).expect("decode");
    assert_eq!(decoded.founding, None);
}
