# Benchmarking core

Three tiers, but timing benches never gate a PR: only deterministic counts
(Tier 1) and compilability/lint of bench code can fail your PR. Concretely,
three things can fail a PR on a bench: the Tier 1 cost gate, the
`bench-compile` job (`cargo bench --workspace --benches --no-run`), and the
`rust` job's `cargo build --workspace --all-targets --tests` /
`cargo clippy --workspace --all-targets --features calimero-storage/testing
-- -D warnings`, both of which compile and lint every bench file too. A
*timing* result, from any of these benches, never can.

## Tier 1 — cost gates (BLOCKING)

Deterministic counts of operations, diffed against a committed snapshot.
Machine-independent, so a delta is a real change and blocks the merge — an
improvement blocks too, because the snapshot is the reviewed record of what an
operation costs.

- `tools/storage-cost` + `scripts/check-storage-cost.sh` — storage rows per
  collection operation. Snapshot: `tools/storage-cost/storage-costs.json`.

Accepting a change:

    cargo run -p storage-cost --bin storage-cost --release > tools/storage-cost/storage-costs.json

and commit it, so the delta shows up in the PR diff.

## Tier 2 — criterion benches (REPORTING ONLY)

Wall-clock. Never gates: an O(n) read pattern against an in-memory store is
almost free in wall-clock while real gas explodes, and shared CI runners exceed
criterion's ~5% significance threshold from cache state alone.

    cargo bench -p calimero-storage --bench child_trie
    cargo bench -p calimero-storage --bench child_trie -- --quick     # ~10s, noisy, for iterating
    cargo bench -p calimero-storage --bench child_trie -- --test      # run once, assert nothing: a local smoke test, NOT what CI's rot gate does (that's `cargo bench --workspace --benches --no-run`, which compiles without executing — see `bench-compile` below)

Criterion compiles benches with release optimisations. Never read numbers from
a debug build — SHA256 and allocation paths are ~20x slower and the curve shape
will lie to you. One exception: `[profile.bench.package.calimero-storage]`
turns `debug-assertions` back on for that one package only, to clear a
release-build guard in `calimero-storage` that a workspace-wide bench compile
otherwise trips (see `crates/storage/src/interface.rs`) — opt-level is
untouched, so this does not put you in a debug build, it only adds
`calimero-storage`'s runtime assertion checks to the measured path.

`master` saves a baseline per commit (`.github/workflows/benchmarks.yml`,
`criterion` job); a PR labelled `run-benchmarks` compares against it with
`critcmp` (same file, `compare` job). Neither job can fail your PR — see
[Reading the comparison](#reading-the-comparison) below for what the
comparison actually needs in order to produce a table rather than a message.

## Tier 3 — macro

`.github/workflows/fuzzy-load-test.yml` (CPU/memory flamegraphs, nightly soak)
and `crates/runtime/tests/chat_wall.rs` (the gas wall against the mero-chat
sibling, `#[ignore]`d by design).

`crates/node/tests/sync_sim/benchmarks.rs`'s `benchmark_all_scenarios` and
`benchmark_scaling` are not sync-cost coverage today. They print round-trip,
entity, merge and byte counters that read as measurements, but the sim
harness never drives the sync protocol — `add_existing_node` schedules
nothing, and the event-dispatch stubs in
`crates/node/tests/sync_sim/sim_runtime.rs` are unfinished — so those
counters are 0 for 12 of the 13 scenarios on every run. `benchmark_scaling`
asserts nothing; `benchmark_all_scenarios` asserts only
`summary.converged > 0`, which the one trivially-converging scenario
satisfies regardless of what the other twelve report. Treat both as
placeholders, not as a cost gate on sync.

## Adding a bench

1. `benches/<question>.rs` in the crate that owns the code. One file per
   question, named for the question, not the crate.
2. `harness = false` on the `[[bench]]`, and `bench = false` on the crate's
   `[lib]`, `[[bin]]` and `[[test]]` targets — otherwise libtest gets handed
   criterion's flags and rejects them.
3. Module docs must say what question the bench answers and what answer would
   change a decision. A bench with no question gets deleted at the next cleanup.
4. Never reach into a private function by copying its body. If it is worth
   benchmarking it is worth a `pub(crate)` seam — the copy silently stops
   tracking the original, which is how PR #2203's merkle bench died.
5. `cargo bench --workspace --benches --no-run` before pushing.
6. If this is the crate's *first* `[[bench]]`, add the crate name to the
   `matrix.crate` list in `.github/workflows/benchmarks.yml` (`criterion`
   job). `bench-compile` (step 5) verifies every bench target still
   compiles, but it does not run anything or add a crate to the trend job —
   a crate missing from that hand-maintained matrix silently gets no
   `master` baseline and never appears in a PR comparison, even though its
   bench compiles cleanly and looks covered.

## Reading the comparison

The `run-benchmarks` label posts a `critcmp` table against the PR's base commit
-- but only when a comparison is actually possible. Getting there needs, in
order: a benchmarks.yml run to have completed successfully on the base commit
(so `push` to `master` must have already run this workflow at that commit),
and that run's artifacts to still be within the 30-day retention window. The
`compare` job looks these up itself (`gh run list --workflow benchmarks.yml
--commit <base-sha>`, then a cross-run `actions/download-artifact` using that
run's id) and posts one of three plain-English "no comparison" messages
instead of a table when any of that is missing, rather than a table that
silently compares nothing. If the PR comment says "no comparison", read the
job's own log for which of the three cases it hit before assuming the
benchmarks are unchanged.

When a table does show up:

- Under ~5%: noise. Shared runners vary by more than that between identical runs.
- 5-20% on one benchmark, nothing else: usually noise too. Re-run before believing it.
- A whole group moving one way, or any change in the SHAPE of a sweep (the ratio
  between n=100 and n=10000 changing): real, and worth explaining in the PR.
- A cost gate failing: not noise, ever. That is a counted operation, and the
  snapshot moved.
