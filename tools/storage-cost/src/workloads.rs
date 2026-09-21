//! The single registry of measured workloads.
//!
//! The binary, the flat-curve tests and the criterion benches all iterate
//! `all()`. Defining a workload anywhere else would let the gate and the
//! benchmarks measure different things while claiming to measure one.
//!
//! # Two kinds of workload
//!
//! A *build* workload does `n` operations and is measured whole: its total cost
//! is expected to grow with `n`, and what must stay flat is cost **per entry**.
//!
//! A *point* workload builds `n` entries, calls [`crate::reset_counters`], and
//! then performs exactly one operation. What it reports is the cost of that one
//! operation with `n` entries already in the collection — which is the number
//! that decides whether a collection stays readable as it grows.
//!
//! [`CostShape`] says which is which, and, for point workloads, whether the
//! curve is required to be flat or is a known-linear cost being held under
//! observation.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

use calimero_storage::action::Action;
use calimero_storage::collections::{
    FugueText, FugueTextSimple, LwwRegister, NestedMapOps, ReplicatedGrowableArray, Root,
    UnorderedMap, UnorderedSet, Vector,
};
use calimero_storage::delta::{clear_pending_delta, StorageDelta};
use calimero_storage::env::{take_last_artifact, with_runtime_env, RuntimeEnv};
use calimero_storage::interface::{ApplyContext, Interface};
use calimero_storage::store::{Key, MainStorage};

use crate::reset_counters;

/// Collection sizes every workload is measured at. The gate compares costs at
/// each size; the shape tests compare the first against the last.
pub const SIZES: [usize; 4] = [10, 100, 1_000, 10_000];

/// Collection sizes for [`CostShape::QuadraticBuild`] workloads only.
///
/// Deliberately smaller than [`SIZES`]. `rga_insert_per_char`'s TOTAL cost is
/// `O(n^2)`, not `O(n)`, so `SIZES`'s top row would not merely be slower — it
/// would be the wrong shape of slower: `n=10_000` measured (see the module's
/// dev notes) at roughly `19s` for `n=5_000` alone, so `n=10_000` is close to
/// a minute for ONE measurement, and every consumer of `all()` measures each
/// workload multiple times (`tests/reproducible.rs` runs it 7x per size,
/// `tests/flat_curve.rs` and the snapshot binary run it once per size, and
/// `benches/collections.rs` iterates it under criterion). `2_000` keeps the
/// slowest single measurement under ~3s — the asymptotic slope is already
/// unambiguous well before `10_000`, since `reads/entry` climbs from `63.5`
/// at `n=10` to `2047.0` at `n=2_000`, tracking `n` almost 1:1 by the top of
/// this range (see `rga_insert_per_char`'s doc comment for the measured
/// curve in full).
pub const QUADRATIC_SIZES: [usize; 4] = [10, 100, 500, 2_000];

/// What the cost curve of a workload is required to look like.
///
/// This is an assertion, not a description. Every variant is checked by
/// `tests/flat_curve.rs`, including [`Self::KnownLinearInN`] — a workload that
/// stops being linear fails just as loudly as one that starts being linear,
/// because "we fixed it and nobody updated the marker" and "we broke something
/// else" must not be told apart by guesswork.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CostShape {
    /// A build of `n` entries. Cost **per entry** must not grow with `n`.
    FlatPerEntry,
    /// One operation against a collection of `n` entries. Its **total** cost
    /// must not grow with `n`.
    ConstantPerCall,
    /// One operation whose total cost is KNOWN to grow linearly with `n`.
    ///
    /// This is not a licence — it is a ratchet. See `ordered_read` below.
    KnownLinearInN,
    /// A build of `n` operations whose TOTAL cost is KNOWN to grow
    /// quadratically with `n` — the operation itself is `O(n)` per call, so a
    /// loop of `n` calls is `O(n^2)` overall.
    ///
    /// This does not fit either of the other two shapes, and declaring it as
    /// one would assert the wrong thing:
    ///
    /// - [`Self::FlatPerEntry`] asserts per-entry cost does NOT grow with
    ///   `n`. Here it does, by construction — that growth is the finding.
    /// - [`Self::KnownLinearInN`] is for a POINT operation (build `n`, reset
    ///   counters, do ONE more call) whose single call costs `O(n)`. A
    ///   `QuadraticBuild` workload has no such point call to isolate — the
    ///   `O(n)` cost is paid on every one of the `n` calls that make up the
    ///   build, not on an `n+1`th call after it.
    ///
    /// Like [`Self::KnownLinearInN`], this is a ratchet, not a licence: see
    /// `tests/flat_curve.rs`'s `quadratic_build_costs_are_still_exactly_
    /// quadratic`, which fails if the curve gets worse (superquadratic) AND
    /// if it silently gets better (the fix nobody recorded).
    ///
    /// Measured at [`QUADRATIC_SIZES`], not [`SIZES`] — see that constant's
    /// doc comment for why.
    QuadraticBuild,
}

/// One measurable unit of work at one collection size.
pub struct Workload {
    /// Stable identifier. Appears in the snapshot, so renaming one is a
    /// snapshot change a reviewer will see.
    pub name: &'static str,
    /// Collection size this instance exercises.
    pub n: usize,
    /// The curve this workload's cost is asserted to follow.
    pub shape: CostShape,
    /// How far a measured row count may sit from the committed snapshot before
    /// the gate fails, as a percentage.
    ///
    /// Zero for almost everything: row counts reproduce exactly. It is nonzero
    /// only where the operation walks the WHOLE child trie, because the trie's
    /// node count depends on how random entity ids happened to distribute, so
    /// the number of node reads varies run to run.
    ///
    /// This is a measured property, not a guess — `tests/reproducible.rs`
    /// re-derives the spread of every workload and fails if a declared
    /// tolerance is either too tight (flaky gate) or gratuitously loose
    /// (blind gate).
    pub tolerance_pct: u32,
    /// Builds a collection of `n` entries and performs the measured operation.
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

/// Insert `n` entries into an `UnorderedSet`, measuring the whole build.
fn unordered_set_insert(n: usize) {
    let mut set = Root::new(UnorderedSet::<String, MainStorage>::new);
    for i in 0..n {
        let _ignored = set
            .insert(format!("value{i}"))
            .expect("insert should succeed");
    }
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

/// Cost of ONE positional read — `Vector::get(i)` — against `n` entries.
///
/// # Why this is `KnownLinearInN`, and what it is standing in for
///
/// This is the in-repo fixture for a read wall measured in mero-chat, whose
/// `get_messages` exhausts a 1e9 gas budget at ~32,000 messages, with 30,000
/// already spending 99.83% of it. The cause is not the app. It is this call:
///
/// ```text
/// Vector::get(i) -> Collection::nth(i) -> children_cache()
///                -> Index::get_children_of(parent)
///                -> ChildTrie::children()   // collect ALL, then sort
/// ```
///
/// One O(n) id walk per positional read, because order lives in a comparator
/// applied after a full enumeration rather than in the structure. The child
/// trie bounded *write* cost; it never touched ordered *read* cost.
///
/// Fixing that is a real project (see the ordering design doc) and is
/// deliberately not attempted here. What this workload does is make the cost
/// **gated** instead of merely known: the snapshot pins the constant, and
/// `tests/flat_curve.rs` pins the slope. Nobody can make it quietly worse, and
/// nobody can fix it without the marker below going red and forcing this
/// comment to be rewritten.
///
/// The middle index is read, not the first, so a hypothetical fast path for
/// index 0 could not make the measurement lie.
fn vector_get_nth(n: usize) {
    let vector = build_vector(n);
    reset_counters();
    let _ignored = vector.get(n / 2).expect("get should succeed");
}

/// Bulk-insert `n` characters into an empty RGA as a single `insert_str` call,
/// measuring the whole build.
///
/// This is deliberately NOT `n` calls to [`ReplicatedGrowableArray::insert`]
/// (one char, one position, at a time). That per-char loop is a real usage
/// pattern (typing), but it is also genuinely `O(n)` *per insert* — every call
/// re-derives the left-neighbour by linearising the whole document
/// (`get_ordered_chars`, see `rga.rs`), so a loop of `n` such inserts is
/// `O(n^2)` overall. That is a real cost, not a measurement artefact, but it
/// is a different question from "is a build flat per entry", and conflating
/// them here would make this workload fail for the wrong reason. `insert_str`
/// linearises the document exactly ONCE (to find the single left-neighbour
/// for the whole batch) and then does `n` flat `UnorderedMap` inserts — the
/// realistic shape for "paste one string", which is genuinely flat per entry.
fn rga_insert(n: usize) {
    build_rga(n);
}

/// Read the whole RGA document after building `n` characters.
///
/// # Why this measures `get_text()`, not a positional `get(i)`
///
/// `ReplicatedGrowableArray` has no positional read at all — no `get(i)`
/// analogous to `Vector::get`. The only public read is [`get_text`], which
/// materialises the entire document. That is not a bug to work around here:
/// it is the RGA read wall in its purest form, one step past `vector_get_nth`
/// below. `Vector::get(i)` at least *tries* to return one element and pays an
/// accidental `O(n)` cost doing it; `ReplicatedGrowableArray` was never given
/// a positional read to begin with, so EVERY read of it is `O(n)` by
/// construction. Declared `KnownLinearInN`, same as `vector_get_nth`, and for
/// the same underlying reason: no ordered index, only a full linearisation on
/// every read.
///
/// [`get_text`]: calimero_storage::collections::ReplicatedGrowableArray::get_text
fn rga_get_nth(n: usize) {
    let rga = build_rga(n);
    reset_counters();
    let _ignored = rga.get_text().expect("get_text should succeed");
}

/// Insert `n` characters into an RGA ONE AT A TIME via
/// [`ReplicatedGrowableArray::insert`], each appended at the current end —
/// the "someone is typing" access pattern, as opposed to `rga_insert`'s
/// single bulk `insert_str` (the "paste one string" pattern).
///
/// # Why this is `QuadraticBuild`, and what it re-derives
///
/// `insert(pos, char)` re-derives its left-neighbour by linearising the
/// WHOLE document on every call (`get_ordered_chars`, see `rga.rs`) — an
/// `O(current length)` cost paid once per call. A loop of `n` such calls is
/// therefore `O(n^2)` in total, not `O(n)`: this is exactly the gap
/// `rga_insert`'s own doc comment names and deliberately does not measure,
/// because `insert_str` linearises only ONCE for the whole batch. This
/// workload is the per-call route `rga_insert` is not, and per-call is what
/// a real editor actually does.
///
/// Measured `reads/entry` (`rows_read / n`, i.e. the AVERAGE cost of one
/// `insert` call over the build) at [`QUADRATIC_SIZES`]:
///
/// | `n`   | reads/entry |
/// |-------|-------------|
/// | 10    | 63.5        |
/// | 100   | 147.7       |
/// | 500   | 547.1       |
/// | 2,000 | 2,047.0     |
///
/// The average tracks `n` almost 1:1 above a small constant offset (~47,
/// from the fixed per-call bookkeeping outside the linearisation) — the
/// signature of a per-call cost that is itself linear in the CURRENT size,
/// summed over a build that grows to `n`. That is what
/// `CostShape::QuadraticBuild` asserts stays true: not a flat per-entry cost
/// (that would be `FlatPerEntry`, and it is not what happens here), but a
/// per-entry AVERAGE that itself climbs with `n`.
fn rga_insert_per_char(n: usize) {
    let mut rga = Root::new(ReplicatedGrowableArray::<MainStorage>::new);
    for i in 0..n {
        rga.insert(i, 'a').expect("insert should succeed");
    }
}

/// Insert `n` characters one at a time at the MIDDLE of the document, the
/// position no end-anchored fast path can serve where `rga_insert_per_char`
/// only ever appends. Costs the same as that workload today, because `insert`
/// linearises the whole document before it looks at `pos` at all.
fn rga_insert_middle(n: usize) {
    let mut rga = Root::new(ReplicatedGrowableArray::<MainStorage>::new);
    for i in 0..n {
        rga.insert(i / 2, 'a').expect("insert should succeed");
    }
}

/// Build `n` characters, alternating local writes with remote arrivals: the
/// only RGA workload here that receives, and so the only one that can see a
/// cost paid per REMOTE write.
///
/// The remote half goes through [`Interface::apply_action`], the real receive
/// path, not a second local `insert` wearing a remote label. The root is
/// re-fetched per iteration because a `Root` handle caches its children and
/// would otherwise never observe what landed. `n / 2` iterations produce `n`
/// characters, so `n` means what it means for `rga_insert_per_char` and the two
/// curves are comparable. [`CostShape::QuadraticBuild`]: a remote landing is
/// itself linear in the document and is paid on top of the local cost.
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
        land_remote_char();
    }
}

/// Field name shared by the local document and the remote replica in
/// [`rga_insert_interleaved_sync`]. It must be a field name and not
/// [`ReplicatedGrowableArray::new`]'s random id, or the two replicas derive
/// different collection ids and quietly build two documents that never meet.
const INTERLEAVED_DOC_FIELD: &str = "interleaved_doc";

/// Apply one character the way the SYNC path does: author it on a SEPARATE
/// replica, over its own backing map so none of the authoring cost is counted,
/// then replay that replica's delta through [`Interface::apply_action`]. The
/// root action is skipped because the receiver's root is not the sender's to
/// overwrite.
fn land_remote_char() {
    for action in remote_char_actions() {
        if action.id().is_root() {
            continue;
        }
        Interface::<MainStorage>::apply_action(action, &ApplyContext::empty())
            .expect("remote apply_action should succeed");
    }
}

/// The actions a remote replica emits when one character is typed into it.
///
/// `calimero_storage::delta`'s pending-action buffer is THREAD-local, and any
/// `commit()` drains all of it into the artifact, so another workload's
/// uncommitted writes could ride along and be reported as sync cost.
/// `clear_pending_delta()` makes the isolation a property of this function
/// rather than of where the caller happens to commit; the count assertion is
/// what actually detects a leak however it arrives.
fn remote_char_actions() -> Vec<Action> {
    clear_pending_delta();
    let delta = with_runtime_env(uncounted_env(), || {
        let mut rga = Root::new(|| {
            ReplicatedGrowableArray::<MainStorage>::new_with_field_name(INTERLEAVED_DOC_FIELD)
        });
        rga.insert(0, 'r').expect("remote insert should succeed");
        rga.commit();
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
         typing ONE character emits. Either the thread-local pending-delta buffer leaked into \
         it (check that `clear_pending_delta()` above still runs, so this workload is not about \
         to re-apply writes that were already done and report the cost as sync), or a \
         legitimate storage-layer change altered what one character insert emits, in which case \
         move REMOTE_CHAR_ACTIONS deliberately per its doc comment and regenerate the snapshot.",
        actions.len()
    );
    actions
}

/// Actions in the delta a remote replica emits for exactly one character.
/// A fixed COUNT rather than an id filter, because the likeliest pollution is
/// the caller's own inserts into the very same document, which share its
/// collection id and would pass an id check.
const REMOTE_CHAR_ACTIONS: usize = 6;

/// A throwaway `RuntimeEnv` over its own map, deliberately NOT wired to
/// [`crate::measure`]'s counters: what is measured is what the RECEIVER pays,
/// so the sender's own writes must not be counted.
fn uncounted_env() -> RuntimeEnv {
    uncounted_env_with_device([2; 32])
}

/// [`uncounted_env`] with an explicit device id.
///
/// `FugueText` mints node ids as `(replica, counter)` with `replica` derived
/// from the DEVICE id, so a remote replica sharing the measurement env's device
/// id would mint the very ids the local writer is minting and the two halves
/// would collide instead of interleaving. `ReplicatedGrowableArray` has no such
/// hazard: its ids carry an HLC timestamp from a clock both envs share.
fn uncounted_env_with_device(device_id: [u8; 32]) -> RuntimeEnv {
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

/// ONE `insert_str` of `n` characters into an empty `FugueText`: the cost of a
/// PASTE, where [`fugue_text_insert_per_char`] is the keystroke row.
///
/// [`CostShape::FlatPerEntry`], and per-entry cost FALLS with `n`: the Fugue
/// insert rule is resolved for the first character only, since every later
/// character is the right child of the one before it, which is the edge a run
/// already carries. A paste therefore touches `ceil(n / MAX_RUN_LEN)` blocks.
fn fugue_text_insert(n: usize) {
    let _ignored = build_fugue_text(n);
}

/// Cost of ONE `get_text` against a FRAGMENTED document of `n` characters: the
/// counterpart of [`fugue_text_get_text`], which reads a pasted document, and
/// the honest number for what a real editing session produces.
fn fugue_text_get_text_fragmented(n: usize) {
    let text = build_fugue_text_fragmented(n);
    reset_counters();
    let _ignored = text.get_text().expect("get_text should succeed");
}

/// Cost of ONE `char_at` against a FRAGMENTED document: the counterpart of
/// [`fugue_text_char_at`]. Positional reads have no fast path here, answering
/// one still means loading every block and rebuilding the tree.
fn fugue_text_char_at_fragmented(n: usize) {
    let text = build_fugue_text_fragmented(n);
    reset_counters();
    let _ignored = text.char_at(n / 2).expect("char_at should succeed");
}

/// Cost of ONE `text_range` (a screenful) against a FRAGMENTED document: the
/// counterpart of [`fugue_text_text_range`].
fn fugue_text_text_range_fragmented(n: usize) {
    let text = build_fugue_text_fragmented(n);
    reset_counters();
    let start = n / 2;
    let end = (start + RANGE_READ_CHARS).min(n);
    let _ignored = text
        .text_range(start, end)
        .expect("text_range should succeed");
}

/// Cost of ONE `get_text` against a PASTED document of `n` characters: rows
/// track BLOCKS, `ceil(n / MAX_RUN_LEN)` of them, not characters. Every row a
/// `FugueText` read touches goes through this crate's counters, so unlike
/// `SortedMap` (see [`all`]) nothing here is measured as free.
fn fugue_text_get_text(n: usize) {
    let text = build_fugue_text(n);
    reset_counters();
    let _ignored = text.get_text().expect("get_text should succeed");
}

/// Cost of ONE `char_at` against a document of `n` characters, a capability
/// `ReplicatedGrowableArray` never had at all. The MIDDLE position is read, not
/// the first, so a hypothetical fast path for position 0 could not make the
/// measurement lie. Answering it still drags every block through borsh, so the
/// cost is [`fugue_text_get_text`]'s.
fn fugue_text_char_at(n: usize) {
    let text = build_fugue_text(n);
    reset_counters();
    let _ignored = text.char_at(n / 2).expect("char_at should succeed");
}

/// A screenful, not the document: fixed rather than a fraction of `n`, or the
/// window would be linear in `n` by construction and the question here is
/// whether a BOUNDED window costs more as text accumulates around it.
const RANGE_READ_CHARS: usize = 100;

/// Cost of ONE short [`FugueText::text_range`] read against a document of `n`,
/// from the MIDDLE for [`fugue_text_char_at`]'s reason. At `n = 10` the window
/// is longer than the document and clamps; that smallest size is the baseline
/// the growth ratio is taken against, so clamping cannot flatter the curve.
fn fugue_text_text_range(n: usize) {
    let text = build_fugue_text(n);
    reset_counters();
    let start = n / 2;
    let _ignored = text
        .text_range(start, start + RANGE_READ_CHARS)
        .expect("text_range should succeed");
}

/// `n` SEPARATE `insert` calls on a `FugueText`, each appending one character
/// at the current end: `n` keystrokes, where [`fugue_text_insert`] is one
/// paste, and the counterpart of [`rga_insert_per_char`].
///
/// [`CostShape::FlatPerEntry`] where its RGA counterpart is quadratic: an
/// append extends the tail run in place, so one keystroke touches one block row
/// whatever the document weighs. Rows cannot see the BYTES rewritten per
/// keystroke, which grow without bound unless the run is capped; that is what
/// `tests/keystroke_bytes.rs` gates, since bytes flake too much to snapshot.
fn fugue_text_insert_per_char(n: usize) {
    let mut text = Root::new(FugueText::<MainStorage>::new);
    for i in 0..n {
        text.insert(i, 'a').expect("insert should succeed");
    }
}

/// Insert `n` characters one at a time at the MIDDLE of the document: the
/// counterpart of [`rga_insert_middle`], and the position no end-anchored fast
/// path can serve.
///
/// [`CostShape::QuadraticBuild`], at parity with RGA rather than better than
/// it, because `load()` reads every block on each call and each distinct
/// insertion point is its own block. That is what run-length blocks do not buy:
/// only APPENDS coalesce (see [`fugue_text_insert_per_char`]).
fn fugue_text_insert_middle(n: usize) {
    let mut text = Root::new(FugueText::<MainStorage>::new);
    for i in 0..n {
        text.insert(i / 2, 'a').expect("insert should succeed");
    }
}

/// Build `n` `FugueText` characters half local and half arriving from a remote
/// replica: the counterpart of [`rga_insert_interleaved_sync`], whose doc
/// comment covers every structural choice made here.
///
/// [`CostShape::QuadraticBuild`], and NOT the win the other Fugue workloads
/// are: each remote character is anchored at position 0 and splits the run it
/// lands in, so the block count grows with the document and `load()` reads
/// every block on the next call. Tolerance `0` unlike its RGA counterpart's
/// `5`, because the remote replica is created with a field name and its blocks
/// are keyed by `BlockKey`, so every id is derived rather than drawn.
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

/// Field name shared by the local document and the remote replica in
/// [`fugue_text_insert_interleaved_sync`]; see [`INTERLEAVED_DOC_FIELD`] for
/// why a field name and not a random id.
const FUGUE_INTERLEAVED_FIELD: &str = "interleaved_fugue_doc";

/// The device id the `index`-th remote `FugueText` character is authored under.
///
/// It must differ from the measurement env's (see
/// [`uncounted_env_with_device`]) and per CHARACTER: a node id is
/// `(replica, counter)` with the counter derived from the blocks that replica
/// already holds in the AUTHORING store, and every remote character is authored
/// on a fresh store, so a fixed device id would mint the same id every time and
/// the receiver would treat each one as a duplicate rather than as new text.
/// Each remote character therefore arrives from a different replica, which
/// bounds remote cost from one side rather than modelling two long-lived
/// writers.
fn remote_fugue_device(index: usize) -> [u8; 32] {
    let mut device = [9_u8; 32];
    // `local_replica` reads the first 8 bytes big-endian, so the replica id is
    // `index + 1`: distinct per character and clear of the measurement env's.
    device[..8].copy_from_slice(&(index as u64 + 1).to_be_bytes());
    device
}

/// Apply one character to the `FugueText` document the way the SYNC path does;
/// [`land_remote_char`]'s counterpart, same decode-and-replay shape.
fn land_remote_fugue_char(index: usize) {
    for action in remote_fugue_char_actions(index) {
        if action.id().is_root() {
            continue;
        }
        Interface::<MainStorage>::apply_action(action, &ApplyContext::empty())
            .expect("remote apply_action should succeed");
    }
}

/// The actions a remote `FugueText` replica emits when one character is typed.
/// The thread-local pending-delta hazard, and the fixed-count guard against it,
/// are [`remote_char_actions`]'s; only the collection and device id differ.
fn remote_fugue_char_actions(index: usize) -> Vec<Action> {
    clear_pending_delta();
    let delta = with_runtime_env(
        uncounted_env_with_device(remote_fugue_device(index)),
        || {
            let mut text = Root::new(|| {
                FugueText::<MainStorage>::new_with_field_name(FUGUE_INTERLEAVED_FIELD)
            });
            text.insert(0, 'r').expect("remote insert should succeed");
            text.commit();
            take_last_artifact().expect("commit should emit a delta")
        },
    );
    let actions = match borsh::from_slice::<StorageDelta>(&delta).expect("delta should decode") {
        StorageDelta::Actions(actions) => actions,
        StorageDelta::CausalActions { actions, .. } => actions,
    };
    assert_eq!(
        actions.len(),
        REMOTE_FUGUE_CHAR_ACTIONS,
        "the remote replica's delta carried {} actions, not the \
         {REMOTE_FUGUE_CHAR_ACTIONS} that typing ONE character into a FugueText emits. \
         Either the thread-local pending-delta buffer leaked into it (check that \
         `clear_pending_delta()` above still runs, so this workload is not about to \
         re-apply writes that were already done and report the cost as sync), or a \
         change to FugueText altered what one character insert emits - in which case \
         move REMOTE_FUGUE_CHAR_ACTIONS deliberately per its doc comment and regenerate \
         the snapshot, because the applied cost changed too.",
        actions.len()
    );
    actions
}

/// Actions in the delta a remote replica emits for exactly one `FugueText`
/// character. Measured, and stable across runs; see [`REMOTE_CHAR_ACTIONS`] for
/// why this is a fixed COUNT rather than an id filter.
const REMOTE_FUGUE_CHAR_ACTIONS: usize = 6;

// ---------------------------------------------------------------------------
// `FugueTextSimple`: the one-entity-per-node control.
//
// Every workload below mirrors its `fugue_text_*` twin line for line, so the
// only difference within a pair is whether sequentially-inserted nodes condense
// into one entity. `RGA -> simple` then isolates Fugue's ORDERING, and
// `simple -> fugue_text` isolates run-length BLOCKS.
//
// All seven are measured at QUADRATIC_SIZES, including the three point READS.
// That is forced: one entity per character makes the control's `insert_str`
// re-derive the tree once per character, so BUILDING the document is `O(n^2)`
// whatever is measured afterwards.
// ---------------------------------------------------------------------------

/// Bulk-insert `n` characters as a single `insert_str` call: the control for
/// [`fugue_text_insert`].
///
/// [`CostShape::QuadraticBuild`], and byte-for-byte equal to
/// `rga_insert_per_char` at every size, which is half the finding: with blocks
/// removed, Fugue's ordering costs exactly what RGA's does. With nothing to
/// coalesce into there is no run to resolve a whole string against, so
/// `insert_str` loops per character and each call re-derives the whole tree.
fn fugue_simple_insert(n: usize) {
    let _ignored = build_fugue_simple(n);
}

/// Read the whole document after building `n` characters: the control for
/// [`fugue_text_get_text`].
///
/// [`CostShape::KnownLinearInN`] at exactly `2n` rows, matching `rga_get_nth`.
/// This is where blocks carry the whole win: a read is `O(entities)`, and
/// blocks are what make `entities` count runs instead of characters.
fn fugue_simple_get_text(n: usize) {
    let text = build_fugue_simple(n);
    reset_counters();
    let _ignored = text.get_text().expect("get_text should succeed");
}

/// Cost of ONE `char_at` against a document of `n` characters, read from the
/// MIDDLE: the control for [`fugue_text_char_at`].
///
/// [`CostShape::KnownLinearInN`] at `2n` rows, identical to
/// [`fugue_simple_get_text`] because both linearise the whole document: without
/// blocks there is no positional read worth the name.
fn fugue_simple_char_at(n: usize) {
    let text = build_fugue_simple(n);
    reset_counters();
    let _ignored = text.char_at(n / 2).expect("char_at should succeed");
}

/// Cost of ONE short `text_range` read against a document of `n`: the control
/// for [`fugue_text_text_range`], same window and same clamping behaviour.
///
/// [`CostShape::KnownLinearInN`] at `2n` rows: a bounded window costs the whole
/// document when the document is one entity per character.
fn fugue_simple_text_range(n: usize) {
    let text = build_fugue_simple(n);
    reset_counters();
    let start = n / 2;
    let _ignored = text
        .text_range(start, start + RANGE_READ_CHARS)
        .expect("text_range should succeed");
}

/// Insert `n` characters ONE AT A TIME at the current end: the control for
/// [`fugue_text_insert_per_char`], and where blocks matter most, because an
/// append is precisely what coalesces.
///
/// [`CostShape::QuadraticBuild`] where its twin is
/// [`CostShape::FlatPerEntry`], and byte-identical to `rga_insert_per_char` at
/// every shared size. Typing is flat because of BLOCKS, not because of Fugue.
fn fugue_simple_insert_per_char(n: usize) {
    let mut text = Root::new(FugueTextSimple::<MainStorage>::new);
    for i in 0..n {
        text.insert(i, 'a').expect("insert should succeed");
    }
}

/// Insert `n` characters one at a time at the MIDDLE: the control for
/// [`fugue_text_insert_middle`], the position no coalescing can serve.
///
/// [`CostShape::QuadraticBuild`], within 0.1% of both neighbours, which is how
/// little blocks buy here: the run an advancing caret coalesces into is one
/// entity out of `n` already stored, so the `O(entities)` re-derivation that
/// dominates the call is unchanged.
fn fugue_simple_insert_middle(n: usize) {
    let mut text = Root::new(FugueTextSimple::<MainStorage>::new);
    for i in 0..n {
        text.insert(i / 2, 'a').expect("insert should succeed");
    }
}

/// Build `n` characters half local and half arriving from a remote replica: the
/// control for [`fugue_text_insert_interleaved_sync`], with the remote half
/// going through [`Interface::apply_action`] exactly as its twin does.
///
/// [`CostShape::QuadraticBuild`], but ~2x CHEAPER than the blocked collection
/// at every size, so blocks are a net LOSS on the receive path: once remote
/// characters have shattered the document into many runs, every mutating
/// `FugueText` call ends in `normalise_blocks`, which loads every block and
/// rebuilds the tree a SECOND time. Tolerance `0` for its twin's reason, every
/// id here is derived rather than drawn.
fn fugue_simple_insert_interleaved_sync(n: usize) {
    let text = Root::new(|| {
        FugueTextSimple::<MainStorage>::new_with_field_name(FUGUE_SIMPLE_INTERLEAVED_FIELD)
    });
    text.commit();
    for i in 0..n / 2 {
        let mut text =
            Root::<FugueTextSimple<MainStorage>>::fetch().expect("document root should exist");
        text.insert(i, 'a').expect("local insert should succeed");
        text.commit();
        land_remote_fugue_simple_char(i);
    }
}

/// Field name shared by the local document and the remote replica in
/// [`fugue_simple_insert_interleaved_sync`].
const FUGUE_SIMPLE_INTERLEAVED_FIELD: &str = "interleaved_fugue_simple_doc";

/// Apply one character the way the SYNC path does;
/// [`land_remote_fugue_char`]'s counterpart, same decode-and-replay shape.
fn land_remote_fugue_simple_char(index: usize) {
    for action in remote_fugue_simple_char_actions(index) {
        if action.id().is_root() {
            continue;
        }
        Interface::<MainStorage>::apply_action(action, &ApplyContext::empty())
            .expect("remote apply_action should succeed");
    }
}

/// The actions a remote `FugueTextSimple` replica emits for one character.
/// The per-character device id and the fixed-count guard are
/// [`remote_fugue_char_actions`]'s; only the collection differs.
fn remote_fugue_simple_char_actions(index: usize) -> Vec<Action> {
    clear_pending_delta();
    let delta = with_runtime_env(
        uncounted_env_with_device(remote_fugue_device(index)),
        || {
            let mut text = Root::new(|| {
                FugueTextSimple::<MainStorage>::new_with_field_name(FUGUE_SIMPLE_INTERLEAVED_FIELD)
            });
            text.insert(0, 'r').expect("remote insert should succeed");
            text.commit();
            take_last_artifact().expect("commit should emit a delta")
        },
    );
    let actions = match borsh::from_slice::<StorageDelta>(&delta).expect("delta should decode") {
        StorageDelta::Actions(actions) => actions,
        StorageDelta::CausalActions { actions, .. } => actions,
    };
    assert_eq!(
        actions.len(),
        REMOTE_FUGUE_SIMPLE_CHAR_ACTIONS,
        "the remote replica's delta carried {} actions, not the \
         {REMOTE_FUGUE_SIMPLE_CHAR_ACTIONS} that typing ONE character into a \
         FugueTextSimple emits - see REMOTE_FUGUE_CHAR_ACTIONS's doc comment for the two \
         causes and what to do about each.",
        actions.len()
    );
    actions
}

/// Actions in the delta a remote replica emits for exactly one
/// `FugueTextSimple` character. Measured; see [`REMOTE_CHAR_ACTIONS`] for why
/// this is a fixed COUNT rather than an id filter.
const REMOTE_FUGUE_SIMPLE_CHAR_ACTIONS: usize = 6;

fn build_fugue_simple(n: usize) -> Root<FugueTextSimple<MainStorage>> {
    let mut text = Root::new(FugueTextSimple::<MainStorage>::new);
    text.insert_str(0, &"a".repeat(n))
        .expect("insert_str should succeed");
    text
}

/// Build a `FugueText` of `n` characters that is FRAGMENTED, one block per
/// character, by typing into the middle.
///
/// The worst case for a `FugueText` read: a mid-document insert cannot
/// coalesce, so every keystroke mints a fresh block and reads are `O(blocks)`,
/// i.e. `O(n)`. What [`build_fugue_text`] pastes holds one block per
/// `MAX_RUN_LEN` characters instead.
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

/// `n` separate set-then-commit transactions against the SAME `LwwRegister`.
///
/// Unlike the other builds, `n` here is not a collection size — a register
/// always holds exactly one value, so there is nothing to grow. It is the
/// number of times the register is overwritten in a fresh top-level
/// transaction (`Root::fetch` + `set` + `commit`, the same shape a real host
/// call does once per invocation). What must stay flat is the cost of one
/// overwrite regardless of how many times it has already happened — a
/// register's write cost must not grow with its own history.
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

/// Insert `n` inner entries under ONE outer key of a nested map, measuring
/// the whole build.
///
/// # Why one outer key, not `n`
///
/// The first version of this workload used a fresh outer key per entry, so
/// every call to `insert_nested` hit the "outer key absent" branch and
/// minted a brand-new inner `UnorderedMap` with `UnorderedMap::new_internal`
/// — a **random** id (`new_internal`'s own doc: "Use this for nested
/// collections stored as values in other maps"). That random id lands the
/// inner map's own root entry in a different bucket of the outer map's child
/// trie on every run, and `tests/reproducible.rs` caught it directly:
/// `nested_map_insert`'s `rows_removed`/`rows_written` varied by up to 2.4%
/// across 7 runs even though every other build workload in this registry is
/// exact. That is the same random-`Id` trie-shape source the module docs on
/// `lib.rs` already name for BYTE counts — surfacing here on ROW counts too,
/// because inserting a nested COLLECTION (not a plain value) touches the
/// trie structurally, not just its serialized length.
///
/// Keeping the outer key fixed reuses the SAME inner map (SAME id) for all
/// `n` inserts — the inner map is minted once, not `n` times — which removes
/// the recurring random-id source. That alone was not quite enough for exact
/// reproducibility (see `build_nested_map`'s comment for the second fix,
/// making the OUTER map's own id deterministic too); with both fixes this
/// reproduces exactly. It is also the more representative shape: "one
/// document, many fields" is the normal nested-map access pattern, not "one
/// document per field".
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
    // Both the outer map AND the seed step below use `new_with_field_name`
    // (a DETERMINISTIC id) rather than `new()`/`new_internal()` (a random
    // one). Both were needed — see the seed comment for why.
    let mut map = Root::new(|| {
        UnorderedMap::<String, UnorderedMap<String, String, MainStorage>, MainStorage>::new_with_field_name("outer")
    });
    // Seed the outer entry with an EMPTY inner map before any `insert_nested`
    // call, so the one-time nested-collection re-key (below) happens with
    // nothing to relocate, rather than folding it into the cost of the
    // FIRST `inner0` insert.
    //
    // `insert_nested`'s own "outer key absent" branch mints the inner map
    // via `UnorderedMap::new_internal()` (a random id) and then, on
    // write-back, `rekey_nested_value` reassigns it the deterministic id the
    // outer entry expects — relocating every entry the inner map holds AT
    // THAT MOMENT through the child trie under the TARGET id
    // (`reassign_deterministic_id_keyed`'s clear-then-reinsert, see
    // `unordered_map.rs`). That target id is
    // `compute_collection_id(Some(outer_entry_id), "__nested_map", ..)` — it
    // depends on `outer_entry_id`, which depends on the OUTER map's own id.
    //
    // Getting this fully deterministic took two fixes, found in this order:
    //
    // 1. Seed with an EMPTY inner map (this function, first version) so the
    //    one-time relocation moves zero entries instead of the `inner0`
    //    entry. This alone reduced but did NOT eliminate the wobble
    //    (measured: `rows_removed` 11..13 -> 5..6, `rows_written` still
    //    wobbling 287..288) — expected, since (2) below was still random.
    // 2. Seed the inner map itself with `new_with_field_name` (a
    //    deterministic id) instead of plain `new()`. This alone, with the
    //    OUTER map still random, did NOT fully fix it either — the wobble
    //    persisted, because the relocation's TARGET id still depended on
    //    the outer map's random id, not the inner map's pre-rekey id.
    //
    // Only fixing BOTH — outer map AND seed inner map deterministic —
    // removed every random input from the whole chain: 20 separate
    // fresh-process runs of the real `storage-cost` binary (not just an
    // in-process loop) now report byte-identical rows_read/written/removed
    // at every size. Every subsequent `insert_nested` call finds the outer
    // key already present with the inner map's id already correct, so
    // `rekey_nested_value`'s `old_id == new_id` fast path skips the
    // relocation entirely from then on — the one-time seed cost does not
    // scale with `n`.
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

/// Every workload at every size.
///
/// `SortedMap` and `SortedSet` are deliberately absent — reconsidered for this
/// registry expansion and still excluded.
///
/// Their cost depends on `StorageAdaptor::index_supported()`
/// (`crates/storage/src/store.rs`). Without `RuntimeEnv::with_index` installed,
/// native ordered-index ops fall through to the process thread-local mock
/// (`crates/storage/src/env.rs`) — and every `storage_index_*` call in that
/// path (`env.rs`, `index_bridge`) reads/writes/removes a plain `BTreeMap`,
/// never going through this crate's counting `RuntimeEnv` callbacks at all.
/// So a `SortedMap` workload measured here would not merely describe the
/// wrong path, it would silently attribute ZERO cost to the index maintenance
/// entirely (the "extra index write + a marker read/write" the module docs on
/// `SortedMap` promise) while still doing the plain-map point op — publishing
/// a number that looks identical to `UnorderedMap` and claiming to be
/// `SortedMap`'s indexed path. Adding them means wiring all eight
/// `IndexCallbacks` through the counting store first — a separate piece of
/// work, not a workload entry.
pub fn all() -> Vec<Workload> {
    use CostShape::{ConstantPerCall, FlatPerEntry, KnownLinearInN, QuadraticBuild};

    /// A registry row: name, shape, tolerance, body. Sized-independent, so
    /// `all()` crosses it with [`SIZES`].
    type Entry = (&'static str, CostShape, u32, fn(usize));

    const REGISTRY: [Entry; 16] = [
        (
            "unordered_map_insert",
            FlatPerEntry,
            0,
            unordered_map_insert,
        ),
        (
            "unordered_set_insert",
            FlatPerEntry,
            0,
            unordered_set_insert,
        ),
        ("vector_push", FlatPerEntry, 0, vector_push),
        ("unordered_map_len", ConstantPerCall, 0, unordered_map_len),
        ("unordered_map_get", ConstantPerCall, 0, unordered_map_get),
        // Walks the whole trie, so its node count follows the random id
        // distribution. Measured worst-case spread over seven runs, across
        // six separate measurement rounds: 5.0%-10.5% at n=10 (the current
        // committed snapshot's n=10 rows_read is 42 - see
        // `storage-costs.json` — a fresh draw from that same distribution,
        // not a change to the workload), under 3% at every larger size. 18%
        // is `tests/reproducible.rs`'s `declared_tolerances_bound_the_
        // observed_spread` re-derived bound for that range (3x the worst
        // observed spread plus 5 points of sampling headroom), not the
        // 25% cap this used to sit at — see that test for the rule.
        ("vector_get_nth", KnownLinearInN, 18, vector_get_nth),
        // `insert_str` linearises the document once per call, then does `n`
        // flat `UnorderedMap` inserts — see `rga_insert`'s doc comment for why
        // this is genuinely flat and not the same question as the per-char
        // `insert(pos, c)` loop, which is real but unrelated `O(n^2)`.
        ("rga_insert", FlatPerEntry, 0, rga_insert),
        // No positional read exists on `ReplicatedGrowableArray`; every read
        // linearises the whole document — same SHAPE as `vector_get_nth`
        // (KnownLinearInN), one step further along the same wall (see
        // `rga_get_nth`'s doc comment), but NOT the same tolerance.
        // `vector_get_nth`'s 18% comes from real child-trie bucket
        // randomness (measured 5.0%-10.5% worst-case spread at n=10 across
        // six rounds). `get_text()`'s
        // linearisation walks `self.chars.entries()` and sorts in memory —
        // no trie-bucket lookup is involved, so it is not subject to that
        // randomness at all. Measured: exactly `2n` rows_read at every size,
        // zero spread across seven runs. Tolerance is 0.
        ("rga_get_nth", KnownLinearInN, 0, rga_get_nth),
        ("lww_register_set", FlatPerEntry, 0, lww_register_set),
        // Reusing one outer key, and building BOTH the outer map and the
        // seed inner map with deterministic ids (see `nested_map_insert`'s
        // doc comment), eliminates the randomness entirely — every metric
        // reproduces exactly at every size. An earlier version of this
        // workload only fixed the outer-key reuse, leaving the outer map's
        // OWN id random; that alone still let `rows_removed`/`rows_written`
        // wobble by ~1 row (see the doc comment for why: a random parent id
        // moves WHERE the one-time nested-collection re-key lands in the
        // child trie, even when nothing else about the workload is random).
        // Fixing the parent id removed the last variable.
        ("nested_map_insert", FlatPerEntry, 0, nested_map_insert),
        ("nested_map_get", ConstantPerCall, 0, nested_map_get),
        (
            "fugue_text_insert_per_char",
            FlatPerEntry,
            0,
            fugue_text_insert_per_char,
        ),
        ("fugue_text_insert", FlatPerEntry, 0, fugue_text_insert),
        // Every read rebuilds the tree from all blocks, and runs are capped, so
        // a read costs two rows per block: linear in the document.
        (
            "fugue_text_get_text",
            KnownLinearInN,
            0,
            fugue_text_get_text,
        ),
        ("fugue_text_char_at", KnownLinearInN, 0, fugue_text_char_at),
        (
            "fugue_text_text_range",
            KnownLinearInN,
            0,
            fugue_text_text_range,
        ),
    ];

    /// [`CostShape::QuadraticBuild`] workloads, measured at
    /// [`QUADRATIC_SIZES`] instead of [`SIZES`] — see that constant's doc
    /// comment for why they need their own, smaller sizes. A separate array
    /// rather than a row in `REGISTRY` because `REGISTRY` is crossed with
    /// `SIZES` unconditionally below; a `QuadraticBuild` entry there would
    /// silently get measured at `n=10_000` too.
    const QUADRATIC_REGISTRY: [Entry; 8] = [
        (
            "rga_insert_per_char",
            QuadraticBuild,
            0,
            rga_insert_per_char,
        ),
        // Same code path and the same numbers as `rga_insert_per_char` today;
        // see `rga_insert_middle`'s doc comment for why that is the point.
        ("rga_insert_middle", QuadraticBuild, 0, rga_insert_middle),
        // Every remote character is authored on a fresh replica whose entity
        // gets an `Id::random()`, so it lands in a different child-trie bucket
        // run to run and `rows_read` does not reproduce exactly. Measured
        // spread is under 1% at every size; 5 is hand-chosen headroom over
        // that, not a bound `tests/reproducible.rs` derives.
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
            "fugue_text_get_text_fragmented",
            KnownLinearInN,
            0,
            fugue_text_get_text_fragmented,
        ),
        (
            "fugue_text_char_at_fragmented",
            KnownLinearInN,
            0,
            fugue_text_char_at_fragmented,
        ),
        (
            "fugue_text_text_range_fragmented",
            KnownLinearInN,
            0,
            fugue_text_text_range_fragmented,
        ),
        (
            "fugue_text_insert_interleaved_sync",
            QuadraticBuild,
            0,
            fugue_text_insert_interleaved_sync,
        ),
    ];

    /// The `FugueTextSimple` control set, measured at [`QUADRATIC_SIZES`]
    /// whatever each one's shape is: even its point reads pay an `O(n^2)`
    /// build, per the block comment above `fugue_simple_insert`. A separate
    /// array because `QUADRATIC_REGISTRY` is a shape list, not a size list.
    const SIMPLE_REGISTRY: [Entry; 7] = [
        (
            "fugue_simple_insert",
            QuadraticBuild,
            0,
            fugue_simple_insert,
        ),
        (
            "fugue_simple_insert_per_char",
            QuadraticBuild,
            0,
            fugue_simple_insert_per_char,
        ),
        (
            "fugue_simple_insert_middle",
            QuadraticBuild,
            0,
            fugue_simple_insert_middle,
        ),
        (
            "fugue_simple_insert_interleaved_sync",
            QuadraticBuild,
            0,
            fugue_simple_insert_interleaved_sync,
        ),
        (
            "fugue_simple_get_text",
            KnownLinearInN,
            0,
            fugue_simple_get_text,
        ),
        (
            "fugue_simple_char_at",
            KnownLinearInN,
            0,
            fugue_simple_char_at,
        ),
        (
            "fugue_simple_text_range",
            KnownLinearInN,
            0,
            fugue_simple_text_range,
        ),
    ];

    let mut out = Vec::with_capacity(
        REGISTRY.len() * SIZES.len()
            + (QUADRATIC_REGISTRY.len() + SIMPLE_REGISTRY.len()) * QUADRATIC_SIZES.len(),
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
        for (name, shape, tolerance_pct, run) in
            QUADRATIC_REGISTRY.into_iter().chain(SIMPLE_REGISTRY)
        {
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

    /// `#[ignore]`d because measuring every workload costs ~260s in the debug
    /// profile that the workspace-wide `cargo test` would pay on every PR, for
    /// a property that does not change between profiles. The dedicated
    /// `storage-cost` CI job re-includes it in release via `--include-ignored`.
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

    /// What `rga_insert_interleaved_sync` measures must not depend on which
    /// workloads ran before it.
    ///
    /// The pending-delta buffer is thread-local and most workloads never
    /// commit, so a remote commit that drains their queued actions turns this
    /// workload into a re-application benchmark reporting sync cost. It has to
    /// assert on COST: every content check in this crate passes either way.
    #[test]
    fn cost_does_not_depend_on_what_ran_before() {
        let n = 100;
        let (_, clean) = measure(|| rga_insert_interleaved_sync(n));

        // `build_map` never commits, so its actions stay queued: exactly what
        // `unordered_map_insert` leaves behind for every workload after it.
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

    /// The sync half of `rga_insert_interleaved_sync` must actually arrive: if
    /// `apply_action` stopped landing the remote characters the workload would
    /// silently become a single-replica build again, at a cost nothing else in
    /// this crate can tell apart from the real thing.
    #[test]
    fn every_remote_character_actually_lands() {
        let n = 10;
        let (text, _) = measure(|| {
            rga_insert_interleaved_sync(n);
            Root::<ReplicatedGrowableArray<MainStorage>>::fetch()
                .expect("document root should exist after the workload")
                .get_text()
                .expect("get_text should succeed")
        });

        assert_eq!(
            text.chars().count(),
            n,
            "rga_insert_interleaved_sync(n={n}) left a {}-character document ({text:?}): \
             the remote half is not landing through apply_action, so this workload is \
             measuring a single-replica build under a sync name",
            text.chars().count(),
        );
        assert!(
            text.contains('r') && text.contains('a'),
            "document {text:?} is missing one side of the interleave - local chars are \
             'a', remote chars are 'r', and both must be present"
        );
    }

    /// [`every_remote_character_actually_lands`]'s `FugueText` counterpart. It
    /// also covers the failure RGA does not have: node ids are
    /// `(replica, counter)` with the replica derived from the DEVICE id, so two
    /// replicas sharing one device id mint colliding ids instead of
    /// interleaving. Both failures show up as a short document.
    #[test]
    fn every_remote_fugue_character_actually_lands() {
        let n = 10;
        let (text, _) = measure(|| {
            fugue_text_insert_interleaved_sync(n);
            Root::<FugueText<MainStorage>>::fetch()
                .expect("document root should exist after the workload")
                .get_text()
                .expect("get_text should succeed")
        });

        assert_eq!(
            text.chars().count(),
            n,
            "fugue_text_insert_interleaved_sync(n={n}) left a {}-character document \
             ({text:?}): either the remote half is not landing through apply_action, or \
             the two replicas are minting colliding node ids - both make this workload \
             a single-replica build under a sync name",
            text.chars().count(),
        );
        assert!(
            text.contains('r') && text.contains('a'),
            "document {text:?} is missing one side of the interleave - local chars are \
             'a', remote chars are 'r', and both must be present"
        );
    }

    /// The `FugueTextSimple` half of
    /// [`every_remote_fugue_character_actually_lands`]: the control shares the
    /// per-character device-id hazard exactly, so it needs the same guard.
    #[test]
    fn every_remote_fugue_simple_character_actually_lands() {
        let n = 10;
        let (text, _) = measure(|| {
            fugue_simple_insert_interleaved_sync(n);
            Root::<FugueTextSimple<MainStorage>>::fetch()
                .expect("document root should exist after the workload")
                .get_text()
                .expect("get_text should succeed")
        });

        assert_eq!(
            text.chars().count(),
            n,
            "fugue_simple_insert_interleaved_sync(n={n}) left a {}-character document \
             ({text:?}): either the remote half is not landing through apply_action, or \
             the two replicas are minting colliding node ids - both make this workload \
             a single-replica build under a sync name",
            text.chars().count(),
        );
        assert!(
            text.contains('r') && text.contains('a'),
            "document {text:?} is missing one side of the interleave - local chars are \
             'a', remote chars are 'r', and both must be present"
        );
    }

    /// The `FugueTextSimple` half of [`positional_reads_return_real_characters`],
    /// so the control's `KnownLinearInN` reads are known to be reading
    /// something.
    #[test]
    fn simple_positional_reads_return_real_characters() {
        let n = 500;
        let (found, _) = measure(|| {
            let text = build_fugue_simple(n);
            let one = text.char_at(n / 2).expect("char_at should succeed");
            let start = n / 2;
            let range = text
                .text_range(start, start + RANGE_READ_CHARS)
                .expect("text_range should succeed");
            (one, range)
        });

        let (one, range) = found;
        assert_eq!(
            one,
            Some('a'),
            "char_at({}) returned {one:?} against a {n}-character FugueTextSimple \
             document - the control's cost curve is measuring a read that finds nothing",
            n / 2
        );
        assert_eq!(
            range.chars().count(),
            RANGE_READ_CHARS,
            "text_range returned {} characters, not {RANGE_READ_CHARS} - the control's \
             cost curve is measuring a shorter read than it reports",
            range.chars().count()
        );
    }

    /// `fugue_text_char_at` and `fugue_text_text_range` would publish the same
    /// cheap curve if they returned NOTHING, so assert they return the
    /// characters they claim to, at the workloads' own position and size.
    #[test]
    fn positional_reads_return_real_characters() {
        let n = 1_000;
        let (found, _) = measure(|| {
            let text = build_fugue_text(n);
            let one = text.char_at(n / 2).expect("char_at should succeed");
            let start = n / 2;
            let range = text
                .text_range(start, start + RANGE_READ_CHARS)
                .expect("text_range should succeed");
            (one, range)
        });

        let (one, range) = found;
        assert_eq!(
            one,
            Some('a'),
            "char_at({}) returned {one:?} against a {n}-character document - the \
             workload's flat cost curve is measuring a read that finds nothing",
            n / 2
        );
        assert_eq!(
            range.chars().count(),
            RANGE_READ_CHARS,
            "text_range returned {} characters, not {RANGE_READ_CHARS} - the workload's \
             flat cost curve is measuring a shorter read than it reports",
            range.chars().count()
        );
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
