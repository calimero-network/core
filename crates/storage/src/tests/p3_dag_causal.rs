//! Tests for phase **P3** of [#2233](https://github.com/calimero-network/core/issues/2233):
//!
//! - **Verifier swap.** When `ApplyContext` carries DAG-causal information,
//!   `Interface::apply_action` validates `Shared` signatures against
//!   `writers_at(causal_parents)` (per ADR 0001) instead of stored writers.
//!   When the context is empty, behavior matches v2 exactly.
//! - **Write hook.** Successful applies of `Shared` rotations append a
//!   [`RotationLogEntry`]. Value-writes (writers unchanged) and ctx without
//!   `delta_id`/`delta_hlc` are no-ops.

use core::num::NonZeroU128;
use std::collections::BTreeSet;

use calimero_primitives::identity::PublicKey;
use ed25519_dalek::{Signer, SigningKey};

use crate::action::Action;
use crate::address::Id;
use crate::entities::{ChildInfo, Metadata, SignatureData, StorageType};
use crate::env;
use crate::index::Index;
use crate::interface::{ApplyContext, Interface};
use crate::logical_clock::{HybridTimestamp, Timestamp, ID, NTP64};
use crate::rotation_log::{self, RotationLogEntry};
use crate::store::{MockedStorage, StorageAdaptor};

// Each test uses a unique mocked-storage scope so they don't bleed into each
// other (the mock store is a thread-local BTreeMap keyed on (scope, key)).
type S<const SCOPE: usize> = MockedStorage<SCOPE>;

// =============================================================================
// Helpers
// =============================================================================

fn make_signing_key(seed: u8) -> SigningKey {
    SigningKey::from_bytes(&[seed; 32])
}

fn pubkey_of(sk: &SigningKey) -> PublicKey {
    PublicKey::from(*sk.verifying_key().as_bytes())
}

fn hlc(ns: u64) -> HybridTimestamp {
    let node_id = ID::from(NonZeroU128::new(1).unwrap());
    HybridTimestamp::new(Timestamp::new(NTP64(ns), node_id))
}

/// Returns an HLC nanosecond value rooted at "now" plus `step` seconds.
/// Use this so sequential actions in a test always have strictly-increasing
/// nonces without depending on wall-clock advancement between calls.
fn hlc_at(step: u64) -> u64 {
    env::time_now().saturating_add(step.saturating_mul(1_000_000_000))
}

/// Pre-create the root index entry so non-root child entities can be
/// added/updated by `apply_action` without tripping `IndexNotFound`.
/// Returns the root `ChildInfo` ready to embed in an action's `ancestors`.
fn setup_root<S: StorageAdaptor>() -> ChildInfo {
    let root_id = Id::root();
    let root_meta = Metadata::default();
    Index::<S>::add_root(ChildInfo::new(root_id, [0; 32], root_meta.clone())).unwrap();
    ChildInfo::new(root_id, [0; 32], root_meta)
}

/// Construct a signed Shared action. `signer_sk` must be in `writers` (or the
/// caller is intentionally testing a forged action).
///
/// `hlc_ns` is used as BOTH the action's `updated_at` and the signature's
/// `nonce` — matching production where `sign_authorized_actions` stamps both
/// with the action's HLC. Tests must pass strictly increasing values across
/// sequential calls to satisfy the per-entity replay-protection check.
fn build_signed_shared_action(
    add: bool,
    id: Id,
    data: Vec<u8>,
    writers: BTreeSet<PublicKey>,
    hlc_ns: u64,
    signer_sk: &SigningKey,
    ancestors: Vec<ChildInfo>,
) -> Action {
    let metadata = Metadata {
        created_at: hlc_ns,
        updated_at: hlc_ns.into(),
        storage_type: StorageType::Shared {
            writers,
            signature_data: Some(SignatureData {
                signature: [0; 64], // placeholder, filled in below
                nonce: hlc_ns,
                signer: Some(pubkey_of(signer_sk)),
            }),
        },
        crdt_type: None,
        field_name: None,
    };
    let mut action = if add {
        Action::Add {
            id,
            data,
            ancestors,
            metadata,
        }
    } else {
        Action::Update {
            id,
            data,
            ancestors,
            metadata,
        }
    };
    let payload = action.payload_for_signing();
    let signature = signer_sk.sign(&payload).to_bytes();
    let metadata_mut = match &mut action {
        Action::Add { metadata, .. } | Action::Update { metadata, .. } => metadata,
        _ => unreachable!(),
    };
    if let StorageType::Shared {
        signature_data: Some(sd),
        ..
    } = &mut metadata_mut.storage_type
    {
        sd.signature = signature;
    }
    action
}

fn empty_ctx<'a>() -> ApplyContext<'a> {
    ApplyContext {
        causal_parents: &[],
        delta_id: None,
        delta_hlc: None,
        happens_before: None,
    }
}

fn dag_ctx<'a>(
    parents: &'a [[u8; 32]],
    delta_id: [u8; 32],
    delta_hlc_ns: u64,
    happens_before: &'a dyn Fn(&[u8; 32], &[u8; 32]) -> bool,
) -> ApplyContext<'a> {
    ApplyContext {
        causal_parents: parents,
        delta_id: Some(delta_id),
        delta_hlc: Some(hlc(delta_hlc_ns)),
        happens_before: Some(happens_before),
    }
}

// Convenient per-test entity id derived from the scope so each test gets a
// distinct, deterministic id (avoids any cross-test interference even if
// scopes were reused).
fn entity_id(seed: u8) -> Id {
    Id::new([seed; 32])
}

// =============================================================================
// Verifier-swap tests
// =============================================================================

/// Baseline: with no DAG context, the verifier behaves exactly like v2 — sig
/// must verify against the stored writer set, falling back to the action's
/// claim when the entity doesn't exist yet (bootstrap).
#[test]
fn verifier_without_dag_context_uses_stored_writers() {
    env::reset_for_testing();
    let root = setup_root::<S<400>>();

    let alice_sk = make_signing_key(0xA1);
    let alice = pubkey_of(&alice_sk);
    let id = entity_id(0x40);

    let bootstrap = build_signed_shared_action(
        true,
        id,
        b"hello".to_vec(),
        [alice].into_iter().collect(),
        hlc_at(0),
        &alice_sk,
        vec![root.clone()],
    );
    Interface::<S<400>>::apply_action(bootstrap, empty_ctx())
        .expect("bootstrap accepted with v2 fallback");

    let update = build_signed_shared_action(
        false,
        id,
        b"world".to_vec(),
        [alice].into_iter().collect(),
        hlc_at(1),
        &alice_sk,
        vec![],
    );
    Interface::<S<400>>::apply_action(update, empty_ctx()).expect("update by writer accepted");
}

/// Action signed by a non-writer is rejected by the v2 path.
#[test]
fn verifier_without_dag_context_rejects_non_writer() {
    env::reset_for_testing();
    let root = setup_root::<S<401>>();

    let alice_sk = make_signing_key(0xA2);
    let bob_sk = make_signing_key(0xB2);
    let alice = pubkey_of(&alice_sk);
    let id = entity_id(0x41);

    let bootstrap = build_signed_shared_action(
        true,
        id,
        b"v0".to_vec(),
        [alice].into_iter().collect(),
        hlc_at(0),
        &alice_sk,
        vec![root.clone()],
    );
    Interface::<S<401>>::apply_action(bootstrap, empty_ctx()).unwrap();

    // Bob is not in the stored writer set — must reject.
    let forged = build_signed_shared_action(
        false,
        id,
        b"v1".to_vec(),
        [alice].into_iter().collect(),
        hlc_at(1),
        &bob_sk,
        vec![],
    );
    let result = Interface::<S<401>>::apply_action(forged, empty_ctx());
    assert!(matches!(
        result,
        Err(crate::interface::StorageError::InvalidSignature)
    ));
}

/// **The partition-correctness fix.** A pre-populated rotation log says the
/// writer set as-of `D1` is `{Alice}`. The stored entity (simulating
/// divergent state from a partition) has `{Bob}`. An action signed by Alice
/// with `causal_parents = [D1]` must be accepted: per ADR 0001 the verifier
/// consults the rotation log, not stored.
#[test]
fn verifier_with_dag_context_uses_rotation_log() {
    env::reset_for_testing();
    let root = setup_root::<S<402>>();

    let alice_sk = make_signing_key(0xA3);
    let bob_sk = make_signing_key(0xB3);
    let alice = pubkey_of(&alice_sk);
    let bob = pubkey_of(&bob_sk);
    let id = entity_id(0x42);

    // Bootstrap with Bob as the stored writer.
    let bootstrap = build_signed_shared_action(
        true,
        id,
        b"divergent".to_vec(),
        [bob].into_iter().collect(),
        hlc_at(0),
        &bob_sk,
        vec![root.clone()],
    );
    Interface::<S<402>>::apply_action(bootstrap, empty_ctx()).unwrap();

    // Pre-populate the rotation log: as-of delta D1, the writer set was {Alice}.
    let d1 = [0xD1; 32];
    rotation_log::append::<S<402>>(
        id,
        RotationLogEntry {
            delta_id: d1,
            delta_hlc: hlc(hlc_at(0)),
            signer: Some(alice),
            new_writers: [alice].into_iter().collect(),
            writers_nonce: 1,
        },
    )
    .unwrap();

    // Alice signs an Update; ctx points at D1 as a causal parent.
    let action = build_signed_shared_action(
        false,
        id,
        b"alice-update".to_vec(),
        [alice].into_iter().collect(),
        hlc_at(2),
        &alice_sk,
        vec![],
    );
    let parents = [d1];
    // Direct parent-match in writers_at handles `delta_id == d1`; no DAG ancestry needed here.
    let happens_before: &dyn Fn(&[u8; 32], &[u8; 32]) -> bool = &|_, _| false;
    let ctx = dag_ctx(&parents, [0xD2; 32], hlc_at(2), happens_before);

    // Without P3 this would be rejected (sig vs stored {Bob} fails).
    // With P3 it's accepted because writers_at returns {Alice}.
    Interface::<S<402>>::apply_action(action, ctx)
        .expect("DAG-causal verifier accepts Alice — she's the writer as-of D1");
}

/// Even with DAG context, an action signed by someone *outside* the causal
/// writer set is rejected.
#[test]
fn verifier_with_dag_context_rejects_non_causal_writer() {
    env::reset_for_testing();
    let root = setup_root::<S<403>>();

    let alice_sk = make_signing_key(0xA4);
    let bob_sk = make_signing_key(0xB4);
    let mallory_sk = make_signing_key(0xC4);
    let alice = pubkey_of(&alice_sk);
    let bob = pubkey_of(&bob_sk);
    let id = entity_id(0x43);

    let bootstrap = build_signed_shared_action(
        true,
        id,
        b"v0".to_vec(),
        [bob].into_iter().collect(),
        hlc_at(0),
        &bob_sk,
        vec![root.clone()],
    );
    Interface::<S<403>>::apply_action(bootstrap, empty_ctx()).unwrap();

    let d1 = [0xD1; 32];
    rotation_log::append::<S<403>>(
        id,
        RotationLogEntry {
            delta_id: d1,
            delta_hlc: hlc(hlc_at(0)),
            signer: Some(alice),
            new_writers: [alice].into_iter().collect(),
            writers_nonce: 1,
        },
    )
    .unwrap();

    // Mallory is in neither stored {Bob} nor causal {Alice}.
    let forged = build_signed_shared_action(
        false,
        id,
        b"forged".to_vec(),
        [alice].into_iter().collect(),
        hlc_at(2),
        &mallory_sk,
        vec![],
    );
    let parents = [d1];
    // Direct parent-match in writers_at handles `delta_id == d1`; no DAG ancestry needed here.
    let happens_before: &dyn Fn(&[u8; 32], &[u8; 32]) -> bool = &|_, _| false;
    let ctx = dag_ctx(&parents, [0xD2; 32], hlc_at(2), happens_before);

    let result = Interface::<S<403>>::apply_action(forged, ctx);
    assert!(matches!(
        result,
        Err(crate::interface::StorageError::InvalidSignature)
    ));
}

// =============================================================================
// Write-hook tests
// =============================================================================

/// Bootstrap with full DAG context appends one rotation log entry.
#[test]
fn write_hook_appends_on_bootstrap_with_ctx() {
    env::reset_for_testing();
    let root = setup_root::<S<404>>();

    let alice_sk = make_signing_key(0xA5);
    let alice = pubkey_of(&alice_sk);
    let id = entity_id(0x44);

    let bootstrap = build_signed_shared_action(
        true,
        id,
        b"v0".to_vec(),
        [alice].into_iter().collect(),
        hlc_at(0),
        &alice_sk,
        vec![root.clone()],
    );
    let happens_before: &dyn Fn(&[u8; 32], &[u8; 32]) -> bool = &|_, _| false;
    let ctx = dag_ctx(&[], [0xAA; 32], hlc_at(0), happens_before);
    Interface::<S<404>>::apply_action(bootstrap, ctx).unwrap();

    let log = rotation_log::load::<S<404>>(id)
        .unwrap()
        .expect("rotation log exists after Shared apply with delta ctx");
    assert_eq!(log.entries.len(), 1);
    assert_eq!(log.entries[0].delta_id, [0xAA; 32]);
    assert_eq!(log.entries[0].signer, Some(alice));
    assert_eq!(log.entries[0].new_writers, [alice].into_iter().collect());
}

/// Same bootstrap but with empty ctx (no delta_id) — the log stays empty.
/// Production sync paths behave like this until the WASM ABI extension lands.
#[test]
fn write_hook_skips_when_ctx_lacks_delta_id() {
    env::reset_for_testing();
    let root = setup_root::<S<405>>();

    let alice_sk = make_signing_key(0xA6);
    let alice = pubkey_of(&alice_sk);
    let id = entity_id(0x45);

    let bootstrap = build_signed_shared_action(
        true,
        id,
        b"v0".to_vec(),
        [alice].into_iter().collect(),
        hlc_at(0),
        &alice_sk,
        vec![root.clone()],
    );
    Interface::<S<405>>::apply_action(bootstrap, empty_ctx()).unwrap();

    assert_eq!(rotation_log::load::<S<405>>(id).unwrap(), None);
}

/// Value-write (writer set unchanged) does not append an entry.
#[test]
fn write_hook_skips_when_writers_unchanged() {
    env::reset_for_testing();
    let root = setup_root::<S<406>>();

    let alice_sk = make_signing_key(0xA7);
    let alice = pubkey_of(&alice_sk);
    let id = entity_id(0x46);

    let happens_before: &dyn Fn(&[u8; 32], &[u8; 32]) -> bool = &|_, _| false;

    let bootstrap = build_signed_shared_action(
        true,
        id,
        b"v0".to_vec(),
        [alice].into_iter().collect(),
        hlc_at(0),
        &alice_sk,
        vec![root.clone()],
    );
    Interface::<S<406>>::apply_action(
        bootstrap,
        dag_ctx(&[], [0xBB; 32], hlc_at(0), happens_before),
    )
    .unwrap();
    assert_eq!(
        rotation_log::load::<S<406>>(id)
            .unwrap()
            .unwrap()
            .entries
            .len(),
        1
    );

    // Value-write with the same writer set → log stays at 1 entry.
    let value_write = build_signed_shared_action(
        false,
        id,
        b"v1".to_vec(),
        [alice].into_iter().collect(), // same set
        hlc_at(1),
        &alice_sk,
        vec![],
    );
    Interface::<S<406>>::apply_action(
        value_write,
        dag_ctx(&[], [0xCC; 32], hlc_at(1), happens_before),
    )
    .unwrap();

    let log = rotation_log::load::<S<406>>(id).unwrap().unwrap();
    assert_eq!(log.entries.len(), 1, "value-write did not append");
}

/// Genuine rotation (writer set changes) appends a second entry.
#[test]
fn write_hook_appends_on_writer_set_change() {
    env::reset_for_testing();
    let root = setup_root::<S<407>>();

    let alice_sk = make_signing_key(0xA8);
    let bob_sk = make_signing_key(0xB8);
    let alice = pubkey_of(&alice_sk);
    let bob = pubkey_of(&bob_sk);
    let id = entity_id(0x47);

    let happens_before: &dyn Fn(&[u8; 32], &[u8; 32]) -> bool = &|_, _| false;

    let bootstrap = build_signed_shared_action(
        true,
        id,
        b"v0".to_vec(),
        [alice].into_iter().collect(),
        hlc_at(0),
        &alice_sk,
        vec![root.clone()],
    );
    Interface::<S<407>>::apply_action(
        bootstrap,
        dag_ctx(&[], [0xD0; 32], hlc_at(0), happens_before),
    )
    .unwrap();

    // Alice rotates: now writers = {Alice, Bob}.
    let rotation = build_signed_shared_action(
        false,
        id,
        b"v0".to_vec(),
        [alice, bob].into_iter().collect(),
        hlc_at(1),
        &alice_sk,
        vec![],
    );
    Interface::<S<407>>::apply_action(
        rotation,
        dag_ctx(&[[0xD0; 32]], [0xD1; 32], hlc_at(1), happens_before),
    )
    .unwrap();

    let log = rotation_log::load::<S<407>>(id).unwrap().unwrap();
    assert_eq!(log.entries.len(), 2);
    assert_eq!(log.entries[1].delta_id, [0xD1; 32]);
    assert_eq!(
        log.entries[1].new_writers,
        [alice, bob].into_iter().collect()
    );
}

// =============================================================================
// P4-impl coverage: ADR Example D (write vs rotate on the same entity)
// =============================================================================
//
// The other ADR examples (A sequential, B concurrent siblings HLC, C HLC tie)
// are exercised in `rotation_log::tests`. Example D — a value-write signed by
// a pre-rotation writer must still verify after the rotation lands locally —
// is the central guarantee that makes concurrent operation safe, and it
// involves the full apply_action verifier swap, so it lives here next to
// the rest of the P3 verifier coverage.

/// ADR Example D: pre-rotation value-write is accepted even after the
/// rotation that removes the signer is applied locally. The verifier must
/// consult `writers_at(value_write.parents)`, NOT the post-merge writer set.
#[test]
fn adr_example_d_pre_rotation_write_accepted_after_rotation() {
    env::reset_for_testing();
    let root = setup_root::<S<420>>();

    let alice_sk = make_signing_key(0xA9);
    let bob_sk = make_signing_key(0xB9);
    let alice = pubkey_of(&alice_sk);
    let bob = pubkey_of(&bob_sk);
    let id = entity_id(0x60);

    // D_root: writers = {Alice, Bob}. Bootstrap so the entity exists locally.
    let d_root = [0xD0; 32];
    let bootstrap = build_signed_shared_action(
        true,
        id,
        b"hello".to_vec(),
        [alice, bob].into_iter().collect(),
        hlc_at(0),
        &alice_sk,
        vec![root.clone()],
    );
    let happens_before_simple: &dyn Fn(&[u8; 32], &[u8; 32]) -> bool = &|_, _| false;
    Interface::<S<420>>::apply_action(
        bootstrap,
        dag_ctx(&[], d_root, hlc_at(0), happens_before_simple),
    )
    .unwrap();

    // D1 (concurrent sibling of D_root from D2's perspective): Alice rotates
    // Bob out → writers = {Alice}. Apply this first.
    let d1 = [0xD1; 32];
    let rotation = build_signed_shared_action(
        false,
        id,
        b"hello".to_vec(),
        [alice].into_iter().collect(),
        hlc_at(1),
        &alice_sk,
        vec![],
    );
    Interface::<S<420>>::apply_action(
        rotation,
        dag_ctx(&[d_root], d1, hlc_at(1), happens_before_simple),
    )
    .unwrap();

    // Sanity: the local stored writer set is now {Alice} and the rotation log
    // has two entries (bootstrap + rotation).
    let log = rotation_log::load::<S<420>>(id).unwrap().unwrap();
    assert_eq!(log.entries.len(), 2);

    // D2 (concurrent sibling of D1): Bob writes "world" against the writer
    // set he saw — {Alice, Bob}. From Bob's local view this is valid; D2's
    // parent is D_root, NOT D1. Carry that into ctx.causal_parents.
    let d2 = [0xD2; 32];
    let bob_write = build_signed_shared_action(
        false,
        id,
        b"world".to_vec(),
        [alice, bob].into_iter().collect(), // Bob's view of writers
        hlc_at(2),
        &bob_sk,
        vec![],
    );

    // happens_before: D_root precedes everything; D1 and D2 are siblings so
    // neither precedes the other.
    let happens_before: &dyn Fn(&[u8; 32], &[u8; 32]) -> bool = &|a, b| {
        // D_root happens-before D1 and D2.
        if *a == d_root && (*b == d1 || *b == d2) {
            return true;
        }
        false
    };
    let parents = [d_root];
    let ctx = dag_ctx(&parents, d2, hlc_at(2), happens_before);

    // Crucial: even though stored writers (post-D1) is {Alice}, D2 is causally
    // a sibling of D1 — it never saw the rotation. writers_at(D2.parents=[D_root])
    // returns the bootstrap writer set {Alice, Bob}, so Bob's signature
    // verifies. Without P3 this would fail (sig vs stored {Alice}).
    Interface::<S<420>>::apply_action(bob_write, ctx).expect(
        "ADR Example D: pre-rotation write by Bob accepted because writers_at \
         (causal parents of D2) includes Bob, even though stored writers no longer do",
    );
}

/// Inverse of Example D: a write whose causal parents *include* the rotation
/// (i.e., the writer saw the rotation and chose to write anyway) must be
/// rejected if the signer is no longer in the writer set as-of those parents.
#[test]
fn write_post_rotation_by_removed_writer_rejected() {
    env::reset_for_testing();
    let root = setup_root::<S<421>>();

    let alice_sk = make_signing_key(0xAA);
    let bob_sk = make_signing_key(0xBA);
    let alice = pubkey_of(&alice_sk);
    let bob = pubkey_of(&bob_sk);
    let id = entity_id(0x61);

    let d_root = [0xD0; 32];
    let bootstrap = build_signed_shared_action(
        true,
        id,
        b"hello".to_vec(),
        [alice, bob].into_iter().collect(),
        hlc_at(0),
        &alice_sk,
        vec![root.clone()],
    );
    let happens_before_simple: &dyn Fn(&[u8; 32], &[u8; 32]) -> bool = &|_, _| false;
    Interface::<S<421>>::apply_action(
        bootstrap,
        dag_ctx(&[], d_root, hlc_at(0), happens_before_simple),
    )
    .unwrap();

    // D1: Alice rotates Bob out.
    let d1 = [0xD1; 32];
    let rotation = build_signed_shared_action(
        false,
        id,
        b"hello".to_vec(),
        [alice].into_iter().collect(),
        hlc_at(1),
        &alice_sk,
        vec![],
    );
    Interface::<S<421>>::apply_action(
        rotation,
        dag_ctx(&[d_root], d1, hlc_at(1), happens_before_simple),
    )
    .unwrap();

    // D2 has D1 as a parent — Bob saw the rotation and tries to write anyway.
    let d2 = [0xD2; 32];
    let bob_write_post = build_signed_shared_action(
        false,
        id,
        b"world".to_vec(),
        [alice].into_iter().collect(), // Bob acknowledges the rotation in his claim
        hlc_at(2),
        &bob_sk,
        vec![],
    );
    let happens_before: &dyn Fn(&[u8; 32], &[u8; 32]) -> bool = &|a, b| {
        // D_root → D1 → D2. (D_root → D2 transitively.)
        matches!(
            (*a, *b),
            (x, y) if x == d_root && (y == d1 || y == d2)
                || (x == d1 && y == d2)
        )
    };
    let parents = [d1];
    let ctx = dag_ctx(&parents, d2, hlc_at(2), happens_before);

    // writers_at(D2.parents=[D1]) returns {Alice} — Bob is no longer a writer
    // and his signature must fail.
    let result = Interface::<S<421>>::apply_action(bob_write_post, ctx);
    assert!(
        matches!(
            result,
            Err(crate::interface::StorageError::InvalidSignature)
        ),
        "post-rotation write by removed writer must be rejected; got {:?}",
        result
    );
}
