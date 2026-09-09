//! Byzantine delta-envelope authorship tests (handler signature gate).
//!
//! # What this covers
//!
//! The state-delta handler's apply path opens with an envelope-signature
//! gate: before a delta touches storage it must carry a signature that the
//! *claimed* `author_id`'s key produced over the canonical payload
//! `(context_id, delta_id, author_id, governance_position)`. If the
//! signature is missing, or verification fails, the handler logs and
//! returns WITHOUT applying anything — so a current group-key holder cannot
//! relabel a foreign delta as their own, and a copied signature cannot be
//! grafted onto a different `delta_id`.
//!
//! The pure verifier already has unit coverage in
//! `crates/node/primitives/src/sync/delta_auth.rs`
//! (`verify_rejects_tampered_author` et al.). What was missing is a test at
//! the *gate* level proving the handler's contract: forged author ⇒ verify
//! Err ⇒ NO state mutation ⇒ root_hash unchanged.
//!
//! The real handler entry point (`apply_authorized_state_delta`) is
//! `pub(crate)` and requires a full `NodeClients` / `NodeState` /
//! `NetworkClient` harness that isn't constructible from an integration
//! test. So this test drives the gate faithfully instead: it exercises the
//! PRODUCTION verifier `verify_delta_signature` on realistically-constructed
//! forged deltas, and models the handler's early-return-no-apply contract
//! against a REAL `calimero-storage` Merkle tree (via `SimStorage` inside
//! `SimNode`). The gate helper below is a direct transcription of the
//! handler's signature-gate block (see
//! `crates/node/src/handlers/state_delta/mod.rs`, the `delta_signature`
//! match + `verify_delta_signature` check at the top of
//! `apply_authorized_state_delta`): `None` signature ⇒ reject, verify Err ⇒
//! reject, otherwise apply. Applying is modeled as inserting the delta's
//! entity into the real tree; a rejected delta never inserts, so the
//! independently-computed Merkle root is provably unchanged.
//!
//! # Content-address gate
//!
//! The signature gate above binds `delta_id`, but `parent_ids` ride the wire
//! OUTSIDE the signed payload, so a second gate re-derives the id from the
//! content it claims to address (`CausalDelta::content_address_matches`). The
//! `content_gate_and_apply` helper below transcribes BOTH gates, and the tests
//! using it cover the republish attack: gossipsub messages are publisher-signed,
//! so a delta cannot be tampered with in flight — but any node in the mesh can
//! republish a member's delta under its own libp2p key with `parent_ids`
//! rewritten. Every author-keyed gate still passes, because they all resolve the
//! honest `author_id` rather than the publisher.

use calimero_node_primitives::sync::delta_auth::{delta_signature_payload, verify_delta_signature};
use calimero_primitives::context::ContextId;
use calimero_primitives::crdt::CrdtType;
use calimero_primitives::identity::{PrivateKey, PublicKey};
use calimero_storage::action::Action;
use calimero_storage::address::Id as StorageId;
use calimero_storage::delta::CausalDelta;
use calimero_storage::entities::Metadata;
use calimero_storage::logical_clock::HybridTimestamp;

use crate::sync_sim::prelude::*;

/// Faithful transcription of the handler's signature gate followed by the
/// (modeled) apply. Returns `true` iff the delta was applied — i.e. the gate
/// passed and the entity was written to the real Merkle tree. Returns `false`
/// for every rejection path (missing signature, verification failure),
/// leaving storage untouched.
///
/// Mirrors `apply_authorized_state_delta`'s opening block: a `None` signature
/// is treated as a verification failure (a missing signature cannot prove
/// authorship and is indistinguishable from a stripped one), and any
/// `verify_delta_signature` error short-circuits before storage is touched.
#[allow(clippy::too_many_arguments)]
fn gate_and_apply(
    node: &mut SimNode,
    context_id: ContextId,
    delta_id: [u8; 32],
    author_id: PublicKey,
    signature: Option<[u8; 64]>,
    apply_entity: EntityId,
    apply_data: Vec<u8>,
) -> bool {
    // These fixtures predate the HLC being part of the signed payload; they are
    // about authorship, so they all use the same fixed clock on both sides.
    let hlc = fixture_hlc();
    // Gate step 1: a missing signature cannot prove authorship — reject.
    let sig = match signature {
        Some(s) => s,
        None => return false,
    };

    // Gate step 2: verify the envelope signature against the CLAIMED author.
    // No governance position in these fixtures (non-group context), matching
    // the handler passing `governance_position.as_ref()` (here `None`).
    if verify_delta_signature(context_id, delta_id, author_id, None, hlc, &sig).is_err() {
        return false;
    }

    // Gate passed — apply the delta's action to the real Merkle tree. In the
    // real handler this is the decrypt + DAG-insert tail; here a single
    // entity insert stands in for "state was mutated", which is all the
    // invariant needs to observe (root_hash advances only on a real apply).
    node.insert_entity(apply_entity, apply_data, CrdtType::lww_register());
    true
}

/// One fixed clock for every fixture, so the signed payload and the gate's
/// reconstruction agree. The HLC is part of the signed payload now, so a
/// mismatch here would look like a signature failure rather than a fixture bug.
fn fixture_hlc() -> HybridTimestamp {
    HybridTimestamp::default()
}

/// Deterministic identities for the fixtures.
fn alice() -> (PrivateKey, PublicKey) {
    let sk = PrivateKey::from([11u8; 32]);
    let pk = sk.public_key();
    (sk, pk)
}

fn bob_pk() -> PublicKey {
    PrivateKey::from([22u8; 32]).public_key()
}

/// A node initialized with one entity, so it has a non-zero, well-defined
/// root hash we can watch for (un)changes.
fn initialized_node() -> SimNode {
    let ctx = ContextId::from(SimNode::DEFAULT_CONTEXT_ID);
    let mut node = SimNode::new_in_context("victim", ctx);
    node.insert_entity(
        EntityId::from_u64(1),
        b"genesis".to_vec(),
        CrdtType::lww_register(),
    );
    node
}

/// A1: a delta whose `author_id` is Bob but whose signature was produced by
/// Alice's key (a current key-holder relabeling authorship) is rejected by
/// the gate, and the node applies NOTHING — root_hash is unchanged.
#[test]
fn forged_author_delta_rejected_no_state_mutation() {
    let mut node = initialized_node();
    let context_id = node.context_id();
    let (alice_sk, alice_pk) = alice();
    let delta_id = [0x99u8; 32];

    // Alice legitimately signs the envelope for author = Alice.
    let payload = delta_signature_payload(context_id, delta_id, alice_pk, None, fixture_hlc())
        .expect("payload serializes");
    let alice_sig = alice_sk.sign(&payload).expect("sign").to_bytes();

    let root_before = node.root_hash();

    // Attack: same signature bytes on the wire, but the delta CLAIMS Bob as
    // author. The gate verifies against Bob's key, which never signed this
    // payload — must reject, and must not mutate state.
    let applied = gate_and_apply(
        &mut node,
        context_id,
        delta_id,
        bob_pk(),
        Some(alice_sig),
        EntityId::from_u64(2),
        b"forged".to_vec(),
    );
    assert!(!applied, "forged-author delta must be rejected by the gate");
    assert_eq!(
        node.root_hash(),
        root_before,
        "rejected delta must not mutate storage: root_hash must be unchanged"
    );

    // Control: the SAME signature with the genuine author (Alice) passes the
    // gate and DOES mutate state — proving the fixture's signature is valid
    // and the negative result above is due to the forged author, not a broken
    // signature or an inert apply path.
    let applied_control = gate_and_apply(
        &mut node,
        context_id,
        delta_id,
        alice_pk,
        Some(alice_sig),
        EntityId::from_u64(2),
        b"genuine".to_vec(),
    );
    assert!(applied_control, "genuine-author delta must pass the gate");
    assert_ne!(
        node.root_hash(),
        root_before,
        "a genuine apply must advance the Merkle root"
    );
}

/// A1: a signature Alice produced for `delta_id = D` cannot be grafted onto a
/// different `delta_id = D2` — the payload binds `delta_id`, so verification
/// against D2 fails and the gate applies nothing.
#[test]
fn copied_signature_on_new_delta_id_rejected_no_state_mutation() {
    let mut node = initialized_node();
    let context_id = node.context_id();
    let (alice_sk, alice_pk) = alice();

    let signed_delta_id = [0x01u8; 32];
    let payload =
        delta_signature_payload(context_id, signed_delta_id, alice_pk, None, fixture_hlc())
            .expect("payload serializes");
    let alice_sig = alice_sk.sign(&payload).expect("sign").to_bytes();

    let root_before = node.root_hash();

    // Attack: copy Alice's signature onto a DIFFERENT delta_id, still claiming
    // Alice as author. `delta_id` is bound into the signed payload, so the
    // verifier reconstructs different bytes and rejects.
    let grafted_delta_id = [0x02u8; 32];
    let applied = gate_and_apply(
        &mut node,
        context_id,
        grafted_delta_id,
        alice_pk,
        Some(alice_sig),
        EntityId::from_u64(3),
        b"grafted".to_vec(),
    );
    assert!(
        !applied,
        "a signature copied onto a different delta_id must be rejected"
    );
    assert_eq!(
        node.root_hash(),
        root_before,
        "rejected delta must not mutate storage: root_hash must be unchanged"
    );
}

/// A1: a delta with NO envelope signature is rejected (a missing signature
/// cannot prove authorship), applying nothing.
#[test]
fn missing_signature_rejected_no_state_mutation() {
    let mut node = initialized_node();
    let context_id = node.context_id();
    let (_alice_sk, alice_pk) = alice();

    let root_before = node.root_hash();

    let applied = gate_and_apply(
        &mut node,
        context_id,
        [0x07u8; 32],
        alice_pk,
        None, // stripped / absent signature
        EntityId::from_u64(4),
        b"unsigned".to_vec(),
    );
    assert!(!applied, "an unsigned delta must be rejected by the gate");
    assert_eq!(
        node.root_hash(),
        root_before,
        "rejected delta must not mutate storage: root_hash must be unchanged"
    );
}

// =============================================================================
// Content-address gate (parents authentication)
// =============================================================================

/// One CRDT action, so the fixtures hash a non-empty action list rather than
/// only exercising the parents half of the preimage.
fn fixture_actions() -> Vec<Action> {
    vec![Action::Add {
        id: StorageId::from([0x5A_u8; 32]),
        data: b"payload".to_vec(),
        ancestors: vec![],
        metadata: Metadata::default(),
    }]
}

/// Faithful transcription of the handler's gate chain as it now stands:
/// envelope signature first (authorship), then the content address (the fields
/// the signature does not cover). Mirrors the block added to
/// `apply_authorized_state_delta` after `decrypt_delta_actions` — the check runs
/// post-decrypt because it needs `actions`, and pre-DAG-insert because a
/// disconnected head is exactly what it prevents.
///
/// Returns `true` iff BOTH gates passed and the entity reached the real Merkle
/// tree. `parents`/`actions`/`hlc` are the values that arrived on the wire; the
/// attacker controls them, and `delta_id` is what the signature covers.
#[allow(clippy::too_many_arguments)]
fn content_gate_and_apply(
    node: &mut SimNode,
    context_id: ContextId,
    delta_id: [u8; 32],
    author_id: PublicKey,
    signature: Option<[u8; 64]>,
    parents: &[[u8; 32]],
    actions: &[Action],
    hlc: &HybridTimestamp,
    apply_entity: EntityId,
    apply_data: Vec<u8>,
) -> bool {
    // Gate 1: authorship (unchanged — see `gate_and_apply`).
    let sig = match signature {
        Some(s) => s,
        None => return false,
    };
    if verify_delta_signature(context_id, delta_id, author_id, None, *hlc, &sig).is_err() {
        return false;
    }

    // Gate 2: does the signed id actually address the content that arrived?
    if !CausalDelta::content_address_matches(&delta_id, parents, actions, hlc) {
        return false;
    }

    node.insert_entity(apply_entity, apply_data, CrdtType::lww_register());
    true
}

/// Control: an honest delta — id derived from the parents and actions it
/// carries, signed by its real author — passes both gates and advances the root.
/// Without this the negative tests below could pass on a broken fixture.
#[test]
fn honest_delta_passes_both_gates_and_mutates_state() {
    let mut node = initialized_node();
    let context_id = node.context_id();
    let (alice_sk, alice_pk) = alice();

    let parents = vec![[0x31_u8; 32]];
    let actions = fixture_actions();
    let hlc = fixture_hlc();
    let delta_id = CausalDelta::compute_id(&parents, &actions, &hlc);

    let payload = delta_signature_payload(context_id, delta_id, alice_pk, None, fixture_hlc())
        .expect("payload serializes");
    let sig = alice_sk.sign(&payload).expect("sign").to_bytes();

    let root_before = node.root_hash();
    let applied = content_gate_and_apply(
        &mut node,
        context_id,
        delta_id,
        alice_pk,
        Some(sig),
        &parents,
        &actions,
        &hlc,
        EntityId::from_u64(10),
        b"honest".to_vec(),
    );
    assert!(applied, "an honest delta must pass both gates");
    assert_ne!(
        node.root_hash(),
        root_before,
        "a genuine apply must advance the Merkle root"
    );
}

/// The #3540 attack. A republisher takes Alice's delta VERBATIM — same
/// `delta_id`, same genuine signature, same honest `author_id` — and strips
/// `parent_ids` to empty. The signature gate cannot see it (parents are not in
/// the signed payload) and every author-keyed gate resolves Alice, a full
/// member. Empty parents are the worst case: they hit the vacuously-true branch
/// of `DagStore::can_apply` and apply immediately as a disconnected head,
/// bypassing missing-parent detection. The content gate must catch it.
#[test]
fn republished_delta_with_emptied_parents_rejected_no_state_mutation() {
    let mut node = initialized_node();
    let context_id = node.context_id();
    let (alice_sk, alice_pk) = alice();

    let parents = vec![[0x31_u8; 32]];
    let actions = fixture_actions();
    let hlc = fixture_hlc();
    let delta_id = CausalDelta::compute_id(&parents, &actions, &hlc);

    // Alice's real signature over her real delta — the attacker does not forge
    // anything, it replays what Alice published.
    let payload = delta_signature_payload(context_id, delta_id, alice_pk, None, fixture_hlc())
        .expect("payload serializes");
    let sig = alice_sk.sign(&payload).expect("sign").to_bytes();

    let root_before = node.root_hash();
    let applied = content_gate_and_apply(
        &mut node,
        context_id,
        delta_id,
        alice_pk,
        Some(sig),
        &[], // <-- the only tampered field
        &actions,
        &hlc,
        EntityId::from_u64(11),
        b"disconnected-head".to_vec(),
    );
    assert!(
        !applied,
        "a delta whose parents were stripped to empty must be rejected"
    );
    assert_eq!(
        node.root_hash(),
        root_before,
        "rejected delta must not mutate storage: root_hash must be unchanged"
    );
}

/// Same attack, non-empty variant: re-parenting moves the causal cut that
/// at-cut authorization resolves against, so a swapped parent set must be
/// rejected too — the gate is about content-addressing, not just emptiness.
#[test]
fn republished_delta_with_swapped_parents_rejected_no_state_mutation() {
    let mut node = initialized_node();
    let context_id = node.context_id();
    let (alice_sk, alice_pk) = alice();

    let parents = vec![[0x31_u8; 32]];
    let actions = fixture_actions();
    let hlc = fixture_hlc();
    let delta_id = CausalDelta::compute_id(&parents, &actions, &hlc);

    let payload = delta_signature_payload(context_id, delta_id, alice_pk, None, fixture_hlc())
        .expect("payload serializes");
    let sig = alice_sk.sign(&payload).expect("sign").to_bytes();

    let root_before = node.root_hash();
    let applied = content_gate_and_apply(
        &mut node,
        context_id,
        delta_id,
        alice_pk,
        Some(sig),
        &[[0x77_u8; 32]], // a different, still non-empty, parent
        &actions,
        &hlc,
        EntityId::from_u64(12),
        b"reparented".to_vec(),
    );
    assert!(!applied, "a re-parented delta must be rejected");
    assert_eq!(
        node.root_hash(),
        root_before,
        "rejected delta must not mutate storage: root_hash must be unchanged"
    );
}
