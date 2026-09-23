//! The single registry of measured workloads; the snapshot binary, the shape
//! tests and the benches all iterate `all()`, so they cannot measure different
//! things.
//!
//! A *build* workload runs `n` operations and reports their total; what is
//! asserted is cost per entry. A *point* workload builds `n` entries, calls
//! [`crate::reset_counters`], then performs one operation and reports only
//! that. [`CostShape`] declares which, and what curve the result must follow.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

use calimero_storage::action::Action;
use calimero_storage::collections::{
    FugueText, LwwRegister, NestedMapOps, ReplicatedGrowableArray, Root, UnorderedMap, Vector,
};
use calimero_storage::delta::{clear_pending_delta, StorageDelta};
use calimero_storage::env::{take_last_artifact, with_runtime_env, RuntimeEnv};
use calimero_storage::interface::{ApplyContext, Interface};
use calimero_storage::store::{Key, MainStorage};

use crate::reset_counters;

pub const SIZES: [usize; 4] = [10, 100, 1_000, 10_000]; // shape tests span first to last

/// Sizes for [`CostShape::QuadraticBuild`] only: their total cost is `O(n^2)`,
/// so `SIZES`'s top row would take about a minute per measurement.
pub const QUADRATIC_SIZES: [usize; 4] = [10, 100, 500, 2_000];

/// What the cost curve of a workload is required to look like. An assertion,
/// not a description: `tests/flat_curve.rs` checks every variant, in both
/// directions, so a fix cannot land while the marker still describes the wall.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CostShape {
    /// A build of `n` entries. Cost **per entry** must not grow with `n`.
    FlatPerEntry,
    /// One operation against a collection of `n` entries. Its **total** cost
    /// must not grow with `n`.
    ConstantPerCall,
    /// One operation whose total cost is known to grow linearly with `n`.
    KnownLinearInN,
    /// A build of `n` operations each costing `O(n)`, so `O(n^2)` overall.
    /// Measured at [`QUADRATIC_SIZES`].
    QuadraticBuild,
}

/// One measurable unit of work at one collection size.
pub struct Workload {
    pub name: &'static str, // appears in the snapshot, so a rename shows up in the diff
    pub n: usize,
    pub shape: CostShape,
    /// Slack the snapshot gate allows, in percent. Zero unless the operation
    /// draws random entity ids, which move how the child trie is walked;
    /// `tests/reproducible.rs` re-derives every declared value from live runs.
    pub tolerance_pct: u32,
    pub run: fn(usize),
}

/// Insert `n` entries into an `UnorderedMap`, measuring the whole build.
fn unordered_map_insert(n: usize) {
    build_map(n);
}

/// Push `n` entries onto a `Vector`, measuring the whole build.
fn vector_push(n: usize) {
    build_vector(n);
}

/// Cost of ONE `len()` against `n` entries. `len()` reading the whole
/// collection to count it is a real regression this pins against.
fn unordered_map_len(n: usize) {
    let map = build_map(n);
    reset_counters();
    let _ignored = map.len().expect("len should succeed");
}

/// Cost of ONE keyed `get()` against `n` entries.
fn unordered_map_get(n: usize) {
    let map = build_map(n);
    reset_counters();
    let _ignored = map.get("key0").expect("get should succeed");
}

/// Cost of ONE `Vector::get(i)` against `n` entries: linear, because ordering
/// is a comparator applied after a full child-trie enumeration. The middle
/// index is read so a fast path for index 0 could not make this lie.
fn vector_get_nth(n: usize) {
    let vector = build_vector(n);
    reset_counters();
    let _ignored = vector.get(n / 2).expect("get should succeed");
}

/// Paste `n` characters into an empty RGA as one `insert_str`, which
/// linearises the document once and then does `n` flat inserts. The per-char
/// route is `rga_insert_per_char`, and it is a different shape.
fn rga_insert(n: usize) {
    build_rga(n);
}

/// Read the whole RGA document after building `n` characters. It measures
/// `get_text` because `ReplicatedGrowableArray` has no positional read at
/// all, so every read of it linearises the document.
fn rga_get_nth(n: usize) {
    let rga = build_rga(n);
    reset_counters();
    let _ignored = rga.get_text().expect("get_text should succeed");
}

/// Type `n` characters into an RGA one call at a time. Quadratic because
/// every `insert` re-derives its left neighbour by linearising the whole
/// document: reads/entry climbs from 63.5 at `n=10` to 2047.0 at `n=2_000`.
fn rga_insert_per_char(n: usize) {
    let mut rga = Root::new(ReplicatedGrowableArray::<MainStorage>::new);
    for i in 0..n {
        rga.insert(i, 'a').expect("insert should succeed");
    }
}

/// The root is re-fetched per iteration because a `Root` caches its children.
fn rga_insert_interleaved_sync(n: usize) {
    let rga = Root::new(|| {
        ReplicatedGrowableArray::<MainStorage>::new_with_field_name(INTERLEAVED_DOC_FIELD)
    });
    rga.commit();
    for i in 0..n / 2 {
        let mut rga = Root::<ReplicatedGrowableArray<MainStorage>>::fetch()
            .expect("document root should exist");
        rga.insert(i, 'a').expect("local insert should succeed");
        rga.commit();
        land_remote_char(REMOTE_RGA_DEVICE, "a ReplicatedGrowableArray", || {
            let mut rga = Root::new(|| {
                ReplicatedGrowableArray::<MainStorage>::new_with_field_name(INTERLEAVED_DOC_FIELD)
            });
            rga.insert(0, 'r').expect("remote insert should succeed");
            rga.commit();
        });
    }
}

/// Shared with the remote replica: differing ids give two documents that never meet.
const INTERLEAVED_DOC_FIELD: &str = "interleaved_doc";

const REMOTE_RGA_DEVICE: [u8; 32] = [2; 32];

/// The root action is skipped: the receiver's root is not the sender's to overwrite.
fn land_remote_char(device_id: [u8; 32], collection: &str, author: impl FnOnce()) {
    for action in remote_char_actions(device_id, collection, author) {
        if action.id().is_root() {
            continue;
        }
        Interface::<MainStorage>::apply_action(action, &ApplyContext::empty())
            .expect("remote apply_action should succeed");
    }
}

/// Clears the thread-local pending delta first, or another workload's writes count as sync cost.
fn remote_char_actions(
    device_id: [u8; 32],
    collection: &str,
    author: impl FnOnce(),
) -> Vec<Action> {
    clear_pending_delta();
    let delta = with_runtime_env(uncounted_env(device_id), || {
        author();
        take_last_artifact().expect("commit should emit a delta")
    });
    let actions = match borsh::from_slice::<StorageDelta>(&delta).expect("delta should decode") {
        StorageDelta::Actions(actions) => actions,
        StorageDelta::CausalActions { actions, .. } => actions,
    };
    assert_eq!(
        actions.len(),
        REMOTE_CHAR_ACTIONS,
        "the remote replica's delta carried {} actions, not the {REMOTE_CHAR_ACTIONS} that \
         typing ONE character into {collection} emits. Either the thread-local pending-delta \
         buffer leaked into it (check that `clear_pending_delta()` above still runs, so this \
         workload is not about to re-apply writes that were already done and report the cost \
         as sync), or a storage-layer change altered what one character insert emits, in which \
         case move REMOTE_CHAR_ACTIONS deliberately per its doc comment and regenerate the \
         snapshot, because the applied cost changed too.",
        actions.len()
    );
    actions
}

const REMOTE_CHAR_ACTIONS: usize = 6;

/// Not wired to the measurement counters: what is being measured is what the
/// receiver pays, so the sender's own writes must stay uncounted.
fn uncounted_env(device_id: [u8; 32]) -> RuntimeEnv {
    let map: Rc<RefCell<BTreeMap<[u8; 32], Vec<u8>>>> = Rc::new(RefCell::new(BTreeMap::new()));
    let read = {
        let map = Rc::clone(&map);
        Rc::new(move |key: &Key| map.borrow().get(&key.to_bytes()).cloned())
    };
    let write = {
        let map = Rc::clone(&map);
        Rc::new(move |key: Key, value: &[u8]| {
            let _ignored = map.borrow_mut().insert(key.to_bytes(), value.to_vec());
            true
        })
    };
    let remove = {
        let map = Rc::clone(&map);
        Rc::new(move |key: &Key| map.borrow_mut().remove(&key.to_bytes()).is_some())
    };
    RuntimeEnv::new(read, write, remove, [1; 32], device_id, [3; 32])
}

fn build_rga(n: usize) -> Root<ReplicatedGrowableArray<MainStorage>> {
    let mut rga = Root::new(ReplicatedGrowableArray::<MainStorage>::new);
    let text: String = std::iter::repeat_n('a', n).collect();
    rga.insert_str(0, &text).expect("insert_str should succeed");
    rga
}

fn fugue_text_insert(n: usize) {
    let _ignored = build_fugue_text(n);
}

fn fugue_text_char_at_fragmented(n: usize) {
    let text = build_fugue_text_fragmented(n);
    reset_counters();
    let _ignored = text.char_at(n / 2).expect("char_at should succeed");
}

fn fugue_text_char_at(n: usize) {
    let text = build_fugue_text(n);
    reset_counters();
    let _ignored = text.char_at(n / 2).expect("char_at should succeed");
}

fn fugue_text_insert_per_char(n: usize) {
    let mut text = Root::new(FugueText::<MainStorage>::new);
    for i in 0..n {
        text.insert(i, 'a').expect("insert should succeed");
    }
}

fn fugue_text_insert_middle(n: usize) {
    let _ignored = build_fugue_text_fragmented(n);
}

fn fugue_text_insert_interleaved_sync(n: usize) {
    let text = Root::new(|| FugueText::<MainStorage>::new_with_field_name(FUGUE_INTERLEAVED_FIELD));
    text.commit();
    for i in 0..n / 2 {
        let mut text = Root::<FugueText<MainStorage>>::fetch().expect("document root should exist");
        text.insert(i, 'a').expect("local insert should succeed");
        text.commit();
        land_remote_fugue_char(i);
    }
}

const FUGUE_INTERLEAVED_FIELD: &str = "interleaved_fugue_doc";

/// Per-index: the node counter restarts per character, so one fixed id would mint duplicates.
fn remote_fugue_device(index: usize) -> [u8; 32] {
    let mut device = [9_u8; 32];
    device[..8].copy_from_slice(&(index as u64 + 1).to_be_bytes());
    device
}

fn land_remote_fugue_char(index: usize) {
    land_remote_char(remote_fugue_device(index), "a FugueText", || {
        let mut text =
            Root::new(|| FugueText::<MainStorage>::new_with_field_name(FUGUE_INTERLEAVED_FIELD));
        text.insert(0, 'r').expect("remote insert should succeed");
        text.commit();
    });
}

fn build_fugue_text_fragmented(n: usize) -> Root<FugueText<MainStorage>> {
    let mut text = Root::new(FugueText::<MainStorage>::new);
    for i in 0..n {
        text.insert(i / 2, 'a').expect("insert should succeed");
    }
    text
}

fn build_fugue_text(n: usize) -> Root<FugueText<MainStorage>> {
    let mut text = Root::new(FugueText::<MainStorage>::new);
    text.insert_str(0, &"a".repeat(n))
        .expect("insert_str should succeed");
    text
}

/// `n` set-then-commit transactions against the SAME `LwwRegister`, so `n` is
/// a history length, not a collection size: a register's write cost must not
/// grow with how often it has already been overwritten.
fn lww_register_set(n: usize) {
    let register = Root::new(|| LwwRegister::<String>::new(String::new()));
    register.commit();
    for i in 0..n {
        let mut register =
            Root::<LwwRegister<String>>::fetch().expect("register root should exist");
        register.set(format!("v{i}"));
        register.commit();
    }
}

/// Insert `n` inner entries under ONE outer key, so the inner map is minted
/// once rather than `n` times: a fresh outer key per entry would mint a random
/// id per call and put the counts back on the trie's random bucket spread.
fn nested_map_insert(n: usize) {
    build_nested_map(n);
}

/// Cost of ONE `get_nested()` against a nested map with `n` inner entries.
fn nested_map_get(n: usize) {
    let map = build_nested_map(n);
    reset_counters();
    let _ignored = map
        .get_nested(&"outer".to_owned(), &"inner0".to_owned())
        .expect("get_nested should succeed");
}

fn build_nested_map(
    n: usize,
) -> Root<UnorderedMap<String, UnorderedMap<String, String, MainStorage>, MainStorage>> {
    // The outer map and the seed below both take a deterministic id: the
    // one-time re-key's target id derives from the outer map's own id, so a
    // random id anywhere in that chain makes the counts vary run to run.
    let mut map = Root::new(|| {
        UnorderedMap::<String, UnorderedMap<String, String, MainStorage>, MainStorage>::new_with_field_name("outer")
    });
    // Seeding an EMPTY inner map first leaves the one-time re-key nothing to
    // relocate, so its cost is not folded into the first `inner0` insert.
    map.insert(
        "outer".to_owned(),
        UnorderedMap::<String, String, MainStorage>::new_with_field_name("seed"),
    )
    .expect("seed insert should succeed");
    for i in 0..n {
        map.insert_nested("outer".to_owned(), format!("inner{i}"), "value".to_owned())
            .expect("insert_nested should succeed");
    }
    map
}

fn build_map(n: usize) -> Root<UnorderedMap<String, String, MainStorage>> {
    let mut map = Root::new(UnorderedMap::<String, String, MainStorage>::new);
    for i in 0..n {
        map.insert(format!("key{i}"), "value".to_owned())
            .expect("insert should succeed");
    }
    map
}

fn build_vector(n: usize) -> Root<Vector<String, MainStorage>> {
    let mut vector = Root::new(Vector::<String, MainStorage>::new);
    for i in 0..n {
        vector
            .push(format!("value{i}"))
            .expect("push should succeed");
    }
    vector
}

/// Every workload at every size. `SortedMap` and `SortedSet` are absent
/// because their index ops bypass this crate's counting callbacks entirely,
/// so a workload here would report zero for the index maintenance it exists
/// to measure; adding them means wiring `IndexCallbacks` through first.
pub fn all() -> Vec<Workload> {
    use CostShape::{ConstantPerCall, FlatPerEntry, KnownLinearInN, QuadraticBuild};

    /// A size-independent registry row, crossed with [`SIZES`] below.
    type Entry = (&'static str, CostShape, u32, fn(usize));

    const REGISTRY: [Entry; 13] = [
        (
            "unordered_map_insert",
            FlatPerEntry,
            0,
            unordered_map_insert,
        ),
        ("vector_push", FlatPerEntry, 0, vector_push),
        ("unordered_map_len", ConstantPerCall, 0, unordered_map_len),
        ("unordered_map_get", ConstantPerCall, 0, unordered_map_get),
        // Walks the whole trie, so its node count follows the random id
        // distribution: 18% bounds the spread seen at n=10, the smallest and
        // therefore noisiest size.
        ("vector_get_nth", KnownLinearInN, 18, vector_get_nth),
        ("rga_insert", FlatPerEntry, 0, rga_insert),
        // Tolerance 0 unlike `vector_get_nth`: `get_text` sorts in memory
        // rather than descending the trie, so no bucket randomness applies.
        ("rga_get_nth", KnownLinearInN, 0, rga_get_nth),
        ("lww_register_set", FlatPerEntry, 0, lww_register_set),
        ("nested_map_insert", FlatPerEntry, 0, nested_map_insert),
        ("nested_map_get", ConstantPerCall, 0, nested_map_get),
        (
            "fugue_text_insert_per_char",
            FlatPerEntry,
            0,
            fugue_text_insert_per_char,
        ),
        ("fugue_text_insert", FlatPerEntry, 0, fugue_text_insert),
        ("fugue_text_char_at", KnownLinearInN, 0, fugue_text_char_at),
    ];

    /// Rows crossed with [`QUADRATIC_SIZES`]; a separate array because
    /// `REGISTRY` is crossed with `SIZES` unconditionally.
    const QUADRATIC_REGISTRY: [Entry; 5] = [
        (
            "rga_insert_per_char",
            QuadraticBuild,
            0,
            rga_insert_per_char,
        ),
        // Remote characters use random entity ids, so rows_read does not reproduce exactly.
        (
            "rga_insert_interleaved_sync",
            QuadraticBuild,
            5,
            rga_insert_interleaved_sync,
        ),
        (
            "fugue_text_insert_middle",
            QuadraticBuild,
            0,
            fugue_text_insert_middle,
        ),
        (
            "fugue_text_char_at_fragmented",
            KnownLinearInN,
            0,
            fugue_text_char_at_fragmented,
        ),
        (
            "fugue_text_insert_interleaved_sync",
            QuadraticBuild,
            0,
            fugue_text_insert_interleaved_sync,
        ),
    ];

    let mut out = Vec::with_capacity(
        REGISTRY.len() * SIZES.len() + QUADRATIC_REGISTRY.len() * QUADRATIC_SIZES.len(),
    );
    for n in SIZES {
        for (name, shape, tolerance_pct, run) in REGISTRY {
            out.push(Workload {
                name,
                n,
                shape,
                tolerance_pct,
                run,
            });
        }
    }
    for n in QUADRATIC_SIZES {
        for (name, shape, tolerance_pct, run) in QUADRATIC_REGISTRY {
            out.push(Workload {
                name,
                n,
                shape,
                tolerance_pct,
                run,
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::measure;

    const RANGE_READ_CHARS: usize = 100;

    type LandingCase = (&'static str, fn(usize), fn() -> String);

    type ReadCase = (&'static str, usize, fn(usize) -> (Option<char>, String));

    #[ignore = "minutes of work; run by the release storage-cost CI job via --include-ignored"]
    #[test]
    fn every_workload_is_measurable_and_touches_storage() {
        for workload in all() {
            let (_, costs) = measure(|| (workload.run)(workload.n));
            let touched = costs.rows_read + costs.rows_written + costs.rows_removed;
            assert!(
                touched > 0,
                "workload {} at n={} performed no storage operations: {costs:?} — a \
                 workload that measures nothing gates nothing",
                workload.name,
                workload.n
            );
        }
    }

    /// A point workload that forgot `reset_counters()` would silently report
    /// its build cost and look linear no matter what the operation does.
    #[test]
    fn point_workloads_cost_far_less_than_the_build_they_follow() {
        let n = 1_000;
        let (_, build) = measure(|| unordered_map_insert(n));
        let (_, point) = measure(|| unordered_map_get(n));

        assert!(
            point.rows_read * 10 < build.rows_read,
            "unordered_map_get at n={n} read {} rows against a build of {} — it is \
             reporting the build, so reset_counters() is not taking effect",
            point.rows_read,
            build.rows_read
        );
    }

    #[test]
    fn cost_does_not_depend_on_what_ran_before() {
        let n = 100;
        let (_, clean) = measure(|| rga_insert_interleaved_sync(n));

        // `build_map` never commits, so its actions stay queued: that is the pollution.
        let (_, _) = measure(|| {
            let _ignored = build_map(1_000);
        });
        let (_, after) = measure(|| rga_insert_interleaved_sync(n));

        let drift_pct = (after.rows_read as f64 - clean.rows_read as f64).abs() * 100.0
            / clean.rows_read as f64;
        assert!(
            drift_pct < 5.0,
            "rga_insert_interleaved_sync read {} rows on a clean thread and {} rows behind \
             an uncommitted build ({drift_pct:.1}% apart): its cost now depends on registry \
             order, so it is measuring someone else's writes as sync cost",
            clean.rows_read,
            after.rows_read
        );
    }

    #[test]
    fn every_remote_character_actually_lands() {
        let n = 10;
        let cases: [LandingCase; 2] = [
            (
                "rga_insert_interleaved_sync",
                rga_insert_interleaved_sync,
                || {
                    Root::<ReplicatedGrowableArray<MainStorage>>::fetch()
                        .expect("document root should exist after the workload")
                        .get_text()
                        .expect("get_text should succeed")
                },
            ),
            (
                "fugue_text_insert_interleaved_sync",
                fugue_text_insert_interleaved_sync,
                || {
                    Root::<FugueText<MainStorage>>::fetch()
                        .expect("document root should exist after the workload")
                        .get_text()
                        .expect("get_text should succeed")
                },
            ),
        ];

        for (name, workload, read) in cases {
            let (text, _) = measure(|| {
                workload(n);
                read()
            });

            assert_eq!(
                text.chars().count(),
                n,
                "{name}(n={n}) left a {}-character document ({text:?}): either the remote \
                 half is not landing through apply_action, or the two replicas are minting \
                 colliding node ids - both make this workload a single-replica build under \
                 a sync name",
                text.chars().count(),
            );
            assert!(
                text.contains('r') && text.contains('a'),
                "{name} left {text:?}, missing one side of the interleave - local chars are \
                 'a', remote chars are 'r', and both must be present"
            );
        }
    }

    #[test]
    fn positional_reads_return_real_characters() {
        let cases: [ReadCase; 1] = [("fugue_text", 1_000, |n| {
            let text = build_fugue_text(n);
            let start = n / 2;
            (
                text.char_at(start).expect("char_at should succeed"),
                text.text_range(start, start + RANGE_READ_CHARS)
                    .expect("text_range should succeed"),
            )
        })];

        for (name, n, read) in cases {
            let ((one, range), _) = measure(|| read(n));

            assert_eq!(
                one,
                Some('a'),
                "char_at({}) returned {one:?} against a {n}-character {name} document - the \
                 cost curve is measuring a read that finds nothing",
                n / 2
            );
            assert_eq!(
                range.chars().count(),
                RANGE_READ_CHARS,
                "text_range returned {} characters, not {RANGE_READ_CHARS} - the {name} cost \
                 curve is measuring a shorter read than it reports",
                range.chars().count()
            );
        }
    }

    #[test]
    fn workload_names_and_sizes_are_unique() {
        let mut seen = std::collections::BTreeSet::new();
        for workload in all() {
            assert!(
                seen.insert((workload.name, workload.n)),
                "duplicate workload {} at n={}",
                workload.name,
                workload.n
            );
        }
    }
}
