# calimero-utils-actix - Actix Actor Helpers

`crates/utils` is a grouping directory with a single sub-crate, `crates/utils/actix` (package `calimero-utils-actix`); this file documents that sub-crate: actor-address adapters, a lazily-resolved actor handle, panic supervision for actors, and a macro that wires an actor to a tokio-based event loop with typed streams.

## Package Identity

- **Crate**: `calimero-utils-actix`
- **Entry**: `crates/utils/actix/src/lib.rs`
- **Key deps**: `actix` (actor framework), `tokio` (`rt`, `rt-multi-thread` - drives the global runtime and `block_in_place`), `async-stream` (the `stream!` macro used in `Lazy::init`), `futures-util`, `itertools`, `pastey` (identifier pasting in the `actor!` macro), `tracing` (restart and exit logs in `supervise.rs`), `calimero-primitives` (`Reflect`/`ReflectExt`, `utils::compact_path` for `Debug`)

## Commands

```bash
# Build
cargo build -p calimero-utils-actix

# Test (all, incl. lazy.rs and macros.rs inline test modules)
cargo test -p calimero-utils-actix
```

## Module Inventory

| Item | Module | Kind | Purpose |
| --- | --- | --- | --- |
| `init_global_runtime()` | `lib.rs` | fn | One-time `OnceLock<Handle>` init from `Handle::current()`; errors if called on a current-thread runtime or twice |
| `global_runtime()` | `lib.rs` | fn | Panics if `init_global_runtime` was never called; returns `&'static Handle` |
| `AddrExt::send_stream` | `adapters.rs` | trait (on `Addr<A>`) | Feeds an entire `Stream<Item = M>` to an actor as a single mailbox message (`StreamMessage`), optionally invoking a callback `F` per item |
| `impl_stream_sender!` | `adapters.rs` | macro | Generates the boilerplate `Handler<StreamMessage<S, M, F>>` impl (returns itself, letting `MessageResponse::handle` do the real work) for each listed actor type |
| `ActorExt::forward_handler` | `adapters.rs` | trait (blanket, on `Actor`) | Runs `self.handle(msg, ctx)` and immediately forwards the `MessageResponse` to a `oneshot::Sender`, decoupling handling from reply delivery |
| `Lazy<T>` / `LazyAddr<A>` / `LazyRecipient<M>` | `lazy.rs` | struct / type aliases | An `Addr<A>` or `Recipient<M>` handle usable before the real actor exists: messages queue up and flush once `Lazy::init` binds the live address |
| `actor!` macro | `macros.rs` | macro | Generates `start`/`create`/`start_in_arbiter` for an actor, wiring 0+ named `Box<dyn Stream>` fields into the actor's mailbox via synthetic `FromStreamInner` messages; a panic in the actor or its streams exits the process |
| `restart_on_panic` / `exit_on_panic` | `supervise.rs` | fn | Start an actor on an arbiter so a handler panic either restarts it in place (same actor value, mailbox and `Addr`) or exits the process with `PANIC_EXIT_CODE` |
| `CrashLoopGuard` | `supervise.rs` | struct | Exits instead of restarting once an actor restarts more than `CRASH_LOOP_MAX_RESTARTS` times within `CRASH_LOOP_WINDOW` |

## Mental Model

**`Lazy<T>` (`lazy.rs`)** solves actor-construction cycles: actor B needs a handle to actor A, but A hasn't started yet (or A needs a handle to itself before `ctx.address()` is available). Callers hold a `LazyAddr<A>`/`LazyRecipient<M>` and call `.send(...)`/`.do_send(...)`/`.try_send(...)` on it immediately; before the real `Addr`/`Recipient` is bound, messages are pushed into an internal `VecDeque` guarded by a `tokio::sync::Mutex`. When the owning actor calls `lazy.init(ctx)` inside its own `Actor::started`, `Lazy` resolves its address via `ctx.address()`, drains the queue into the actor's real mailbox, and any `Lazy::recipient::<M>()` clones derived from it resolve too (tracked via a shared `LazyStore` keyed by `TypeId` so all recipients recorded before `init` get flushed together). `sync_lock`/`sync_lock_owned` bridge sync and async call sites: on a multi-thread tokio runtime they use `block_in_place`, otherwise they spin on `try_lock` up to `SYNC_LOCK_BUDGET` (100_000) iterations before panicking - this fallback assumes no real async contention, which only holds in single-threaded tests.

**`actor!` macro (`macros.rs`)** replaces manual `Actor::start`/`create` boilerplate for actors that also need to consume external streams (e.g. a network event stream) alongside normal mailbox messages. `actor!(MyActor => { .field_a, .field_b as SomeType })` takes named `Box<dyn Stream>` fields on the actor, spawns a local tokio task per stream that forwards `Started`/`Value(item)`/`Finished` into the actor's own mailbox as `FromStreamInner<T>` (handled via `StreamHandler<T>` - the actor's existing `started`/`handle`/`finished` stream callbacks fire exactly as if driven by actix's own stream machinery), and polls the actor's own future alongside those forwarding tasks so the actor keeps making progress even while streams are pending.

**`adapters.rs`** is smaller: `send_stream` is the one-shot-stream sibling of the `actor!` macro's per-actor forwarding - useful when you want to push a stream at an *already running* actor from the outside rather than wiring it in at construction. `forward_handler` is used inside a `Handler<M>` impl to answer a message asynchronously via a `oneshot` channel while still running the actor's own synchronous `handle` logic.

## Consumers (verified)

- `crates/network`, `crates/network/primitives`: `actor!` for `NetworkManager`, `LazyAddr`/`LazyRecipient` in the primitives client
- `crates/context`, `crates/context/primitives`: `actor!` and `init_global_runtime` in handlers (`update_application`, `execute`), `LazyAddr` in the context client
- `crates/node`, `crates/node/primitives`: `actor!` and `LazyAddr`/`LazyRecipient` throughout `run.rs`, `handlers.rs`, `network_event_processor.rs`, the node client, and test support
- `crates/server`: `actor!` and `LazyAddr` in `ws.rs`
- `crates/merod`: `actor!` and `init_global_runtime` in `main.rs`
- `AddrExt`/`ActorExt` (`send_stream`/`forward_handler`): `crates/context/src/handlers.rs`, `crates/network/src/handlers/commands.rs`, `crates/node/src/handlers.rs`

`impl_stream_sender!` has no current call sites outside its own definition/tests - it exists to opt an actor type into `send_stream` but nothing in the workspace currently invokes it.

## Key Files

| Path | What's there |
| --- | --- |
| `src/lib.rs` | Global tokio runtime handle (`init_global_runtime`/`global_runtime`), module wiring |
| `src/adapters.rs` | `AddrExt::send_stream`, `StreamMessage`, `impl_stream_sender!`, `ActorExt::forward_handler` |
| `src/lazy.rs` | `Lazy<T>`, `LazyAddr`/`LazyRecipient`, `sync_lock`/`sync_lock_owned`, `Receiver`/`Sender`/`IntoRef`/`IntoEnvelope` traits, `DynErased` type-erasure helper |
| `src/lazy_tests.rs` | Inline tests for `Lazy` (loaded via `#[path]` in `lazy.rs`, `#[cfg(test)]` only) |
| `src/macros.rs` | `actor!` macro and its `__private` support module |
| `src/supervise.rs` | Panic supervision: `restart_on_panic`, `exit_on_panic`, `exit_after_panic`, `CrashLoopGuard` |
| `src/supervise_tests.rs` | Tests for supervision; the exit paths re-run the test binary as a child process and check its exit code |
| `src/macros_tests.rs` | Inline tests for `actor!` (loaded via `#[path]` in `macros.rs`, `#[cfg(test)]` only) |

## Invariants and Gotchas

- **`init_global_runtime` must run on a multi-thread runtime and only once**: both violations return `Err` via `eyre::bail!` rather than panicking, but callers that don't check the result will silently proceed without a global runtime and later panic in `global_runtime()`.
- **`DynErased` relies on trait-object layout**: it type-erases `Weak<dyn Resolve<A>>` via `mem::transmute`, guarded by a `const` layout sanity check at compile time (`src/lazy.rs` lines ~255-293). Do not change `DynErased`'s field layout without re-checking that assertion.
- **`sync_lock`'s current-thread fallback has a hard budget** (`SYNC_LOCK_BUDGET = 100_000` yields) and panics past it; it is only safe when there's no real async contention on the same thread, which is true in single-threaded tests but would be a latent bug if hit in production (production uses `block_in_place` on the multi-thread runtime instead).
- **actix's `Supervisor` does not catch panics**: a panic unwinds through it and drops the actor, leaving every `Addr` pointing at a closed mailbox. Start node actors with `supervise.rs` instead. A restart keeps the same actor value (fields mutated before the panic stay as they were), cancels every future spawned on the actor's context, and runs `started()` again. Use `exit_on_panic` for an actor whose fields or in-flight futures guard work that must not be dropped halfway; run such work for a restartable actor with `actix::spawn`, off its context. A restart also needs a live sender, so keep the `Addr` of a restartable actor for as long as it should run.
- **`Lazy::init` is idempotent by design**: it uses `LazyStore::initialize` (an atomic swap) to guarantee only the first `init` call does the queue-draining work; subsequent calls return `false` immediately.

Part of [crates/](../AGENTS.md).
