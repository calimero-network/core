# Per-method read/write intent (parallel-read accelerator)

**Status:** design — decisions resolved, ready for implementation.
**Issue:** #2684 · **Epic:** #2022 · **Consumes:** #2685 increment 2

## 1. Problem

The per-context lock is now an `RwLock` (#2698), but every caller still takes the
exclusive *write* guard, so it behaves exactly like the prior mutex — no
parallelism yet. To let read-only calls on a single context run concurrently,
the node must choose **read vs write lock at acquisition time, before the method
runs**.

Today there is no up-front signal for that choice:

- The node only learns whether a call mutated state **after** execution, by
  inspecting `outcome.artifact` (`crates/context/src/handlers/execute/mod.rs:787`)
  / `outcome.root_hash` (`mod.rs:1455`).
- Method **names** are app-author conventions, not enforced — a `get_x` may
  legally mutate. Classifying by name would let a writer take a read lock →
  concurrent writers on one context → non-deterministic deltas / split-brain.

This doc proposes a trustworthy, compiled-in, per-method intent signal, plus the
runtime enforcement that makes a wrong declaration fail safely rather than
corrupt state. It is a **pure opt-in optimization**: with the fail-safe default
(unknown intent → write lock), nothing here is required for correctness, and apps
that never adopt it behave exactly as today.

## 2. Why post-hoc detection is not enough under a read lock

There is already a post-execution "ran but shouldn't have written" path — the
**read-only member** enforcement at `mod.rs:1455-1465`:

```rust
if executor_is_read_only && outcome.root_hash.is_some() {
    // "ReadOnly member attempted state mutation — discarding changes"
    outcome.root_hash = None;
    outcome.artifact.clear();
    outcome.xcalls.clear();
    return Ok((outcome, None, None, None));
}
```

This is safe **only because that execution holds the exclusive lock**: the
mutation is computed into `outcome` and discarded before it is committed, and no
other execution is touching the context concurrently.

That property does **not** hold once a read-declared method runs under a *shared*
read lock. WASM execution mutates the shared in-memory Merkle index *during* the
run (the cause of past execute-vs-sync split-brain races), so a mis-declared
writer running alongside concurrent readers would corrupt them *before* the
post-hoc `artifact`/`root_hash` check ever runs. Discarding the outcome
afterwards is too late.

**Conclusion:** a read-lock execution must be **prevented from writing at the
source**, not corrected afterwards. The post-hoc check stays as defense-in-depth
(it turns a violation into a clean error / metric), but the primary guarantee is
a write-rejecting storage view (§5).

## 3. Surfaces (current code)

| # | Surface | Crate | Key location |
|---|---|---|---|
| 1 | method model + attribute parsing | `calimero-sdk-macros` | `src/logic/method.rs:23` (`PublicLogicMethod`), `:355` (`#[app::init]` parse), `:19` (`enum Modifer`) |
| 2 | ABI `Method` struct + embed/read | `calimero-wasm-abi` | `src/schema.rs:68` (`struct Method`), `src/emitter.rs:494` (emit), `src/embed.rs:44` (`read_embedded_state_schema`, section `calimero_abi_v1`) |
| 3 | runtime dispatch | `calimero-runtime` | `src/lib.rs:238` (`struct Module`), `:251` (`Module::run`), `:336` (`execute_wasm`) — **no manifest held at call time** |
| 4 | execute lock site | `calimero-context` | `mod.rs:108` (`context.lock()`), `:102` (`is_state_op`), `:787` / `:1455` (post-hoc mutation checks) |

Two facts drive the design:

- The ABI manifest already exists and is embedded in the wasm custom section
  `calimero_abi_v1`; it is currently read only at the upgrade-validation gate
  (`upgrade_group.rs:535`), **not** at method dispatch. `Method` has no intent
  field today.
- At the lock site (`mod.rs:108`) the handler already knows `method` (the name)
  and `current_application_id` (read from `context.meta.application_id` *before*
  the lock). It does **not** yet have the module loaded — `get_module` runs later
  (~`mod.rs:555`).

## 4. Where the intent comes from — three options

### Option A — explicit `#[app::view]` attribute (opt-in)
Add `Modifer::View`, parsed exactly like `#[app::init]` (`method.rs:355`), and a
matching `read_only: bool` on the ABI `Method` (`schema.rs:68`) set by the
emitter when it sees the attribute. Default = mutating.

- ➕ Explicit and unambiguous; trivial to reason about.
- ➖ Every read method needs annotating; old code gets zero benefit until edited.

### Option B — infer from the `&self` receiver (default-on), with override
The macro already models the receiver (`self_type: Option<SelfType>` at
`method.rs:26`). Treat `&self` methods as read-only candidates and `&mut self` as
mutating, with `#[app::view]` / `#[app::mutate]` as explicit overrides for edge
cases.

- ➕ **Old code benefits on recompile with no edits** — existing `&self` getters
  become parallel-readable automatically. Best matches "just an optimization."
- ➖ `&self` is necessary but not *sufficient* for read-only: a `&self` method can
  still write via `env::` host calls, `unsafe`, or interior mutability. So
  inference is only sound **in combination with the §5 write-rejecting view**
  (which it needs anyway). A `&self` method that does write becomes a clean
  runtime error instead of silent mutation.

### Option C — caller-supplied hint, ABI-validated
RPC marks a call read-only; node verifies against the ABI before honoring it.

- ➖ Adds a wire/API surface and a trust-but-verify step for no gain over A/B;
  not recommended.

**Decision: A — explicit `#[app::view]`, tri-state ABI.** See §10.

## 5. Runtime enforcement — the load-bearing part

Selecting a read lock is only safe if a read-lock execution **cannot** mutate
shared state. Proposal:

1. **Write-rejecting storage view.** When the execute path takes a read guard, it
   passes the runtime a `ContextStorage` (and private storage) whose write host
   functions (`storage_write`, delete, etc.) return an error instead of
   mutating. A method that attempts a write under a read lock fails with a
   deterministic `FunctionCallError` — no shared structure is touched, so
   concurrent readers are unaffected. (This generalizes the existing read-only
   member machinery from "discard after" to "reject during.")
2. **Post-hoc assertion (defense-in-depth).** Keep the `outcome.root_hash` /
   `outcome.artifact` check (`mod.rs:1455`/`:787`); for a read-declared call it
   must be empty. A non-empty artifact here is a *bug* (the write-rejecting view
   leaked) — fail the call and emit a metric, do not commit.
3. **Sync stays exclusive.** `is_state_op` (`__calimero_sync_next`,
   `mod.rs:102`) is always a writer → always the write guard, never inferred.

## 6. Threading intent to the lock site

The lock is taken at `mod.rs:108`, before `get_module`. To pick the guard we need
intent there. Proposed flow (fail-safe at every gap):

1. Resolve a per-application **read-only method set** from the embedded ABI
   (`read_embedded_state_schema`) **once**, when the module is compiled/cached,
   and store it next to the cached module (either a new field on
   `calimero_runtime::Module`, or a sibling `BoundedCache<ApplicationId,
   Arc<ReadOnlySet>>` in `ContextManager` to avoid coupling the runtime crate to
   `calimero-wasm-abi`).
2. At `mod.rs:108`, before locking: if the set is **cached** and contains
   `method`, and it is not a state-op → take the **read** guard; otherwise
   (cache miss, method absent, unannotated module, any uncertainty) → take the
   **write** guard.
3. A cold application (set not yet cached) simply takes the write guard on the
   first call and can use the read guard once warm. No reordering of the
   expensive module compile; no correctness risk.

`ContextGuard` gains its `Read` variant here (deferred from #2698 per the
no-dead-code rule) — it is finally *minted* in this work.

## 7. Atomic-batch (`ContextAtomic::Held`) interaction

A guard handed back for a multi-call atomic batch (`mod.rs:108`,
`ContextAtomicKey`) may have been taken as a read guard, then a later call in the
batch may need to write. An `RwLock` read guard cannot be upgraded in place.
Rule: **any atomic batch that may contain a write takes the write guard up
front.** Batches are the rare path; defaulting them to exclusive costs nothing
and sidesteps the upgrade gap entirely. (Single read-only calls — the common
case — are unaffected.)

## 8. Backward compatibility

- Modules with no intent metadata (every pre-#2684 build) → `Unspecified` →
  write guard → **exactly today's behavior**, no recompile required.
- ABI gains an optional field (`#[serde(skip_serializing_if)]`), so old manifests
  still deserialize and new manifests are readable by old nodes.
- No wire-format or RPC change.

## 9. Phasing

1. **ABI + macro:** tri-state intent on `Method`; `#[app::view]` parsed by macro
   and emitter; default `Unspecified`. (No runtime behavior change yet.)
2. **Runtime enforcement:** write-rejecting storage view for read-lock
   executions + post-hoc assertion.
3. **Lock selection:** read-only set cache + guard selection at `mod.rs:108`;
   mint `ContextGuard::Read`; intent-aware eviction (folded #2683).
4. *(Optional, later)* flip `Unspecified`+`&self` to read-only (Option B) — pure
   default change, no ABI break.

## 10. Decisions

All open questions resolved.

**Q: Explicit `#[app::view]` (A) or infer-from-`&self` (B) for v1?**
**→ A — explicit `#[app::view]`, tri-state ABI (`ReadOnly`/`Mutating`/`Unspecified`).**
Smallest scope, fully explicit, easy to review. The tri-state field leaves the
door open to flip `Unspecified`+`&self` to read-only later (no ABI break).

**Q: Where does the per-application read-only method set live?**
**→ Sibling `BoundedCache<ApplicationId, Arc<ReadOnlySet>>` in `ContextManager`.**
Populated from the embedded ABI when the module is loaded into the module cache.
Keeps `calimero-runtime` decoupled from `calimero-wasm-abi`.

**Q: How are writes blocked during a read-lock execution?**
**→ Read-only `ContextStorage` wrapper** — storage handle passed to the runtime
has write/delete methods that return an error when the execution holds a read
guard. Fits the existing `Storage` trait surface and mirrors the shape of the
existing read-only-member machinery.

**Q (factual): Does the ABI emitter see `#[app::view]` reliably?**
**→ Yes.** `crates/wasm-abi/src/emitter.rs` is a `syn::visit::Visit` source
parser — it already reads `#[app::state]` attributes (`emitter.rs:282`) and the
full method signature via `visit_item_impl` (`:437`). It will see `#[app::view]`
directly; the proc-macro does not need to emit anything into the WASM section
independently.
