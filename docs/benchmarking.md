# Benchmarking core

Three tiers, and a timing result never gates a PR.
Only deterministic counts and the compilability of bench code can fail one: the Tier 1 cost gate, and the `rust` job's build and clippy passes, which compile and lint every bench file too.

## Tier 1 - cost gates (BLOCKING)

Deterministic counts of operations, diffed against a committed snapshot.
They are machine-independent, so any delta is a real change and blocks the merge.
An improvement blocks too, because the snapshot is the reviewed record of what an operation costs.

- `tools/storage-cost` + `scripts/check-storage-cost.sh`: storage rows per collection operation, snapshotted in `tools/storage-cost/storage-costs.json`.

To accept a change, regenerate the snapshot and commit it so the delta shows up in the PR diff:

    cargo run -p storage-cost --bin storage-cost --release > tools/storage-cost/storage-costs.json

## Tier 2 - criterion benches (REPORTING ONLY)

Wall-clock timings, which never gate.
An O(n) read pattern against an in-memory store is almost free in wall-clock while real gas explodes, and shared CI runners exceed criterion's ~5% significance threshold from cache state alone.

    cargo bench -p calimero-storage --bench child_trie
    cargo bench -p calimero-storage --bench child_trie -- --quick   # ~10s, noisy, for iterating
    cargo bench -p calimero-storage --bench child_trie -- --test    # run once, assert nothing

Criterion compiles benches with release optimisations.
Never read numbers from a debug build: SHA256 and allocation paths are ~20x slower, and the curve shape will lie to you.
The one exception is `[profile.bench.package.calimero-storage]`, which turns `debug-assertions` back on for that package alone to clear a release-build guard (see `crates/storage/src/interface.rs`).
Opt-level is untouched there, so it only adds that crate's runtime assertions to the measured path.

`master` saves a baseline per commit, and a PR labelled `run-benchmarks` compares against it with `critcmp` (`.github/workflows/benchmarks.yml`, jobs `criterion` and `compare`).
Neither job can fail a PR; see [Reading the comparison](#reading-the-comparison) for what a comparison needs in order to produce a table rather than a message.

## Tier 3 - macro

`.github/workflows/fuzzy-load-test.yml` runs CPU and memory flamegraphs as a nightly soak.
The document-ceiling probes are `#[ignore]`d by design: `crates/runtime/tests/chat_wall.rs` (the gas wall against the mero-chat sibling), `crates/runtime/tests/rga_wall.rs` (where a `ReplicatedGrowableArray` document stops being writable and readable, driven through `apps/collaborative-editor`), and `crates/runtime/tests/fugue_wall.rs` (the same two questions for `FugueText` plus its positional reads, driven through `apps/fugue-editor`).
They answer "how big can one document get", which Tier 1's row counts cannot: rows are blind to the byte cost of a run-length block, and gas charges the decode.

`crates/node/tests/sync_sim/benchmarks.rs` prints round-trip, entity, merge and byte counters that read as measurements, but the sim harness never drives the sync protocol, so those counters are 0 for 12 of its 13 scenarios.
Treat `benchmark_all_scenarios` and `benchmark_scaling` as placeholders, not as a cost gate on sync.

## Adding a bench

1. `benches/<question>.rs` in the crate that owns the code, one file per question, named for the question rather than the crate.
2. `harness = false` on the `[[bench]]`, and `bench = false` on the crate's `[lib]` and `[[bin]]` targets, or libtest gets handed criterion's flags and rejects them (a `[[test]]` target already defaults to `bench = false`).
3. Module docs must say what question the bench answers and what answer would change a decision.
   A bench with no question gets deleted at the next cleanup.
4. Never reach into a private function by copying its body.
   If it is worth benchmarking it is worth a `pub(crate)` seam, because a copied body stops tracking the original silently.
5. Run `cargo bench --workspace --benches --no-run` before pushing.
6. For a crate's *first* `[[bench]]`, add the crate to the `matrix.crate` list in `.github/workflows/benchmarks.yml` (`criterion` job).
   Compiling a bench target proves nothing about the matrix, so a crate missing from that hand-maintained list silently gets no `master` baseline and never appears in a comparison.

## Reading the comparison

The `run-benchmarks` label posts a `critcmp` table against the PR's base commit, but only when a comparison is actually possible.
That needs a benchmarks.yml run to have completed successfully on the base commit, which means `push` to `master` must already have run this workflow there, and that run's artifacts to still be inside the 30-day retention window.
The `compare` job looks both up itself and posts one of three plain-English "no comparison" messages when either is missing, rather than a table that silently compares nothing.
If the PR comment says "no comparison", read the job's own log for which of the three cases it hit before assuming the benchmarks are unchanged.

When a table does show up:

- Under ~5%: noise. Shared runners vary by more than that between identical runs.
- 5-20% on one benchmark, nothing else: usually noise too. Re-run before believing it.
- A whole group moving one way, or any change in the SHAPE of a sweep (the ratio between n=100 and n=10000 changing): real, and worth explaining in the PR.
- A cost gate failing: not noise, ever. That is a counted operation, and the snapshot moved.
