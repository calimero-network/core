# calimero-app-engine-wasmtime - the wasmtime engine for `calimero:app`

## Package Identity

- **Crate**: `calimero-app-engine-wasmtime` (`publish = false`, prototype); wasmtime 50 (features: component-model, cranelift, parallel-compilation, pooling-allocator, runtime, std), wasmtime-wasi 50; no wasmtime-wasi-http
- **Entry**: `src/lib.rs` (`WasmtimeEngine`, store data, limiter, import check); `bindings.rs` (`bindgen!` over `../app-wit/wit`, calimero interfaces onto `HostCore`), `wasi.rs` (WASI wiring), `cache.rs` (checked disk cache), `errors.rs` (trap classification)
- **Spec**: `docs/specs/app-runtime-v0.14.md` sections 4-5, D8, D19, D43, D44, S1, S2, S6, S8

## What This Crate Does

Implements `calimero_app_host::Engine`. One `wasmtime::Engine` (fuel, epoch interruption with a 10 ms ticker thread, 200 KiB wasm stack, a pinned feature set with threads, multi-memory, memory64, GC, exceptions, tail calls, relaxed SIMD and component-model async off, pooling allocator with 64 MiB slots) serves every app. `prepare` refuses imports outside the lists, reads the ABI, refuses more than `MAX_MEMORIES` defined memories, compiles through the cache, links the per-component `wasi:http` stubs and builds an `AppPre`. `invoke` runs `handle` in a fresh store per call and returns `HostCore::finish(..)`.

## Contract

- Imports: exactly `allowed_imports() ∪ stubbed_imports()`, plus any listed `wasi:*` interface at an older `0.2.z` patch (z ≤ 12; Rust std imports the stubs at 0.2.9). `wasi:sockets/*`, `wasi:random/insecure*` and everything else: `ForbiddenImport`. Then `NoAbi` / ABI errors.
- Per call: fuel = `budget.fuel`; epoch deadline = `ceil(budget.wall_clock / 10 ms)` ticks; memory = `min(budget.memory_pages, 1,024)` pages; 100,000 table elements; 20 core instances (a wasip2 component holds several), `MAX_MEMORIES` = 1 memory and `MAX_TABLES` = 2 tables (measured: Rust wasip2 and componentize-js 0.23 guests each define 1 memory and 2 tables), `MAX_RESOURCES` = 4,096 host resources in the store's `ResourceTable`.
- Status mapping: `Err(app-error)` → `AppError`; `Trap::OutOfFuel` → `OutOfFuel`; `Trap::Interrupt` → `Timeout`; refused memory/table growth (the limiter traps instead of failing `memory.grow`) → `OutOfMemory`; `Trap::StackOverflow` → `StackOverflow`; `HostDenied` or a full resource table → `HostDenied`; an exhausted instance pool or an OS allocation failure while instantiating → `Timeout` (`"instance pool exhausted"` / `"instance allocation failed"`: node load, not logged, retryable; D23, D36); an instance, memory or table count over the limit at instantiation → `OutOfMemory`; `exit`, any other trap, a host panic (caught with `catch_unwind`) → `Fault`. Trap details are fixed texts or trap codes, never guest data (S10).
- WASI: wall clock = `HostCore::wall_ms`, monotonic = `HostCore::monotonic_ns`, both fixed; `subscribe-*` pollables are ready at once, so a guest sleep never blocks a node thread. `random` = the host core's RNG, requests over the memory size refused. stdout/stderr lines → `HostCore::log` (buffer capped just over `MAX_LOG_BYTES`; a full log drops bytes, never fails the guest). Environment, args, cwd, stdin empty; no terminals; no preopens, so no file descriptor exists; `exit` traps. `wasi:http/types` and `wasi:http/outgoing-handler`: every function the component imports is a per-component stub that traps `HostDenied`, every resource an opaque host type nothing constructs (Ruling 20); wasmtime-wasi-http is not linked, because its undrained body streams block a node thread past the epoch deadline. Real `http/types` will be linked only into action stores when actions land. Nothing reaches the OS.
- Cache: `<sha256(component)>-wasmtime50-<config hash>.cwasm` plus `.cwasm.sha256`; the artifact bytes are hashed in memory and only those bytes are deserialized; mismatch or error recompiles and rewrites both (temp file + rename). The sidecar catches corruption and torn writes, not a party that can write the cache dir: S8 requires the caller to place `cache_dir` inside the node's encrypted store.

## Tests

The `testing` feature exposes `WasmtimeEngine::with_pool_size` so a test can exhaust the pool.

`tests/engine.rs` builds the standalone guests under `tests/guests/*` (each its own `[workspace]`, built with `calimero_app_build::build_component` into one shared target dir under `CARGO_TARGET_TMPDIR`). Every manual ABI includes `"abiVersion": 1`.

## Verify

`cargo test -p calimero-app-engine-wasmtime` and `cargo clippy -p calimero-app-engine-wasmtime --all-targets -- -D warnings`
