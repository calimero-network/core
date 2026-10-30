# Benchmarking core

Three tiers. Only Tier 1 can fail a PR; a timing result never does.

## Tier 1 - cost gates (BLOCKING)

Deterministic counts of storage operations, diffed against a committed snapshot, so any delta is a real change and blocks the merge.
An improvement blocks too: the snapshot is the reviewed record of what an operation costs.

- `tools/storage-cost` + `scripts/check-storage-cost.sh`, snapshotted in `tools/storage-cost/storage-costs.json`.
- State rows (`rows_*`) and node-local ordered-index rows (`index_rows_*`, used by `SortedMap` and `IndexedMap`) are counted separately, because they live in different columns and only state is synced or hashed.
  An index metric is left out of the JSON when it is zero, and the gate reads a missing one as zero.
- SHA-256 work (`hash_calls`, `hash_blocks`): every hash `calimero-storage` finishes, and the compression blocks it runs (`ceil((len + 9) / 64)`), counted through `crates/storage/src/hash_meter.rs` under the `cost-meter` feature that only `tools/storage-cost` enables.
  This is the CPU proxy. Re-hashing a row's data on every decode costs no extra row: #4266 did that, and its read workloads (`unordered_map_get`, `rga_get_nth`, `nested_map_get`) kept every row count while their `hash_blocks` rose 100-200%. It is a count, so it reproduces on any machine.
  Storage code hashes through `crate::hash_meter::Sha256`: a module naming `sha2` directly is uncounted, and a unit test refuses it.
- Bytes that row counts cannot see are gated by tests in `tools/storage-cost/tests/`: `keystroke_bytes.rs` (what one keystroke rewrites) and `delta_bytes.rs` (the ancestor bytes one write ships in its delta, which every node keeps in its DAG history; `-- --nocapture` prints each action's breakdown).

To accept a change, regenerate the snapshot and commit it so the delta shows up in the PR diff:

    cargo run -p storage-cost --bin storage-cost --release > tools/storage-cost/storage-costs.json

## Tier 1b - scaling guards (BLOCKING)

Ordinary `#[test]`s that time one operation at a small and a large input and assert the *ratio*, never an absolute time.
The ratio is what a complexity bug changes and what a slower machine does not: growing the input 8x grows a linear path ~8x and a quadratic one ~64x, so a bound between the two (24x, keeping the fastest of three runs) fails a quadratic regression on any runner and passes a linear path even in a debug build under `nextest`.

- `tools/storage-cost/tests/flat_curve.rs` (rows and hashes counted, not timed): the shape of each storage workload's cost curve, in both directions (a cost marked linear that stops being linear fails too); `cargo test -p storage-cost --release --test flat_curve -- --include-ignored`.
- `crates/runtime/tests/cost_is_flat.rs` (gas, not time): whether a guest doing a fixed number of host calls is charged more gas as the store grows. It exists because an append whose cost grew with the collection made a collection unwritable while every correctness test stayed green.

(v1's pending-set guard, `crates/dag/tests/complexity.rs`, went with the DAG crate.)

Write one when a path's cost depends on state a caller can grow (a collection, a queue, a ring of records) and the right answer is "linear" or "flat".
Build the fixture outside the timed region, size it so the large run is a few tens of milliseconds, and also assert the result (counts, order), so the test is a correctness check as well as a timing one.
Pair it with a Criterion sweep over the same shape in Tier 2, which shows the trend the guard only bounds.

## Tier 2 - criterion benches (REPORTING ONLY)

    cargo bench -p calimero-storage --bench child_trie
    cargo bench -p calimero-storage --bench child_trie -- --quick   # ~10s, noisy, for iterating
    cargo bench -p calimero-storage --bench child_trie -- --test    # run once, assert nothing
    cargo bench -p storage-cost --bench collections                 # every registry workload, timed
    cargo bench -p storage-cost --bench indexed_map_indexes         # write cost per declared index
    cargo bench -p calimero-log --bench log                         # append + commit, 1 vs 3 nodes; one synced RocksDB write
    cargo bench -p calimero-sealed --bench sealed                   # a request sealed vs plain, 1 KiB / 64 KiB / 1 MiB

Criterion compiles benches with release optimisations; never read numbers from a debug build.

Two benches are not criterion, and run as their own jobs of the same workflow:

    cargo run --release -p search-bench -- --sizes 10000,100000 --out search-bench.md     # tools/search-bench
    cargo bench -p calimero-node --bench cluster_throughput -- --secs 20 --out cluster-throughput.json

- `tools/search-bench`: `calimero-search`'s engine (index size, query latency, memory, the search-gas fit) as a markdown report, uploaded as `search-bench-<sha>`. Its `README.md` holds the reference run.
- `crates/node/benches/cluster_throughput.rs`: calls per second and p50/p99 through a 3-node cluster in one process (the real node, sealed channels, the kv-store wasm, merod's Raft timing): 1 app with 1 and 64 calls in flight, and 2 apps with 64 each, led by one node and by two. It needs `kv_store.wasm` built first (`cargo build -p kv-store --target wasm32-unknown-unknown --profile app-release`) and writes JSON, uploaded as `cluster-throughput-<sha>`; the `compare` job adds a table of it against the base commit's to the PR comment. The [testing page](src/content/docs/contribute/testing.mdx#benchmarks-and-cost-gates) has a reference run.

`master` and the release branch `release-v2-parity` each save a baseline per commit, and a PR compares against its base branch's with `critcmp` (`.github/workflows/benchmarks.yml`): a PR benches only while it carries the `run-benchmarks` label, and then benches every crate, again on each push. A storage change without the label is covered by the deterministic storage cost gate in `ci-checks.yml` instead.
Neither ever fails the PR. On a fork PR the comparison runs but cannot comment (its token is read-only), so read the `compare` job's log.

## Tier 3 - macro

`.github/workflows/e2e.yml` runs the process suite's kv-store soak (`kv-soak` in `crates/node/tests/fault/suite.py`, kv-store's fuzzy load on three `merod run --dev` processes): 30 s on a PR, 900 s nightly, against this tree's `merod` (a release build, job `process-suite`) and against the `merod` in the latest release's image (job `release-process-suite`); `release.yml` runs it for 30 s on a PR's own image. The fuzzy scenarios (`workflows/fuzzy-tests/`) also run 900 s nightly in the in-process scenario shards.
The soak records per-call latency (p50/p95/p99) and calls/s per operation (`soak.py`'s report), shown in the job summary and uploaded with the job's artifact (`process-suite`, `release-process-suite`, `image-e2e-results`); like Tier 2, it gates nothing, and only the pass rate can fail.
The calls run one at a time, so the numbers are one caller's latency and rate, not the cluster's capacity (the cluster bench measures that); compare a night with earlier nights of the same job, and read the release image's rows for the shipped build's speed.
The same nightly run profiles one node through the kv-store soak with `perf record -g` and renders a flamegraph with inferno (artifact `merod-flamegraph`, with the folded stacks for `inferno-diff-folded`).
The [testing page](src/content/docs/contribute/testing.mdx#soak-latency-and-throughput) has the commands and a sample.
The document-ceiling probes in `crates/runtime/tests/{chat,rga,fugue}_wall.rs` are `#[ignore]`d by design; they answer "how big can one document get", which Tier 1's row counts cannot.

## Adding a bench

1. `benches/<question>.rs` in the crate that owns the code, one file per question, named for the question.
2. `harness = false` on the `[[bench]]`, and `bench = false` on the crate's `[lib]` and `[[bin]]` targets.
3. Module docs say what question the bench answers.
4. Never copy a private function's body; add a `pub(crate)` seam instead.
5. Run `cargo bench --workspace --benches --no-run` before pushing.
6. For a crate's *first* `[[bench]]`, add the crate to the `all` list in the `plan` job of `.github/workflows/benchmarks.yml`, or it silently gets no baseline on `master` or the release branch.
   A bench that is not criterion (no baseline for `critcmp`) gets its own job there instead, uploading its report as an artifact named with the commit, as `search-bench` and `cluster` do.

## Reading the comparison

The `run-benchmarks` label posts a `critcmp` table against the PR's base commit, but only when benchmarks.yml ran successfully there and its artifacts are still inside the 30-day retention window.
Otherwise the `compare` job posts a "no comparison" message naming which case it hit; read its log before assuming the benchmarks are unchanged.

- Under ~5%: noise. Shared runners vary by more than that between identical runs.
- 5-20% on one benchmark, nothing else: usually noise too. Re-run before believing it.
- A whole group moving one way, or a change in the SHAPE of a sweep: real, and worth explaining in the PR.
- A cost gate failing: not noise, ever. That is a counted operation, and the snapshot moved.
  A `hash_blocks` delta with no row delta is the CPU change criterion shows only noisily: read which workloads moved before regenerating.
