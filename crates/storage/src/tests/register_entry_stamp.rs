//! A register that is a map entry's whole value is stored without its stamp,
//! and reads it back from the entry's `updated_at` (`entry_stamp`).
//!
//! Nothing ever resolved two versions of such an entry by the register's own
//! stamp: a local write, a remote apply, a snapshot and a HashComparison leaf
//! all pick between whole entries by `updated_at`. So replicas must converge
//! exactly as before, and every register that is NOT a map entry's whole value
//! must keep its stamp, since those are merged field by field.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

use crate::address::Id;
use crate::collections::{LwwRegister, Root, SortedMap, UnorderedMap, Vector};
use crate::delta::{clear_pending_delta, StorageDelta};
use crate::env::{self, take_last_artifact, with_runtime_env, RuntimeEnv};
use crate::index::Index;
use crate::interface::{ApplyContext, Interface};
use crate::logical_clock::HybridTimestamp;
use crate::store::{Key, MainStorage, KEY_LEN};

type Rows = Rc<RefCell<BTreeMap<[u8; KEY_LEN], Vec<u8>>>>;
type Map = UnorderedMap<String, LwwRegister<String>, MainStorage>;

/// Shared by every replica, so they hold the same collection.
const FIELD: &str = "registers";

/// Must be the native default: `ROOT_ID` is a process-global `LazyLock` seeded
/// from the first context id any test on the process reads.
const CONTEXT_ID: [u8; 32] = [236; 32];

const T: u64 = 1_800_000_000_000_000_000;

fn env(rows: &Rows, device: [u8; 32]) -> RuntimeEnv {
    let r = Rc::clone(rows);
    let read = Rc::new(move |key: &Key| r.borrow().get(&key.to_bytes()).cloned());
    let w = Rc::clone(rows);
    let write = Rc::new(move |key: Key, value: &[u8]| {
        w.borrow_mut()
            .insert(key.to_bytes(), value.to_vec())
            .is_some()
    });
    let rm = Rc::clone(rows);
    let remove = Rc::new(move |key: &Key| rm.borrow_mut().remove(&key.to_bytes()).is_some());
    RuntimeEnv::new(read, write, remove, CONTEXT_ID, device, [3; 32])
}

/// Runs `f` against `rows` as replica `device`, one execution with the wall
/// clock at `time`.
fn on<R>(rows: &Rows, device: u8, time: u64, f: impl FnOnce() -> R) -> R {
    clear_pending_delta();
    let previous = env::begin_execution_for_testing(time);
    let out = with_runtime_env(env(rows, [device; 32]), f);
    env::restore_wall_clock_for_testing(previous);
    out
}

/// Replica `device` writes `value` under "k" at `time`, returning the delta it
/// ships.
fn write(rows: &Rows, device: u8, time: u64, value: &str) -> Vec<u8> {
    on(rows, device, time, || {
        let mut map = match Root::<Map>::fetch() {
            Some(map) => map,
            None => Root::new(|| Map::new_with_field_name(FIELD)),
        };
        drop(
            map.insert("k".to_owned(), LwwRegister::new(value.to_owned()))
                .unwrap(),
        );
        map.commit();
        take_last_artifact().unwrap()
    })
}

/// Applies a delta `write` returned, as a peer receiving it does.
fn apply(rows: &Rows, delta: &[u8]) {
    let actions = match borsh::from_slice::<StorageDelta>(delta).unwrap() {
        StorageDelta::Actions(actions) | StorageDelta::CausalActions { actions, .. } => actions,
    };
    on(rows, 9, T, || {
        for action in actions.into_iter().filter(|action| !action.id().is_root()) {
            Interface::<MainStorage>::apply_action(action, &ApplyContext::empty()).unwrap();
        }
    });
}

/// What `rows` holds under "k": the register, the entry's `updated_at`, and
/// the entry's stored value bytes.
fn read(rows: &Rows) -> (LwwRegister<String>, u64, Vec<u8>) {
    let (register, updated_at, id) = on(rows, 9, T, || {
        let map = Root::<Map>::fetch().unwrap();
        let id = map.entry_id(&"k".to_owned());
        let register = map.get("k").unwrap().unwrap().clone();
        let updated_at = Index::<MainStorage>::get_metadata(id)
            .unwrap()
            .unwrap()
            .updated_at();
        (register, updated_at, id)
    });
    (register, updated_at, data(rows, id))
}

fn row(rows: &Rows, id: Id) -> Vec<u8> {
    rows.borrow()
        .get(&Key::Index(id).to_bytes())
        .cloned()
        .expect("the entry has a row")
}

fn data(rows: &Rows, id: Id) -> Vec<u8> {
    crate::row::decode(id, &row(rows, id))
        .and_then(|row| row.data)
        .expect("the entry has data")
}

#[test]
fn a_map_register_is_stored_without_its_stamp() {
    let rows: Rows = Rc::default();
    let _delta = write(&rows, 1, T, "v");

    let id = on(&rows, 1, T, || {
        Root::<Map>::fetch().unwrap().entry_id(&"k".to_owned())
    });
    let mut expected = borsh::to_vec(&("v", "k")).unwrap();
    expected.extend_from_slice(id.as_bytes());
    assert_eq!(
        data(&rows, id),
        expected,
        "value, then key, then the id: no 16-byte stamp"
    );
}

#[test]
fn a_map_register_reads_its_stamp_from_the_row() {
    let rows: Rows = Rc::default();
    let _first = write(&rows, 1, T, "first");
    let (register, updated_at, _) = read(&rows);
    assert_eq!(register.get(), "first");
    assert_eq!(
        register.timestamp(),
        HybridTimestamp::from_unix_nanos(updated_at)
    );

    // An overwrite on a clock behind the stored stamp still lands after it,
    // and its register reads the newer stamp.
    let _second = write(&rows, 1, T - 3_000_000_000, "second");
    let (second, second_at, _) = read(&rows);
    assert_eq!(second.get(), "second");
    assert!(second_at > updated_at);
    assert_eq!(
        second.timestamp(),
        HybridTimestamp::from_unix_nanos(second_at)
    );
    assert!(second.timestamp() > register.timestamp());
}

#[test]
fn a_stamp_orders_as_the_nanoseconds_it_is_read_from() {
    let stamps: Vec<_> = [0, 1, 999_999_999, 1_000_000_000, T, T + 1]
        .into_iter()
        .map(HybridTimestamp::from_unix_nanos)
        .collect();
    assert!(stamps.windows(2).all(|pair| pair[0] < pair[1]));
}

/// Registers that are merged by their own stamp keep it: a register inside a
/// map value of another type, a vector's element and the root. A sorted map's
/// register value goes without it, as a map's does.
#[test]
fn only_registers_that_are_an_entry_value_go_without_their_stamp() {
    type Wrapped = UnorderedMap<String, Option<LwwRegister<u64>>, MainStorage>;
    type Listed = Vector<LwwRegister<u64>, MainStorage>;
    type Sorted = SortedMap<String, LwwRegister<u64>, MainStorage>;

    let stamped = borsh::to_vec(&LwwRegister::new_with_metadata(
        7_u64,
        HybridTimestamp::from_unix_nanos(T),
    ))
    .unwrap();
    assert_eq!(stamped.len(), 8 + 16);

    let rows: Rows = Rc::default();
    let (wrapped, listed, sorted) = on(&rows, 1, T, || {
        let mut wrapped = Wrapped::new_with_field_name("wrapped");
        let _replaced = wrapped
            .insert("k".to_owned(), Some(LwwRegister::new(7)))
            .unwrap();
        let mut listed = Listed::new_with_field_name("listed");
        listed.push(LwwRegister::new(7)).unwrap();
        let listed = Index::<MainStorage>::get_children_of(listed.collection_id()).unwrap()[0].id();
        let mut sorted = Sorted::new_with_field_name("sorted");
        let _replaced = sorted.insert("k".to_owned(), LwwRegister::new(7)).unwrap();
        let sorted = sorted.entry_id(&"k".to_owned());
        (wrapped.entry_id(&"k".to_owned()), listed, sorted)
    });
    // `Some` ‖ register ‖ key ‖ id; register ‖ id; value ‖ key ‖ id.
    assert_eq!(data(&rows, wrapped).len(), 1 + 24 + (4 + 1) + 32);
    assert_eq!(data(&rows, listed).len(), 24 + 32);
    assert_eq!(data(&rows, sorted).len(), 8 + (4 + 1) + 32);

    let root: Rows = Rc::default();
    let root_data = on(&root, 1, T, || {
        let mut register = Root::new(|| LwwRegister::new(7_u64));
        register.set(8);
        register.commit();
        Interface::<MainStorage>::find_by_id_raw(crate::collections::ROOT_ENTRY_ID).unwrap()
    });
    assert!(
        root_data.starts_with(&8_u64.to_le_bytes()) && root_data.len() >= 24,
        "the root register keeps its stamp: {root_data:02x?}"
    );
}

/// Both replicas write "k" concurrently and receive each other's write; they
/// end on the same register, stamp and stored bytes. (Not the same row: a
/// replica keeps its own `created_at` for an entry it created, as it always
/// has.)
fn converge(alice_at: u64, alice: &str, bob_at: u64, bob: &str) -> (String, u64) {
    let at_alice: Rows = Rc::default();
    let at_bob: Rows = Rc::default();
    let from_alice = write(&at_alice, 1, alice_at, alice);
    let from_bob = write(&at_bob, 2, bob_at, bob);
    apply(&at_alice, &from_bob);
    apply(&at_bob, &from_alice);

    let (register_a, updated_a, data_a) = read(&at_alice);
    let (register_b, updated_b, data_b) = read(&at_bob);
    assert_eq!(
        register_a, register_b,
        "{alice}@{alice_at} vs {bob}@{bob_at}"
    );
    assert_eq!(updated_a, updated_b);
    assert_eq!(data_a, data_b, "the entries must match byte for byte");
    (register_a.get().clone(), updated_a)
}

const SECOND: u64 = 1_000_000_000;

#[test]
fn concurrent_writes_with_different_stamps_converge_on_the_later() {
    let (winner, at) = converge(T, "alice", T + SECOND, "bob");
    assert_eq!(winner, "bob");
    assert!(at > T + SECOND);
    let (winner, at) = converge(T + SECOND, "alice", T, "bob");
    assert_eq!(winner, "alice");
    assert!(at > T + SECOND);
}

/// The two writes are stamped the same nanosecond, which only the stored
/// bytes can break, the same way on both replicas.
#[test]
fn concurrent_writes_with_equal_stamps_converge_on_one_value() {
    let (winner, _) = converge(T, "alice", T, "bob");
    // Whichever replica wrote which, the same value wins.
    assert_eq!(converge(T, "bob", T, "alice").0, winner);
}

#[test]
fn concurrent_writes_of_one_value_converge_on_the_later_stamp() {
    let (_, at) = converge(T, "same", T + SECOND, "same");
    assert!(at > T + SECOND);
    let (_, back) = converge(T + SECOND, "same", T, "same");
    assert_eq!(back, at);
}
