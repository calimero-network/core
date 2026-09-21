# Benchmarking core

Three tiers. Only Tier 1 can fail a PR; a timing result never does.

## Tier 1 - cost gates (BLOCKING)

Deterministic counts of storage operations, diffed against a committed snapshot, so any delta is a real change and blocks the merge.
An improvement blocks too: the snapshot is the reviewed record of what an operation costs.

- `tools/storage-cost` + `scripts/check-storage-cost.sh`, snapshotted in `tools/storage-cost/storage-costs.json`.

To accept a change, regenerate the snapshot and commit it so the delta shows up in the PR diff:

    cargo run -p storage-cost --bin storage-cost --release > tools/storage-cost/storage-costs.json

## Tier 2 - criterion benches (REPORTING ONLY)

    cargo bench -p calimero-storage --bench child_trie
    cargo bench -p calimero-storage --bench child_trie -- --quick   # ~10s, noisy, for iterating
    cargo bench -p calimero-storage --bench child_trie -- --test    # run once, assert nothing

Criterion compiles benches with release optimisations; never read numbers from a debug build.

`master` saves a baseline per commit, and a PR labelled `run-benchmarks` compares against it with `critcmp` (`.github/workflows/benchmarks.yml`).

## Tier 3 - macro

`.github/workflows/fuzzy-load-test.yml` runs CPU and memory flamegraphs as a nightly soak.
The document-ceiling probes in `crates/runtime/tests/{chat,rga,fugue}_wall.rs` are `#[ignore]`d by design; they answer "how big can one document get", which Tier 1's row counts cannot.

## Adding a bench

1. `benches/<question>.rs` in the crate that owns the code, one file per question, named for the question.
2. `harness = false` on the `[[bench]]`, and `bench = false` on the crate's `[lib]` and `[[bin]]` targets.
3. Module docs say what question the bench answers.
4. Never copy a private function's body; add a `pub(crate)` seam instead.
5. Run `cargo bench --workspace --benches --no-run` before pushing.
6. For a crate's *first* `[[bench]]`, add the crate to `matrix.crate` in `.github/workflows/benchmarks.yml`, or it silently gets no `master` baseline.

## Reading the comparison

The `run-benchmarks` label posts a `critcmp` table against the PR's base commit, but only when benchmarks.yml ran successfully there and its artifacts are still inside the 30-day retention window.
Otherwise the `compare` job posts a "no comparison" message naming which case it hit; read its log before assuming the benchmarks are unchanged.

- Under ~5%: noise. Shared runners vary by more than that between identical runs.
- 5-20% on one benchmark, nothing else: usually noise too. Re-run before believing it.
- A whole group moving one way, or a change in the SHAPE of a sweep: real, and worth explaining in the PR.
- A cost gate failing: not noise, ever. That is a counted operation, and the snapshot moved.
