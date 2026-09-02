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
    LwwRegister, NestedMapOps, ReplicatedGrowableArray, Root, UnorderedMap, UnorderedSet, Vector,
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
/// collection to count it was core#3602 finding 2.
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
/// This is the in-repo fixture for the read wall documented in
/// `docs/superpowers/2026-08-26-chat-read-wall.md`: mero-chat's `get_messages`
/// exhausts a 1e9 gas budget at ~32,000 messages, and 30,000 already spends
/// 99.83% of it. The cause is not the app. It is this call:
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

/// Insert `n` characters one at a time at the MIDDLE of the document.
///
/// `rga_insert_per_char` inserts at `i` — i.e. always at the END, because at
/// the top of iteration `i` the document is exactly `i` characters long. It is
/// an append benchmark wearing a typing benchmark's name, and an append is the
/// one position an ordered structure can answer without searching: a seek to
/// the last key. This workload inserts at `i / 2` instead — the middle — which
/// is the position no end-anchored fast path can serve.
///
/// # What it measures TODAY, and why it is here anyway
///
/// Today it measures exactly what `rga_insert_per_char` measures, to the row:
/// `63.5`, `147.7`, `547.1`, `2_047.0` reads/entry at [`QUADRATIC_SIZES`],
/// identical at every size. That is not a redundancy, it is the finding —
/// `insert` re-derives its left-neighbour by linearising the WHOLE document
/// (`get_ordered_chars`, see `rga.rs`) before it looks at `pos` at all, so
/// today the position is free and the linearisation is the whole cost.
///
/// It earns its place as the control on the fix, not on the status quo. A
/// change that makes appends cheap by remembering the tail — the obvious first
/// move, and one that would make `rga_insert_per_char` go flat — leaves this
/// workload untouched, so the pair separates "ordered insertion got cheaper"
/// from "the append case got a special case".
fn rga_insert_middle(n: usize) {
    let mut rga = Root::new(ReplicatedGrowableArray::<MainStorage>::new);
    for i in 0..n {
        rga.insert(i / 2, 'a').expect("insert should succeed");
    }
}

/// Build a document of `n` characters, HALF of them authored locally and half
/// arriving from a remote replica, strictly alternating.
///
/// # The blind spot this exists to remove
///
/// Every other RGA workload in this registry is single-replica: one writer,
/// no sync, nothing ever arrives from outside. A design that pays to
/// re-derive some ordering structure on every REMOTE write is therefore
/// structurally invisible to all of them — they would stay flat while
/// production regressed under exactly the workload a CRDT text collection
/// exists for. This workload is the one that can see it.
///
/// The remote half does NOT go through [`ReplicatedGrowableArray::insert`].
/// Calling `insert` again would be a second LOCAL write wearing a remote
/// label, and would measure the same code path twice. It goes through
/// [`Interface::apply_action`] — the real receive path, the one
/// `crates/node/primitives/src/sync/storage_bridge.rs` drives when a delta
/// lands off the wire. See [`land_remote_char`] for how those actions are
/// authored, and [`remote_char_actions`] for the one piece of thread-local
/// hygiene that path needs.
///
/// # Why it re-fetches the root every iteration
///
/// A `Root` handle caches its children, so a handle held across the loop
/// never observes a character that landed through the apply path — the local
/// writer would keep linearising only its OWN half and the sync cost would
/// be measured against a document half the size it claims. Fetch-write-commit
/// per iteration is also what a node actually does: one host call is one
/// fetch, one mutation, one commit (`lww_register_set` uses the same shape,
/// for the same reason). `every_remote_character_actually_lands` below pins
/// this: it fails if the applied characters stop showing up in the document.
///
/// # Why `n / 2` iterations, not `n`
///
/// `n` means "characters in the collection" everywhere else in this
/// registry, and `reads/entry` divides by it. One iteration here produces
/// TWO characters — one local, one remote — so `n / 2` iterations is what
/// makes `n` mean the same thing it means for `rga_insert_per_char`, and
/// makes the two curves directly comparable rather than merely adjacent.
/// Measured `reads/entry` (`rows_read / n`) at [`QUADRATIC_SIZES`]:
///
/// | `n`   | reads/entry |
/// |-------|-------------|
/// | 10    | 148.3       |
/// | 100   | 413.4       |
/// | 500   | 1,504.0     |
/// | 2,000 | 5,193.6     |
///
/// (the committed `storage-costs.json` row; `rows_read` moves by well under
/// 1% run to run — see the registry entry's `tolerance_pct` note.)
///
/// Against `rga_insert_per_char`'s `63.5 / 147.7 / 547.1 / 2_047.0` at the
/// same sizes: the same shape, ~2.5x the constant, and still tracking `n`.
/// So a remote landing is not a cheap event that the local writer's own
/// linearisation dwarfs — it is itself linear in the document, and it is
/// paid on top of the local cost rather than instead of it. Declared
/// [`CostShape::QuadraticBuild`] for that reason: per-call cost grows with
/// `n`, so the whole build is `O(n^2)`.
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
/// [`rga_insert_interleaved_sync`].
///
/// It has to be a FIELD NAME, not [`ReplicatedGrowableArray::new`]'s random
/// id: the two replicas are separate `RuntimeEnv`s, and the actions authored
/// in one only land in the other's document if both derive the SAME
/// collection id. With random ids the remote actions would quietly build a
/// second, parallel collection and the workload would measure two documents
/// that never meet — green, and meaningless.
const INTERLEAVED_DOC_FIELD: &str = "interleaved_doc";

/// Apply one character to the collection the way the SYNC path does.
///
/// Authors the character on a SEPARATE replica — its own `RuntimeEnv` over
/// its own backing map, so none of the authoring cost is counted — captures
/// the delta that replica emits on commit, and replays its actions into the
/// caller's env through [`Interface::apply_action`].
///
/// The decode-and-replay shape (`StorageDelta` -> actions, skip the root
/// action, `apply_action(action, &ApplyContext::empty())`) is copied from
/// `sorted_set_apply_invalidates_host_index_marker` in
/// `crates/node/primitives/src/sync/storage_bridge.rs`, which drives the same
/// path for the same reason: only the real apply path exercises what apply
/// actually does. The root action is skipped there and here because the
/// receiver's own root entry is not the sender's to overwrite.
///
/// Each call authors on a FRESH replica, so every remote character is a
/// first-position insert with `left = CharId::root()`. The character ids
/// still differ across calls — [`calimero_storage::env::hlc_timestamp`] is a
/// process-thread-local clock shared by both envs, so it advances across the
/// replica boundary and no two remote characters collide on the same map key.
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
/// # The thread-local hazard, and the two things standing against it
///
/// `calimero_storage::delta`'s pending-action buffer is a THREAD-local, not a
/// `RuntimeEnv` one: every storage write on this thread queues into it, and
/// any `commit()` — including the remote replica's, three lines down — drains
/// whatever is in it into the artifact. If actions that are not this
/// character's ride along, they get applied into the caller's env and the
/// workload reports the cost of re-applying already-done writes as a sync
/// cost. An earlier revision of `rga_insert_interleaved_sync` (one held
/// `Root`, no per-iteration commit) did exactly that: `1_469` reads at
/// `n=10` standalone against `59_505` with an `unordered_map_insert(1_000)`
/// ahead of it in the registry.
///
/// As the workload stands, its own commits already drain the buffer before
/// this function is ever reached — the leading `commit()` clears whatever
/// earlier workloads left, and the per-iteration `commit()` clears the local
/// insert. So `clear_pending_delta()` below is NOT currently load-bearing;
/// measured with it removed, every size reproduces inside the declared
/// tolerance. It is kept because it makes the isolation a property of THIS
/// function rather than an accident of where the caller happens to commit,
/// and the caller's commit structure is not fixed — the whole point of this
/// registry is that workloads get rewritten.
///
/// The assertion is the part that actually detects a leak, and it detects one
/// however it arrives: see [`REMOTE_CHAR_ACTIONS`].
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
         typing ONE character emits — either the thread-local pending-delta buffer leaked into \
         it (check that `clear_pending_delta()` above still runs, so this workload is not about \
         to re-apply writes that were already done and report the cost as sync), or a \
         legitimate storage-layer change altered what one character insert emits, in which case \
         move REMOTE_CHAR_ACTIONS deliberately per its doc comment and regenerate the snapshot.",
        actions.len()
    );
    actions
}

/// Actions in the delta a remote replica emits for exactly one character.
///
/// Measured, and stable across runs: the root entry (twice — opened and
/// closed), the char map's collection entry, its index entry (twice), and the
/// character itself. The exact composition matters less than the COUNT being
/// fixed: one character is one bounded, unchanging set of actions, so any
/// other number means something that is not this character is riding along.
///
/// # Why a count, and not "every action targets the RGA's collection id"
///
/// The id check was considered and rejected: it cannot see the dominant
/// pollution. The actions most likely to leak into this delta are the
/// CALLER'S OWN local `insert`s into the very same document — same collection
/// id, same entity shape — so an id filter would wave them straight through
/// while the measurement silently became a re-application benchmark. A fixed
/// count catches those, catches earlier workloads' uncommitted actions, and
/// catches anything else that ever starts sharing the buffer, without knowing
/// what any of it looks like.
///
/// If a legitimate change to the storage layer alters what one character
/// emits, this constant is what has to move — deliberately, with the new
/// number read off a clean run, and with the snapshot regenerated because the
/// applied cost changed too.
const REMOTE_CHAR_ACTIONS: usize = 6;

/// A throwaway `RuntimeEnv` over its own map, deliberately NOT wired to
/// [`crate::measure`]'s counters.
///
/// The remote replica's own writes are not part of what this crate measures —
/// what is being measured is what the RECEIVER pays. Counting the sender's
/// work too would attribute an unrelated single-replica build to the sync
/// path and hide the number the workload exists to publish.
fn uncounted_env() -> RuntimeEnv {
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
    RuntimeEnv::new(read, write, remove, [1; 32], [2; 32], [3; 32])
}

fn build_rga(n: usize) -> Root<ReplicatedGrowableArray<MainStorage>> {
    let mut rga = Root::new(ReplicatedGrowableArray::<MainStorage>::new);
    let text: String = std::iter::repeat_n('a', n).collect();
    rga.insert_str(0, &text).expect("insert_str should succeed");
    rga
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
/// registry expansion (task 9 asked for `sorted_map_insert`/`sorted_map_get`
/// by name) and still excluded, for the same reason as before.
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
/// work, not a workload entry. See the task 9 report for the full note.
pub fn all() -> Vec<Workload> {
    use CostShape::{ConstantPerCall, FlatPerEntry, KnownLinearInN, QuadraticBuild};

    /// A registry row: name, shape, tolerance, body. Sized-independent, so
    /// `all()` crosses it with [`SIZES`].
    type Entry = (&'static str, CostShape, u32, fn(usize));

    const REGISTRY: [Entry; 11] = [
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
        // committed snapshot's n=10 rows_read is 42 — see
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
    ];

    /// [`CostShape::QuadraticBuild`] workloads, measured at
    /// [`QUADRATIC_SIZES`] instead of [`SIZES`] — see that constant's doc
    /// comment for why they need their own, smaller sizes. A separate array
    /// rather than a row in `REGISTRY` because `REGISTRY` is crossed with
    /// `SIZES` unconditionally below; a `QuadraticBuild` entry there would
    /// silently get measured at `n=10_000` too.
    const QUADRATIC_REGISTRY: [Entry; 3] = [
        (
            "rga_insert_per_char",
            QuadraticBuild,
            0,
            rga_insert_per_char,
        ),
        // Same code path as `rga_insert_per_char` today, and the same numbers
        // to the row — see `rga_insert_middle`'s doc comment for why that is
        // the point rather than a duplication. Deterministic for the same
        // reason its sibling is: nothing here depends on a random id.
        ("rga_insert_middle", QuadraticBuild, 0, rga_insert_middle),
        // The only workload in this registry that receives. `rows_written`
        // and `rows_removed` reproduce exactly; `rows_read` does not, because
        // every remote character is authored on a fresh replica whose entity
        // gets an `Id::random()` and therefore lands in a different child-trie
        // bucket run to run — the same source `lib.rs`'s module docs name for
        // byte counts, surfacing on rows here because the applied entity is
        // linked into the trie. Measured spread over seven runs is well under
        // 1% at every size (the counts are large, so a few bucket-shaped rows
        // barely move them). `tests/reproducible.rs` re-derives the spread
        // over seven runs and fails if 5 is too TIGHT — i.e. if the real
        // spread ever exceeds it. It does not police 5 in the other
        // direction: its too-loose rule is `declared > worst_spread * 3 + 8`,
        // and the flat `+ 8` means no declaration of 5 can ever trip it. So
        // the too-loose half of that test is not evidence for this number;
        // the evidence is the measured sub-1% spread above, and 5 is the
        // headroom chosen over it by hand.
        (
            "rga_insert_interleaved_sync",
            QuadraticBuild,
            5,
            rga_insert_interleaved_sync,
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

    /// `#[ignore]`d by default, and NOT because it is optional.
    ///
    /// It iterates `all()` and measures every workload, including the
    /// `CostShape::QuadraticBuild` set at `n=2_000` — the same set that makes
    /// `tests/reproducible.rs`'s `declared_tolerances_bound_the_observed_spread`
    /// slow. In the debug profile this alone measures ~260s.
    /// `.github/workflows/ci-checks.yml`'s workspace-wide `cargo test` is a
    /// DEBUG build and would pay that on every PR, on the critical path, for a
    /// property that does not change between profiles.
    ///
    /// So it is excluded from the default run and re-included explicitly by
    /// the dedicated `storage-cost` job, which already builds this crate in
    /// release: `cargo test -p storage-cost --release -- --include-ignored`.
    /// Removing the `--include-ignored` there deletes this coverage silently,
    /// which is why it is named in that step's own comment too.
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
    /// The pending-delta buffer is thread-local and shared by every workload
    /// in the registry, and most of them never commit — their actions sit in
    /// it. Any `commit()` on that thread drains whatever is queued into the
    /// artifact, so if the remote replica's commit ever picks those up, this
    /// workload starts re-applying other people's writes and reporting the
    /// cost as sync. That is not theoretical: an earlier revision of this
    /// workload (one held `Root`, no per-iteration commit) measured `1_469`
    /// reads at `n=10` standalone against `59_505` with an
    /// `unordered_map_insert(1_000)` ahead of it — a 40x swing decided by
    /// registry order.
    ///
    /// Nothing else in this crate can see that.
    /// `every_remote_character_actually_lands` passes either way (the
    /// polluted delta still carries the character),
    /// `every_workload_is_measurable_and_touches_storage` passes either way
    /// (the cost is nonzero either way),
    /// `declared_tolerances_bound_the_observed_spread` compares the workload
    /// only against itself, and an inflation of this size still clears
    /// `flat_curve.rs`'s quadratic ceiling. So this asserts the property
    /// directly, on COST rather than on content: same workload, once clean
    /// and once behind an uncommitted build, must cost the same.
    #[test]
    fn cost_does_not_depend_on_what_ran_before() {
        let n = 100;
        let (_, clean) = measure(|| rga_insert_interleaved_sync(n));

        // `build_map` never commits, so its actions stay queued — exactly what
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

    /// The sync half of `rga_insert_interleaved_sync` must actually arrive.
    ///
    /// Everything about that workload's value rests on the applied characters
    /// being IN the document: if `apply_action` stopped landing them (a
    /// diverging collection id, a dropped action, a root-skip that skips too
    /// much), the workload would still read plausibly and still be measured —
    /// it would simply be a single-replica build again, i.e. the exact blind
    /// spot it was added to remove, restored silently. `every_workload_is_
    /// measurable_and_touches_storage` above cannot see that: the cost is
    /// nonzero either way.
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
            "document {text:?} is missing one side of the interleave — local chars are \
             'a', remote chars are 'r', and both must be present"
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
