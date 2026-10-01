//! Tests for phase **P3** of [#2233](https://github.com/calimero-network/core/issues/2233):
//! the **verifier swap**. When `ApplyContext` carries the writers the node resolved for the
//! delta's governance position (`effective_writers`), `Interface::apply_action` validates
//! `Shared` signatures against that set instead of the stored one. With none, the stored set
//! stands.
//!
//! A rotation is a governance op, so the writers a delta is judged against are the governance
//! fold's answer at its position. These tests play that fold with a real projection
//! ([`RotationWorld`]) and hand storage what the node would compute.

use calimero_storage::address::Id;
use calimero_storage::entities::{full_mask, ChildInfo, Metadata};
use calimero_storage::index::Index;
use calimero_storage::interface::{ApplyContext, Interface, StorageError};
use calimero_storage::shared_writers::{CellWriters, Writers};
use calimero_storage::store::{MockedStorage, StorageAdaptor};
use calimero_storage::tests::common::{
    account_of_key, apply_ctx_for, build_signed_shared_action, cell_at, pubkey_of,
};
use ed25519_dalek::SigningKey;

use calimero_context::test_support::RotationWorld;

// =============================================================================
// Harness
// =============================================================================

// Signing/action builders (`build_signed_shared_action`, `pubkey_of`) come from
// `calimero_storage::tests::common`.

// Each test uses a unique mocked-storage scope so they don't bleed into each
// other (the mock store is a thread-local BTreeMap keyed on (scope, key)).
type S<const SCOPE: usize> = MockedStorage<SCOPE>;

fn make_signing_key(seed: u8) -> SigningKey {
    SigningKey::from_bytes(&[seed; 32])
}

/// Returns a HLC nanosecond value rooted at "now" plus `step` seconds.
fn hlc_at(step: u64) -> u64 {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    now.saturating_add(step.saturating_mul(1_000_000_000))
}

/// Pre-create the root index entry so non-root child entities can be
/// added/updated by `apply_action` without tripping `IndexNotFound`.
///
/// Returns a `ChildInfo` whose `merkle_hash` matches what's actually
/// stored after `add_root` (post-`full_hash_from_trie`),
/// so callers can use the returned value as an ancestor without
/// tripping the v2 `verify_ancestor_integrity` check.
fn setup_root<S: StorageAdaptor>() -> ChildInfo {
    let root_id = Id::root();
    let root_meta = Metadata::default();
    Index::<S>::add_root(ChildInfo::new(root_id, [0; 32], root_meta.clone())).unwrap();
    let (full_hash, _) = Index::<S>::get_hashes_for(root_id).unwrap().unwrap();
    ChildInfo::new(root_id, full_hash, root_meta)
}

/// What the node computes for a delta's cell at its governance `position`: the set a rotation
/// left, or nothing while the cell stands at the set its id commits to.
fn writers_at(world: &RotationWorld, cell: Id, position: &[[u8; 32]]) -> Option<Writers> {
    let folded = world
        .projections
        .read()
        .unwrap()
        .shared_writers_at_cut(&world.store, &world.context, cell, position)
        .expect("the position is folded");
    match folded {
        CellWriters::Genesis => None,
        CellWriters::Rotated(writers) => Some(writers),
    }
}

/// An `ApplyContext` as the node builds it for a delta signed by `signer_account` at `position`.
fn ctx_at(
    world: &RotationWorld,
    cell: Id,
    position: &[[u8; 32]],
    signer_account: calimero_account::AccountId,
) -> ApplyContext {
    ApplyContext {
        effective_writers: writers_at(world, cell, position),
        // Per-apply: the writer set is the same at one cut, but WHO is writing is not, and
        // these tests turn on exactly that difference.
        signer_account: Some(signer_account),
    }
}

/// An `ApplyContext` for tests that take no writers from governance.
fn hook_ctx(signer: &SigningKey) -> ApplyContext {
    ApplyContext {
        effective_writers: None,
        signer_account: Some(account_of_key(signer)),
    }
}

/// `owners` joined to one group, each with the account they speak for there.
fn world_of(owners: &[&SigningKey]) -> RotationWorld {
    let keys: Vec<_> = owners.iter().map(|sk| pubkey_of(sk)).collect();
    RotationWorld::new(&keys)
}

// =============================================================================
// Verifier-swap tests
// =============================================================================

/// Baseline: with no DAG context, the verifier behaves exactly like v2 — sig
/// must verify against the stored writer set, falling back to the action's
/// claim when the entity doesn't exist yet (bootstrap).
#[test]
fn verifier_without_dag_context_uses_stored_writers() {
    let root = setup_root::<S<6400>>();

    let alice_sk = make_signing_key(0xA1);
    let alice = account_of_key(&alice_sk);
    let id = cell_at(0x40, &[alice].into_iter().collect());

    let bootstrap = build_signed_shared_action(
        true,
        id,
        b"hello".to_vec(),
        [alice].into_iter().collect(),
        hlc_at(0),
        &alice_sk,
        vec![root.clone()],
    );
    Interface::<S<6400>>::apply_action(bootstrap, &apply_ctx_for(account_of_key(&alice_sk)))
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
    Interface::<S<6400>>::apply_action(update, &apply_ctx_for(account_of_key(&alice_sk)))
        .expect("update by writer accepted");
}

/// Action signed by a non-writer is rejected by the v2 path.
#[test]
fn verifier_without_dag_context_rejects_non_writer() {
    let root = setup_root::<S<6401>>();

    let alice_sk = make_signing_key(0xA2);
    let bob_sk = make_signing_key(0xB2);
    let alice = account_of_key(&alice_sk);
    let id = cell_at(0x41, &[alice].into_iter().collect());

    let bootstrap = build_signed_shared_action(
        true,
        id,
        b"v0".to_vec(),
        [alice].into_iter().collect(),
        hlc_at(0),
        &alice_sk,
        vec![root.clone()],
    );
    Interface::<S<6401>>::apply_action(bootstrap, &apply_ctx_for(account_of_key(&alice_sk)))
        .unwrap();

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
    // Bob's key resolves to Bob's account — an honest resolution — so the refusal
    // below is "that account is not a writer", not "unresolvable signer".
    let result =
        Interface::<S<6401>>::apply_action(forged, &apply_ctx_for(account_of_key(&bob_sk)));
    assert!(matches!(result, Err(StorageError::InvalidSignature)));
}

/// **The partition-correctness fix.** The stored entity (simulating divergent state from a
/// partition) has `{Bob}`, and governance rotated the cell to `{Alice}` at `R1`. An action signed
/// by Alice at a position that includes `R1` must be accepted: the verifier consults the
/// governance fold at the delta's position, not the stored set.
#[test]
fn verifier_with_a_governance_position_uses_the_writers_at_it() {
    let root = setup_root::<S<6402>>();

    let alice_sk = make_signing_key(0xA3);
    let bob_sk = make_signing_key(0xB3);
    let world = world_of(&[&alice_sk, &bob_sk]);
    let (alice, bob) = (
        world.account(&pubkey_of(&alice_sk)),
        world.account(&pubkey_of(&bob_sk)),
    );
    let id = cell_at(0x42, &[bob].into_iter().collect());

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
    Interface::<S<6402>>::apply_action(bootstrap, &apply_ctx_for(bob)).unwrap();

    // Governance: Bob rotates the cell to {Alice} at R1.
    let r1 = [0xD1; 32];
    world.rotate(
        &pubkey_of(&bob_sk),
        id,
        r1,
        &world.joined(),
        full_mask([bob].into_iter().collect()),
        1,
        full_mask([alice].into_iter().collect()),
    );

    // Alice signs an Update at a position that includes R1. The claim is the stored set: an
    // Update cannot name other writers.
    let action = build_signed_shared_action(
        false,
        id,
        b"alice-update".to_vec(),
        [bob].into_iter().collect(),
        hlc_at(2),
        &alice_sk,
        vec![],
    );
    let ctx = ctx_at(&world, id, &[r1], alice);

    // Judged by the stored {Bob} this would be refused.
    Interface::<S<6402>>::apply_action(action, &ctx)
        .expect("the governance fold accepts Alice: she is the writer at R1");
}

/// Even with a governance position, an action signed by someone *outside* the writer set at it
/// is rejected.
#[test]
fn verifier_with_a_governance_position_rejects_a_non_writer() {
    let root = setup_root::<S<6403>>();

    let alice_sk = make_signing_key(0xA4);
    let bob_sk = make_signing_key(0xB4);
    let mallory_sk = make_signing_key(0xC4);
    let world = world_of(&[&alice_sk, &bob_sk]);
    let (alice, bob) = (
        world.account(&pubkey_of(&alice_sk)),
        world.account(&pubkey_of(&bob_sk)),
    );
    let id = cell_at(0x43, &[bob].into_iter().collect());

    let bootstrap = build_signed_shared_action(
        true,
        id,
        b"v0".to_vec(),
        [bob].into_iter().collect(),
        hlc_at(0),
        &bob_sk,
        vec![root.clone()],
    );
    Interface::<S<6403>>::apply_action(bootstrap, &apply_ctx_for(bob)).unwrap();

    let r1 = [0xD1; 32];
    world.rotate(
        &pubkey_of(&bob_sk),
        id,
        r1,
        &world.joined(),
        full_mask([bob].into_iter().collect()),
        1,
        full_mask([alice].into_iter().collect()),
    );

    // Mallory is in neither stored {Bob} nor the writers {Alice} at R1.
    let forged = build_signed_shared_action(
        false,
        id,
        b"forged".to_vec(),
        [bob].into_iter().collect(),
        hlc_at(2),
        &mallory_sk,
        vec![],
    );
    // The ctx resolves MALLORY's account, because that is what a node does: it resolves the
    // account from the action's own signer. Passing Alice's account beside Mallory's signature
    // would be a resolution mismatch that cannot arise in production (one delta, one author,
    // one resolution) and that storage has no way to detect, see `resolve_signer`'s contract.
    let ctx = ctx_at(&world, id, &[r1], account_of_key(&mallory_sk));

    let result = Interface::<S<6403>>::apply_action(forged, &ctx);
    assert!(matches!(result, Err(StorageError::InvalidSignature)));
}

// =============================================================================
// Write-hook tests
// =============================================================================

/// A cell's bootstrap with delta context stores nothing beside the cell: a writer set
/// changes by governance op, so applying a delta adds no child entity.
#[test]
fn applying_a_shared_bootstrap_with_delta_context_logs_no_rotation() {
    let root = setup_root::<S<6404>>();

    let alice_sk = make_signing_key(0xA5);
    let alice = account_of_key(&alice_sk);
    let id = cell_at(0x44, &[alice].into_iter().collect());

    let bootstrap = build_signed_shared_action(
        true,
        id,
        b"v0".to_vec(),
        [alice].into_iter().collect(),
        hlc_at(0),
        &alice_sk,
        vec![root.clone()],
    );
    let ctx = hook_ctx(&alice_sk);
    Interface::<S<6404>>::apply_action(bootstrap, &ctx).unwrap();

    assert!(
        calimero_storage::index::Index::<S<6404>>::get_children_of(id)
            .unwrap()
            .is_empty(),
        "the cell has no child beside the ones the app wrote"
    );
}

// =============================================================================
// ADR Example D coverage (write vs rotate on the same entity)
// =============================================================================

/// ADR Example D: a write signed before the rotation that removes its signer is accepted even
/// after that rotation is applied locally. The verifier consults the writers at the delta's own
/// governance position, NOT the set the node holds now.
#[test]
fn adr_example_d_pre_rotation_write_accepted_after_rotation() {
    let root = setup_root::<S<6420>>();

    let alice_sk = make_signing_key(0xA9);
    let bob_sk = make_signing_key(0xB9);
    let world = world_of(&[&alice_sk, &bob_sk]);
    let (alice, bob) = (
        world.account(&pubkey_of(&alice_sk)),
        world.account(&pubkey_of(&bob_sk)),
    );
    let id = cell_at(0x60, &[alice, bob].into_iter().collect());
    let joined = world.joined();

    // D_root: writers = {Alice, Bob}. Bootstrap so the entity exists locally.
    let bootstrap = build_signed_shared_action(
        true,
        id,
        b"hello".to_vec(),
        [alice, bob].into_iter().collect(),
        hlc_at(0),
        &alice_sk,
        vec![root.clone()],
    );
    Interface::<S<6420>>::apply_action(bootstrap, &ctx_at(&world, id, &joined, alice)).unwrap();

    // Governance: Alice rotates Bob out at R1, and this node has folded it.
    let r1 = [0xD1; 32];
    world.rotate(
        &pubkey_of(&alice_sk),
        id,
        r1,
        &joined,
        full_mask([alice, bob].into_iter().collect()),
        1,
        full_mask([alice].into_iter().collect()),
    );
    assert_eq!(
        writers_at(&world, id, &[r1]),
        Some(full_mask([alice].into_iter().collect())),
        "control: the rotation is in effect at R1"
    );

    // D2: Bob writes "world" against the writers he saw, signed at the joined cut that predates
    // the rotation.
    let bob_write = build_signed_shared_action(
        false,
        id,
        b"world".to_vec(),
        [alice, bob].into_iter().collect(),
        hlc_at(2),
        &bob_sk,
        vec![],
    );
    let ctx = ctx_at(&world, id, &joined, bob);

    // Judged at the node's current heads this would be refused; judged at D2's own position
    // Bob is still a writer.
    Interface::<S<6420>>::apply_action(bob_write, &ctx).expect(
        "ADR Example D: pre-rotation write by Bob accepted because the writers at the \
         position he signed at include him",
    );
}

/// Inverse of Example D: a write whose position *includes* the rotation (the writer saw it and
/// chose to write anyway) is rejected if the signer is no longer a writer at that position.
#[test]
fn write_post_rotation_by_removed_writer_rejected() {
    let root = setup_root::<S<6421>>();

    let alice_sk = make_signing_key(0xAA);
    let bob_sk = make_signing_key(0xBA);
    let world = world_of(&[&alice_sk, &bob_sk]);
    let (alice, bob) = (
        world.account(&pubkey_of(&alice_sk)),
        world.account(&pubkey_of(&bob_sk)),
    );
    let id = cell_at(0x61, &[alice, bob].into_iter().collect());
    let joined = world.joined();

    let bootstrap = build_signed_shared_action(
        true,
        id,
        b"hello".to_vec(),
        [alice, bob].into_iter().collect(),
        hlc_at(0),
        &alice_sk,
        vec![root.clone()],
    );
    Interface::<S<6421>>::apply_action(bootstrap, &ctx_at(&world, id, &joined, alice)).unwrap();

    // Governance: Alice rotates Bob out at R1.
    let r1 = [0xD1; 32];
    world.rotate(
        &pubkey_of(&alice_sk),
        id,
        r1,
        &joined,
        full_mask([alice, bob].into_iter().collect()),
        1,
        full_mask([alice].into_iter().collect()),
    );

    // Bob saw the rotation and tries to write anyway, at a position that includes it.
    let bob_write_post = build_signed_shared_action(
        false,
        id,
        b"world".to_vec(),
        [alice, bob].into_iter().collect(),
        hlc_at(2),
        &bob_sk,
        vec![],
    );
    // Bob authored this write, so the ctx resolves BOB's account, the honest resolution. He
    // was rotated out at R1, so the refusal below is authorization at the cut, not a mismatched
    // principal.
    let ctx = ctx_at(&world, id, &[r1], bob);

    let result = Interface::<S<6421>>::apply_action(bob_write_post, &ctx);
    assert!(
        matches!(result, Err(StorageError::InvalidSignature)),
        "post-rotation write by removed writer must be rejected; got {result:?}",
    );
}
