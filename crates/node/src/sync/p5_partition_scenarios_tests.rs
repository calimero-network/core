//! Phase **P5** of [#2233](https://github.com/calimero-network/core/issues/2233):
//! cross-node integration tests for the four motivating partition scenarios.
//!
//! A rotation is a governance op, so a delta is judged against the writers the governance
//! fold gives at the position its author signed. Each node here holds its own
//! [`RotationWorld`], a real projection fed with real ops in the order that node received them,
//! and `deliver` hands storage what the node would compute: the set a rotation left at the
//! delta's position, or nothing while the cell stands at the set its id commits to.

use core::num::NonZeroU64;

use calimero_account::AccountId;
use calimero_context::test_support::RotationWorld;
use calimero_storage::action::Action;
use calimero_storage::address::Id;
use calimero_storage::entities::{full_mask, ChildInfo, Metadata};
use calimero_storage::index::Index;
use calimero_storage::interface::{
    disable_nonce_check_for_testing, ApplyContext, Interface, StorageError,
};
use calimero_storage::logical_clock::{HybridTimestamp, Timestamp, ID, NTP64};
use calimero_storage::shared_writers::{CellWriters, Writers};
use calimero_storage::store::{MockedStorage, StorageAdaptor};
use calimero_storage::tests::common::{
    account_of_key, build_signed_shared_action, cell_at, pubkey_of,
};
use ed25519_dalek::SigningKey;

// =============================================================================
// Harness
// =============================================================================

/// One delta authored on some node; gets delivered to one or more nodes.
struct Delta {
    id: [u8; 32],
    hlc_ns: u64,
    action: Action,
    /// The account the node resolves `action`'s signing key to at the delta's position.
    signer: AccountId,
    /// The governance heads its author signed it at.
    position: Vec<[u8; 32]>,
}

fn hlc(ns: u64) -> HybridTimestamp {
    let node_id = ID::from(NonZeroU64::new(1).unwrap());
    HybridTimestamp::new(Timestamp::new(NTP64(ns), node_id))
}

/// The writers `world` gives `cell` at `position`: the set a rotation left, or nothing while
/// the cell stands at the set its id commits to.
fn writers_at(world: &RotationWorld, cell: Id, position: &[[u8; 32]]) -> Option<Writers> {
    let folded = world
        .projections
        .read()
        .unwrap()
        .shared_writers_at_cut(&world.store, &world.context, cell, position)
        .expect("the position is folded on this node");
    match folded {
        CellWriters::Genesis => None,
        CellWriters::Rotated(writers) => Some(writers),
    }
}

/// Apply `delta` to a node identified by const-generic `SCOPE` whose governance is `world`.
/// Mirrors the production flow from `delta_store::ContextStorageApplier::apply`: resolve
/// `effective_writers` at the delta's position, build an `ApplyContext`, then
/// `Interface::apply_action`.
fn deliver<S: StorageAdaptor>(delta: &Delta, world: &RotationWorld) -> Result<(), StorageError> {
    let ctx = ApplyContext {
        effective_writers: writers_at(world, delta.action.id(), &delta.position),
        delta_id: Some(delta.id),
        delta_hlc: Some(hlc(delta.hlc_ns)),
        signer_account: Some(delta.signer),
    };
    Interface::<S>::apply_action(delta.action.clone(), &ctx)
}

fn make_signing_key(seed: u8) -> SigningKey {
    SigningKey::from_bytes(&[seed; 32])
}

fn setup_root<S: StorageAdaptor>() -> ChildInfo {
    let root_id = Id::root();
    let root_meta = Metadata::default();
    Index::<S>::add_root(ChildInfo::new(root_id, [0; 32], root_meta.clone())).unwrap();
    let (full_hash, _) = Index::<S>::get_hashes_for(root_id).unwrap().unwrap();
    ChildInfo::new(root_id, full_hash, root_meta)
}

fn one_sec(n: u64) -> u64 {
    n.saturating_mul(1_000_000_000)
}

/// A world whose members are `owners`.
fn world_of(owners: &[&SigningKey]) -> RotationWorld {
    let keys: Vec<_> = owners.iter().map(|sk| pubkey_of(sk)).collect();
    RotationWorld::new(&keys)
}

// =============================================================================
// Scenario 1: Update-vs-rotation race (#2197 motivator 1)
// =============================================================================

/// Bob writes "world" against the writer set he sees ({Alice, Bob}) under a partition.
/// Concurrently, Alice rotates Bob out. The rotation reaches a third peer Carol first, then
/// Bob's write. Carol must accept Bob's write because it was signed at a governance position
/// before the rotation; from Bob's view he was authoritatively a writer when he authored it.
///
/// Judged against the writers Carol holds now, Bob's signature would be refused.
#[test]
fn update_vs_rotation_race_pre_rotation_write_accepted() {
    let _nonce_off = disable_nonce_check_for_testing();
    type Carol = MockedStorage<5500>;
    let root = setup_root::<Carol>();

    let alice_sk = make_signing_key(0xA1);
    let bob_sk = make_signing_key(0xB1);
    let carol = world_of(&[&alice_sk, &bob_sk]);
    let (alice, bob) = (
        carol.account(&pubkey_of(&alice_sk)),
        carol.account(&pubkey_of(&bob_sk)),
    );
    let id = cell_at(0x50, &[alice, bob].into_iter().collect());
    let joined = carol.joined();

    // D_root: Alice bootstraps the entity with writers = {Alice, Bob}.
    let d_root = Delta {
        id: [0xD0; 32],
        hlc_ns: one_sec(10),
        action: build_signed_shared_action(
            true,
            id,
            b"hello".to_vec(),
            [alice, bob].into_iter().collect(),
            one_sec(10),
            &alice_sk,
            vec![root.clone()],
        ),
        signer: alice,
        position: joined.clone(),
    };
    deliver::<Carol>(&d_root, &carol).expect("bootstrap delivered to Carol");

    // R1: Alice rotates Bob out, and Carol folds it before anything of Bob's arrives.
    let r1 = [0xD1; 32];
    carol.rotate(
        &pubkey_of(&alice_sk),
        id,
        r1,
        &joined,
        full_mask([alice, bob].into_iter().collect()),
        1,
        full_mask([alice].into_iter().collect()),
    );

    // D2: Bob writes "world" without knowledge of R1, so he signs at the joined position.
    let d2 = Delta {
        id: [0xD2; 32],
        hlc_ns: one_sec(21),
        action: build_signed_shared_action(
            false,
            id,
            b"world".to_vec(),
            [alice, bob].into_iter().collect(), // Bob's view of writers
            one_sec(21),
            &bob_sk,
            vec![],
        ),
        signer: bob,
        position: joined,
    };
    deliver::<Carol>(&d2, &carol).expect(
        "Bob's pre-rotation write must be accepted: the writers at the position he signed at \
         include him, even though the set Carol holds now does not",
    );

    // Control: the same write signed at a position that includes R1 is refused.
    let d3 = Delta {
        id: [0xD3; 32],
        hlc_ns: one_sec(22),
        action: build_signed_shared_action(
            false,
            id,
            b"again".to_vec(),
            [alice, bob].into_iter().collect(),
            one_sec(22),
            &bob_sk,
            vec![],
        ),
        signer: bob,
        position: vec![r1],
    };
    assert!(matches!(
        deliver::<Carol>(&d3, &carol),
        Err(StorageError::InvalidSignature)
    ));
}

// =============================================================================
// Scenario 2: Self-removal mid-flight (#2197 motivator 3)
// =============================================================================

/// Alice rotates herself out (writers go from {Alice, Bob} to {Bob}). She has an in-flight
/// update D2 signed before the rotation; a peer that sees both accepts it. A second in-flight
/// update D3 signed at a position that includes the rotation (Alice saw it and tried to write
/// anyway) is rejected.
#[test]
fn self_removal_mid_flight_pre_accepted_post_rejected() {
    let _nonce_off = disable_nonce_check_for_testing();
    type Carol = MockedStorage<5510>;
    let root = setup_root::<Carol>();

    let alice_sk = make_signing_key(0xA2);
    let bob_sk = make_signing_key(0xB2);
    let carol = world_of(&[&alice_sk, &bob_sk]);
    let (alice, bob) = (
        carol.account(&pubkey_of(&alice_sk)),
        carol.account(&pubkey_of(&bob_sk)),
    );
    let id = cell_at(0x51, &[alice, bob].into_iter().collect());
    let joined = carol.joined();

    // D_root: bootstrap with {Alice, Bob}.
    let d_root = Delta {
        id: [0xE0; 32],
        hlc_ns: one_sec(10),
        action: build_signed_shared_action(
            true,
            id,
            b"v0".to_vec(),
            [alice, bob].into_iter().collect(),
            one_sec(10),
            &alice_sk,
            vec![root.clone()],
        ),
        signer: alice,
        position: joined.clone(),
    };
    deliver::<Carol>(&d_root, &carol).unwrap();

    // R1: Alice rotates herself out.
    let r1 = [0xE1; 32];
    carol.rotate(
        &pubkey_of(&alice_sk),
        id,
        r1,
        &joined,
        full_mask([alice, bob].into_iter().collect()),
        1,
        full_mask([bob].into_iter().collect()),
    );

    // D2: Alice's in-flight write, signed before R1.
    let d2 = Delta {
        id: [0xE2; 32],
        hlc_ns: one_sec(15),
        action: build_signed_shared_action(
            false,
            id,
            b"alice-pre".to_vec(),
            [alice, bob].into_iter().collect(),
            one_sec(15),
            &alice_sk,
            vec![],
        ),
        signer: alice,
        position: joined,
    };
    // D3: Alice tries to write AFTER her own rotation.
    let d3 = Delta {
        id: [0xE3; 32],
        hlc_ns: one_sec(25),
        action: build_signed_shared_action(
            false,
            id,
            b"alice-post".to_vec(),
            [alice, bob].into_iter().collect(),
            one_sec(25),
            &alice_sk,
            vec![],
        ),
        signer: alice,
        position: vec![r1],
    };

    deliver::<Carol>(&d2, &carol).expect(
        "Alice's pre-rotation write accepted: the writers at the position she signed at \
         include her",
    );
    let post_result = deliver::<Carol>(&d3, &carol);
    assert!(
        matches!(post_result, Err(StorageError::InvalidSignature)),
        "post-rotation write by removed writer must be rejected; got {post_result:?}",
    );
}

// =============================================================================
// Scenario 3: Concurrent conflicting rotations (#2197 motivator 2)
// =============================================================================

/// Alice and Bob rotate the cell concurrently, each adding a different writer. Two peers (Carol,
/// Dave) fold the two rotations in opposite orders. The governance fold picks the same winner on
/// both, the lowest `(nonce, signer, op id)`, and they accept and refuse the same writes.
#[test]
fn concurrent_conflicting_rotations_deterministic_convergence() {
    let _nonce_off = disable_nonce_check_for_testing();
    type Carol = MockedStorage<5520>;
    type Dave = MockedStorage<5521>;
    let carol_root = setup_root::<Carol>();
    let dave_root = setup_root::<Dave>();

    let alice_sk = make_signing_key(0xA3);
    let bob_sk = make_signing_key(0xB3);
    let erin_sk = make_signing_key(0xE3);
    let frank_sk = make_signing_key(0xF3);
    let carol_world = world_of(&[&alice_sk, &bob_sk]);
    let dave_world = world_of(&[&alice_sk, &bob_sk]);
    let (alice, bob) = (
        carol_world.account(&pubkey_of(&alice_sk)),
        carol_world.account(&pubkey_of(&bob_sk)),
    );
    let (erin, frank) = (account_of_key(&erin_sk), account_of_key(&frank_sk));
    let id = cell_at(0x52, &[alice, bob].into_iter().collect());
    let joined = carol_world.joined();
    let genesis = full_mask([alice, bob].into_iter().collect());

    let bootstrap = |root: &ChildInfo| Delta {
        id: [0xF0; 32],
        hlc_ns: one_sec(10),
        action: build_signed_shared_action(
            true,
            id,
            b"v0".to_vec(),
            [alice, bob].into_iter().collect(),
            one_sec(10),
            &alice_sk,
            vec![root.clone()],
        ),
        signer: alice,
        position: joined.clone(),
    };
    deliver::<Carol>(&bootstrap(&carol_root), &carol_world).unwrap();
    deliver::<Dave>(&bootstrap(&dave_root), &dave_world).unwrap();

    // Ra: Alice adds Erin (nonce 5). Rb: Bob adds Frank (nonce 9). Concurrent, both on `joined`.
    let (ra, rb) = ([0xF1; 32], [0xF2; 32]);
    let with = |extra: AccountId| {
        let mut set = genesis.clone();
        let _ = set.insert(extra, calimero_storage::entities::OpMask::WRITE);
        set
    };
    let rotate_ra = |world: &RotationWorld| {
        world.rotate(
            &pubkey_of(&alice_sk),
            id,
            ra,
            &joined,
            genesis.clone(),
            5,
            with(erin),
        );
    };
    let rotate_rb = |world: &RotationWorld| {
        world.rotate(
            &pubkey_of(&bob_sk),
            id,
            rb,
            &joined,
            genesis.clone(),
            9,
            with(frank),
        );
    };
    // Carol folds Ra then Rb; Dave folds Rb then Ra.
    rotate_ra(&carol_world);
    rotate_rb(&carol_world);
    rotate_rb(&dave_world);
    rotate_ra(&dave_world);

    // Queried at the merged cut, both give the lower nonce's set.
    let merged = [ra, rb];
    let carol_writers = writers_at(&carol_world, id, &merged);
    let dave_writers = writers_at(&dave_world, id, &merged);
    assert_eq!(carol_writers, dave_writers, "deterministic convergence");
    assert_eq!(
        carol_writers,
        Some(with(erin)),
        "Ra (nonce 5) wins the tie-break against Rb (nonce 9)"
    );

    // Both accept Erin's write and refuse Frank's at the merged cut.
    let write_by = |who: &SigningKey, account: AccountId, n: u8| Delta {
        id: [n; 32],
        hlc_ns: one_sec(30 + u64::from(n)),
        action: build_signed_shared_action(
            false,
            id,
            vec![n],
            [alice, bob].into_iter().collect(),
            one_sec(30 + u64::from(n)),
            who,
            vec![],
        ),
        signer: account,
        position: merged.to_vec(),
    };
    let by_erin = write_by(&erin_sk, erin, 0x71);
    let by_frank = write_by(&frank_sk, frank, 0x72);
    deliver::<Carol>(&by_erin, &carol_world).expect("Erin is a writer at the merged cut on Carol");
    deliver::<Dave>(&by_erin, &dave_world).expect("Erin is a writer at the merged cut on Dave");
    assert!(matches!(
        deliver::<Carol>(&by_frank, &carol_world),
        Err(StorageError::InvalidSignature)
    ));
    assert!(matches!(
        deliver::<Dave>(&by_frank, &dave_world),
        Err(StorageError::InvalidSignature)
    ));
}

// =============================================================================
// Scenario 4: Long-partition reconciliation
// =============================================================================

/// Two nodes are partitioned for a "long time": each side rotates the cell and accumulates a
/// write signed under its own rotation. After the partition heals, both nodes fold the other
/// side's rotation and deliver its write. Every write is judged at the position it was signed
/// at, so both are accepted on both sides, and both sides agree on the writers at the merged cut.
#[test]
fn long_partition_reconciliation_converges() {
    let _nonce_off = disable_nonce_check_for_testing();
    type Left = MockedStorage<5530>;
    type Right = MockedStorage<5531>;
    let left_root = setup_root::<Left>();
    let right_root = setup_root::<Right>();

    let alice_sk = make_signing_key(0xA4);
    let bob_sk = make_signing_key(0xB4);
    let carol_sk = make_signing_key(0xC4);
    let dave_sk = make_signing_key(0xD4);
    let left_world = world_of(&[&alice_sk, &bob_sk]);
    let right_world = world_of(&[&alice_sk, &bob_sk]);
    let (alice, bob) = (
        left_world.account(&pubkey_of(&alice_sk)),
        left_world.account(&pubkey_of(&bob_sk)),
    );
    let (carol, dave) = (account_of_key(&carol_sk), account_of_key(&dave_sk));
    let id = cell_at(0x53, &[alice, bob].into_iter().collect());
    let joined = left_world.joined();
    let genesis = full_mask([alice, bob].into_iter().collect());
    let with = |extra: AccountId| {
        let mut set = genesis.clone();
        let _ = set.insert(extra, calimero_storage::entities::OpMask::WRITE);
        set
    };

    // Pre-partition bootstrap: writers = {Alice, Bob}.
    let bootstrap = |root: &ChildInfo| Delta {
        id: [0x10; 32],
        hlc_ns: one_sec(10),
        action: build_signed_shared_action(
            true,
            id,
            b"v0".to_vec(),
            [alice, bob].into_iter().collect(),
            one_sec(10),
            &alice_sk,
            vec![root.clone()],
        ),
        signer: alice,
        position: joined.clone(),
    };
    deliver::<Left>(&bootstrap(&left_root), &left_world).unwrap();
    deliver::<Right>(&bootstrap(&right_root), &right_world).unwrap();

    let write_by = |who: &SigningKey, account: AccountId, n: u8, position: Vec<[u8; 32]>| Delta {
        id: [n; 32],
        hlc_ns: one_sec(u64::from(n)),
        action: build_signed_shared_action(
            false,
            id,
            vec![n],
            [alice, bob].into_iter().collect(),
            one_sec(u64::from(n)),
            who,
            vec![],
        ),
        signer: account,
        position,
    };

    // Left: Alice adds Carol (L1, nonce 20); Carol writes "left" at L1.
    let (l1, r1) = ([0x11; 32], [0x21; 32]);
    let rotate_l1 = |world: &RotationWorld| {
        world.rotate(
            &pubkey_of(&alice_sk),
            id,
            l1,
            &joined,
            genesis.clone(),
            20,
            with(carol),
        );
    };
    let rotate_r1 = |world: &RotationWorld| {
        world.rotate(
            &pubkey_of(&bob_sk),
            id,
            r1,
            &joined,
            genesis.clone(),
            25,
            with(dave),
        );
    };
    rotate_l1(&left_world);
    let l2 = write_by(&carol_sk, carol, 0x12, vec![l1]);
    deliver::<Left>(&l2, &left_world).expect("Carol writes under Alice's rotation");

    // Right: Bob adds Dave (R1, nonce 25); Dave writes "right" at R1.
    rotate_r1(&right_world);
    let r2 = write_by(&dave_sk, dave, 0x22, vec![r1]);
    deliver::<Right>(&r2, &right_world).expect("Dave writes under Bob's rotation");

    // Partition heals: each side folds the other's rotation, then delivers its write.
    rotate_r1(&left_world);
    rotate_l1(&right_world);
    deliver::<Left>(&r2, &left_world).expect("Dave's write, signed at R1, accepted on Left");
    deliver::<Right>(&l2, &right_world).expect("Carol's write, signed at L1, accepted on Right");

    // Both sides queried at the merged cut agree.
    let merged = [l1, r1];
    let left_writers = writers_at(&left_world, id, &merged);
    let right_writers = writers_at(&right_world, id, &merged);
    assert_eq!(
        left_writers, right_writers,
        "both sides converge on the same writer set at {{L1, R1}}"
    );
    // Neither rotation is in the other's past, so the lower nonce wins: L1 (20) over R1 (25).
    assert_eq!(left_writers, Some(with(carol)));
}
