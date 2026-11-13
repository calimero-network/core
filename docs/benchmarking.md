# Benchmarking core

Two tiers, neither of which fails a PR: a timing result never does. (The storage
cost gate and the wasmer runtime's scaling guards went with SDK 0.12's storage
and runtime crates.)

Write a scaling guard (an ordinary `#[test]` that times one operation at a small
and a large input and asserts the *ratio*, never an absolute time) when a path's
cost depends on state a caller can grow (a collection, a queue, a ring of
records) and the right answer is "linear" or "flat". Build the fixture outside
the timed region, size it so the large run is a few tens of milliseconds, and also
assert the result (counts, order). Pair it with a Criterion sweep over the same
shape in Tier 2.

## Tier 2 - criterion benches (REPORTING ONLY)

    cargo bench -p calimero-log --bench log                         # append + commit, 1 vs 3 nodes; one synced RocksDB write
    cargo bench -p calimero-sealed --bench sealed                   # a request sealed vs plain, 1 KiB / 64 KiB / 1 MiB

Criterion compiles benches with release optimisations; never read numbers from a debug build.

Two benches are not criterion, and run as their own jobs of the same workflow:

    cargo run --release -p search-bench -- --sizes 10000,100000 --out search-bench.md     # tools/search-bench
    cargo bench -p calimero-node --bench cluster_throughput -- --secs 20 --out cluster-throughput.json

- `tools/search-bench`: `calimero-search`'s engine (index size, query latency, memory, a query's host time against the work it reports) as a markdown report, uploaded as `search-bench-<sha>`. Its `README.md` holds the reference run.
- `crates/node/benches/cluster_throughput.rs`: calls per second and p50/p99 through a 3-node cluster in one process (the real node, sealed channels, the kv component, merod's Raft timing): 1 app with 1 and 64 calls in flight, and 2 apps with 64 each, led by one node and by two. It builds `apps/v2-fixtures/kv` on first use and writes JSON, uploaded as `cluster-throughput-<sha>`; the `compare` job adds a table of it against the base commit's to the PR comment. The [testing page](src/content/docs/contribute/testing.mdx#benchmarks-and-cost-gates) has a reference run.

`master` saves a baseline nightly, and a PR compares against its base commit's (or else its base branch's newest) with `critcmp` (`.github/workflows/benchmarks.yml`): a PR benches only while it carries the `run-benchmarks` label, and then benches every crate, again on each push.
Neither ever fails the PR. On a fork PR the comparison runs but cannot comment (its token is read-only), so read the `compare` job's log.

## Tier 3 - macro

`.github/workflows/e2e.yml` runs the process suite's kv-store soak (`kv-soak` in `crates/node/tests/fault/suite.py`, kv-store's fuzzy load on three `merod run --dev` processes): 30 s on a PR, 900 s nightly, against this tree's `merod` (a release build, job `process-suite`) and against the `merod` in the latest release's image (job `release-process-suite`); `release.yml` runs it for 30 s on a PR's own image. The fuzzy scenarios (`workflows/fuzzy-tests/`) also run 900 s nightly in the in-process scenario shards.
The soak records per-call latency (p50/p95/p99) and calls/s per operation (`soak.py`'s report), shown in the job summary and uploaded with the job's artifact (`process-suite`, `release-process-suite`, `image-e2e-results`); like Tier 2, it gates nothing, and only the pass rate can fail.
The calls run one at a time, so the numbers are one caller's latency and rate, not the cluster's capacity (the cluster bench measures that); compare a night with earlier nights of the same job, and read the release image's rows for the shipped build's speed.
The same nightly run profiles one node through the kv-store soak with `perf record -g` and renders a flamegraph with inferno (artifact `merod-flamegraph`, with the folded stacks for `inferno-diff-folded`).
The [testing page](src/content/docs/contribute/testing.mdx#soak-latency-and-throughput) has the commands and a sample.

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

