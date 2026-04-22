# Benchmarks

Criterion benchmarks for `calimero-storage`. Each bench targets a
concrete performance question tied to an issue or investigation —
not generic coverage.

## Running

All benches in this crate:

```
cargo bench -p calimero-storage
```

A single bench:

```
cargo bench -p calimero-storage --bench merkle_rehash
```

A specific measurement inside a bench (criterion filter syntax):

```
cargo bench -p calimero-storage --bench merge_root_state -- "merge_root_state/1000"
```

Criterion's `bench` profile compiles with release optimisations. Do not
run benches in debug mode — SHA256 and allocation paths are ~20× slower
there and the curve shape will lie to you.

The first run warms up the release build and writes baseline timings
to `target/criterion/`. Subsequent runs compare against the baseline
and report deltas. Delete `target/criterion/` to reset.

HTML reports are written to `target/criterion/report/index.html`.

## Quick iteration

For fast local feedback while iterating on a bench, add `-- --quick`:

```
cargo bench -p calimero-storage --bench merkle_rehash -- --quick
```

`--quick` reduces warm-up and sample counts; numbers are noisier but
the curve shape comes out in ~10s instead of minutes. Use for
development; drop the flag for measurements you intend to cite.

## What lives here

### `merkle_rehash`

Measures `Index::calculate_full_hash_for_children`'s scaling with
child count. Targets issue [#2199] — the suspected "merge-apply full
merkle recompute" bottleneck. Answer so far: linear at ~6M
hashes/second, 17ms at N=100k. Rules out incremental-merkle
maintenance as the #2199 fix.

### `merge_root_state`

Measures `merge::merge_root_state`'s framework overhead (borsh
deserialize → dispatch → borsh serialize) at varying payload sizes.
Also #2199, a different suspect — the nested WASM merge callback.
Answer so far: ~18µs at N=1000 items, linear scaling. Framework cost
isn't the 918ms outlier explanation either.

## Adding a new bench

Benches here follow a simple pattern:

1. **Name after the function under investigation.** `merkle_rehash.rs`
   targets `calculate_full_hash_for_children`; `merge_root_state.rs`
   targets `merge_root_state`. Name should let someone running `cargo
   bench --list` immediately recognise what's being measured.

2. **Tie the bench to a concrete question.** Top-of-file docstring
   must link the issue or PR that motivated the bench, state what
   the hypothesis is, and what the bench would tell us that changes
   a decision. If you can't write that paragraph, the bench probably
   shouldn't exist yet.

3. **Sweep one dimension at a time.** Criterion's `BenchmarkId::from_
   parameter(n)` + `group.bench_with_input(...)` pattern. Pick a
   dimension that matters (item count, payload size, depth) and
   sweep it from "trivially small" to "beyond realistic". The curve
   shape is the signal; a single point tells you nothing.

4. **Register the bench target in `Cargo.toml`.** Every bench file
   needs a matching:

   ```toml
   [[bench]]
   name = "your_bench_name"
   harness = false
   ```

5. **`black_box` inputs and outputs.** Otherwise the optimiser can
   fold the entire benchmark to a constant and you'll measure
   nothing.

## What not to bench

- **Trivial getters / setters.** They'll bench at rustc's noise floor;
  you'll measure benchmark overhead, not the function.
- **Functions with behaviour you don't understand.** Benches tell you
  how fast something is, not whether it's correct. Bench after you've
  profiled and have a specific hypothesis.
- **"Coverage" goals.** A bench that doesn't answer a question tends
  to rot. The criterion baseline diffs on every run; noisy benches
  produce false positives and get ignored, which teaches people to
  ignore benches generally.

## Known gaps

- **Storage-level benches (CRDT merges, `Interface::save_raw` scaling)**
  currently need a concrete `StorageAdaptor`, and the only in-memory
  implementation (`MockedStorage`) is `pub(crate)`. Exposing a public
  in-memory adaptor would unblock this category — tracked as [#2204].

[#2199]: https://github.com/calimero-network/core/issues/2199
[#2204]: https://github.com/calimero-network/core/issues/2204
