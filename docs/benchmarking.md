# Benchmarking core

Three tiers. Only one of them can fail your PR.

## Tier 1 — cost gates (BLOCKING)

Deterministic counts of operations, diffed against a committed snapshot.
Machine-independent, so a delta is a real change and blocks the merge — an
improvement blocks too, because the snapshot is the reviewed record of what an
operation costs.

- `tools/storage-cost` + `scripts/check-storage-cost.sh` — storage rows per
  collection operation. Snapshot: `tools/storage-cost/storage-costs.json`.
- `tools/sync-cost` + `scripts/check-sync-cost.sh` — sync round-trips, entities
  and bytes per scenario. Snapshot: `tools/sync-cost/sync-costs.json`.

Accepting a change:

    cargo run -p storage-cost --bin storage-cost --release > tools/storage-cost/storage-costs.json

and commit it, so the delta shows up in the PR diff.

## Tier 2 — criterion benches (REPORTING ONLY)

Wall-clock. Never gates: an O(n) read pattern against an in-memory store is
almost free in wall-clock while real gas explodes, and shared CI runners exceed
criterion's ~5% significance threshold from cache state alone.

    cargo bench -p calimero-storage --bench child_trie
    cargo bench -p calimero-storage --bench child_trie -- --quick     # ~10s, noisy, for iterating
    cargo bench -p calimero-storage --bench child_trie -- --test      # run once, assert nothing: what CI's rot gate does

Criterion compiles benches with release optimisations. Never read numbers from
a debug build — SHA256 and allocation paths are ~20x slower and the curve shape
will lie to you.

`master` saves a baseline per commit; a PR labelled `run-benchmarks` compares
against it with `critcmp`.

## Tier 3 — macro

`.github/workflows/fuzzy-load-test.yml` (CPU/memory flamegraphs, nightly soak)
and `crates/runtime/tests/chat_wall.rs` (the gas wall against the mero-chat
sibling, `#[ignore]`d by design).

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
