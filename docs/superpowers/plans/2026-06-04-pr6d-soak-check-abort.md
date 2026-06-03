# PR-6d — Soak + `migration_check` + Logical Abort — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking. One task per fresh subagent with two-stage review.

**Goal:** Health-check a produced v2 migration root **before** it is committed, and let a failing check (or an admin) **logically abort** the migration — discarding the produced v2 root and leaving the still-untouched v1 root and all committed user data intact. Ship `migration_check` as an SDK macro export + a built-in helper library, wired into the whole-root migrate flow ahead of the commit, plus an admin abort RPC + HTTP route. Shape the check/soak surface so a future canary-subgroup gate drops in later — but **do not build canary now**.

**Architecture (logical-abort, NOT byte-restore):** The whole-root migrate path is the same one PR-6a left non-freezing. Its single load-bearing property here: **the v1 root is never mutated until `write_migration_state` commits** (`crates/context/src/handlers/update_application/mod.rs:445`). Everything before that — `execute_migration` producing `new_state_bytes` (`:428`) — is pure computation against a still-v1 store, plus a `storage.commit()` of merge-mode-buffered child-collection writes whose keys are deterministic and idempotently overwritten on retry (`:583-585`, see the long comment at `:554-582`). So "abort" means: **don't call `write_migration_state`, don't finalize the new `application_id`** — return an error and surface a metric. There is **NO byte snapshot and NO restore** — none exists; #2644 is a write-freeze, not a checkpoint; replicated deltas can't be recalled (spec §3 decision 10, §7 invariant 5). Any code or comment implying "restore the v1 bytes" is wrong and must be rejected in review.

> **One subtlety to honor (from the `execute_migration` comment at `:554-582`):** the merge-mode child-collection writes ARE committed to the store at `:583-585` *before* the root is written, deliberately, because the entries are deterministic and overwritten in-place on retry. A failed `migration_check` therefore leaves those v2-shaped child entries under a **still-v1 root** — exactly the same "less-bad, idempotent" state a crash between commit and root-write would leave, and identical to a retry. This is acceptable and is the documented failure mode; the abort path does **not** try to delete them (that would be a byte-restore we explicitly don't do). The v1 root still points only at v1 entries, so reads stay v1-correct. Add a test asserting this exact post-abort shape (Task 6d.3 Step 1).

**Tech Stack:** Rust workspace (`crates/{sdk,sdk/macros,context,runtime,server,context-client}`), borsh, WASM runtime (wasmtime), TestHost in-process harness (`crates/sdk/src/testing.rs`, shipped #2674), merobox e2e (`workflows/app-migration/`).

**Spec:** `docs/superpowers/specs/2026-06-03-pr6-expand-contract-design.md` — §3 decision 10, §5 "PR-6d", §6, §7 invariant 5, §8 (canary deferred), §10 (full canary-subgroup gating is a non-goal here). Read it first.

**Branch / base:** Off the PR-6a branch `feat/2539-pr6a-migration-v2` (per the standing test-placement rule: dependents branch off their predecessor, not master). 6d depends on **6a** only (the non-freezing flag + the clean-rollback property), NOT on 6b/6c. Flag-gated behind `migration_v2` like the rest of the train. `#2674`'s `crates/sdk/macros/src/{migrate_derive.rs,state.rs}` + `crates/sdk/src/testing.rs` are on `origin/master` (`e240223a`) and must be present in the working tree before starting (rebase the 6a branch onto a base that includes #2674, or cherry-pick — confirm in Task 6d.0).

---

## Where `migration_check` hooks into the migrate flow (the one diagram that matters)

```
update_application_with_migration()           crates/context/src/handlers/update_application/mod.rs:394
  └─ verify_appkey_continuity()                                          :416   (v1 store untouched)
  └─ if let Some(migration_params):
       ├─ (new_state_bytes, events, logs) = execute_migration(...)       :428   produces v2 root bytes
       │     └─ storage.commit() of merge-mode child writes              :583   (deterministic, idempotent)
       │
       ├─ ★ NEW: run_migration_check(&module, &context, &new_state_bytes)  ← INSERT HERE (Task 6d.3)
       │     pass  → fall through to write_migration_state
       │     fail  → LOGICAL ABORT: return Err(MigrationCheckFailed)      (skip write + finalize)
       │
       ├─ write_migration_state(... new_state_bytes ...)                 :445   ← FIRST mutation of v1 root
       │     └─ Interface::write_pre_merged_root_state                    crates/storage/src/interface.rs:2676
       ├─ context.root_hash = ...; context.dag_heads = ...               :450-455
       └─ (emit events)
  └─ finalize_application_update(...)                                    :488   commits new application_id
```

The check runs on the **produced v2 root** (`new_state_bytes`, the same bytes `write_migration_state` would persist) with the **old v1 root** still readable from the store via `read_raw()`. Because the early-return happens before `write_migration_state` AND before `finalize_application_update`, a failed check leaves the committed context (root_hash, dag_heads, application_id) fully on v1.

**How the app's `migration_check` is invoked:** it's a second `#[no_mangle] extern "C"` WASM export (named `__calimero_migration_check`, like `#[app::migrate]` exports a no-mangle fn) on the **v2 module**, invoked via `module.run(context_id, executor, "__calimero_migration_check", input=&new_state_bytes, &mut storage, None, Some(node_client))` (`crates/runtime/src/lib.rs:251`). Inside the export: the **old** root is read via `read_raw()` (still v1 in the store, exactly as `#[app::migrate]` reads it), the **new** root is the `input` bytes (borsh-deserialized into the v2 state type). The export returns a borsh `Ok::<bool,_>` via `value_return`. This avoids a host that can't deserialize app-typed bytes (only wasm can) — same constraint that forced whole-root migrate into wasm (spec §2 theme B).

---

## File-touch map

- **SDK macro export:** `crates/sdk/macros/src/migration.rs` (new `migration_check_impl`, sibling of `migrate_impl`), `crates/sdk/macros/src/lib.rs` (new `#[proc_macro_attribute] pub fn migration_check`), `crates/sdk/src/lib.rs` (re-export in the `app` glob, line ~40).
- **Built-in helper library:** `crates/sdk/src/migration_check.rs` (new module: `entity_count_parity`, `no_orphaned_refs`, `conservation` helpers), `crates/sdk/src/lib.rs` (`pub mod migration_check;`).
- **Flow wiring + logical abort:** `crates/context/src/handlers/update_application/mod.rs` (`run_migration_check` helper + the insert at `:428`→`:445`; new `MigrationCheckFailed` error variant).
- **Admin abort RPC:** `crates/context/src/lib.rs` (ContextClient `abort_migration`), a new `crates/context/src/handlers/abort_migration.rs` handler (flips the schema/upgrade target back), `crates/context-client/src/...` request type (mirror `GetCascadeStatusRequest`), `crates/server/src/admin/handlers/groups/abort_migration.rs` + route in `crates/server/src/admin/service.rs` (~`:230`, beside `cascade-status`), `crates/server-primitives/src/admin.rs` API types.
- **Tests:** `crates/sdk/src/migration_check.rs` unit tests via TestHost; `crates/sdk/macros/src/migration.rs` expansion tests; `crates/context/src/handlers/update_application/mod.rs` abort-flow unit test; merobox `workflows/app-migration/{29,30,31}-*.yml` + a v2 fixture app exporting a failing `migration_check`.

---

## Task 6d.0: Confirm base includes #2674 + characterize the clean-rollback property

**Files:**
- Read-only verification; one regression-guard test in `crates/context/src/handlers/update_application/mod.rs` test module (`:899`).

- [ ] **Step 1 — Verify base:** confirm the working tree has `crates/sdk/macros/src/migrate_derive.rs`, `crates/sdk/macros/src/state.rs`, and `crates/sdk/src/testing.rs` (`ls crates/sdk/macros/src/migrate_derive.rs crates/sdk/src/testing.rs`). If absent, rebase `feat/2539-pr6a-migration-v2` onto a base that includes #2674 (`origin/master` `e240223a`) **before proceeding** — do not implement against a stale tree. STOP and report if the rebase conflicts non-trivially (blocking unknown).
- [ ] **Step 2 — Write a characterization test** locking the property 6d exploits: a `write_migration_state`-free run of the migrate flow leaves the committed root entry unchanged. Drive it at the storage seam — install a v1 root entry, assert its `full_hash`, run `execute_migration`'s effects only up to (not including) `write_migration_state`, and assert the root entry's `full_hash` is **identical**. (If `execute_migration` is hard to call in isolation, instead assert the narrower invariant: `write_migration_state` is the only writer of the root `Index`/`Entry` in this file — `rg "write_pre_merged_root_state|get_index|put.*Index" crates/context/src/handlers/update_application/mod.rs` shows the single call site at `:826`.)
- [ ] **Step 3 — Run, expect PASS** (`cargo test -p calimero-context clean_rollback`). Documents current truth; locks it so 6d's abort can rely on it.
- [ ] **Step 4 — Commit:** `test(context): characterize migrate clean-rollback (v1 root untouched pre-commit)`.

```
Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
```

---

## Task 6d.1: `#[app::migration_check]` macro export (no helper logic yet)

**Files:**
- Modify: `crates/sdk/macros/src/migration.rs` (add `migration_check_impl`, mirror `migrate_impl` at the top of the file).
- Modify: `crates/sdk/macros/src/lib.rs` (add `#[proc_macro_attribute] pub fn migration_check`, beside `migrate` at `:208`).
- Modify: `crates/sdk/src/lib.rs` (`app` re-export glob at `:40`).

- [ ] **Step 1 — Write the failing expansion test** in `migration.rs`'s `#[cfg(test)] mod tests` (mirror `migrate_expansion_produces_wasm_export`): the macro on `fn check(old: AppV1, new: AppV2) -> bool { .. }` expands to a `#[cfg(target_arch="wasm32")] #[no_mangle] pub extern "C" fn __calimero_migration_check()` that (a) sets up the panic hook, (b) reads old via `read_raw()` + borsh-deserializes into the `old` param type, (c) borsh-deserializes the new state from `env::input()`, (d) calls the user body, (e) `value_return`s a borsh `Ok::<bool, Vec<u8>>`. Assert the expansion contains `extern \"C\"`, `__calimero_migration_check`, `read_raw`, `input`, `value_return`, `no_mangle`, and preserves a native stub under `not(target_arch = "wasm32")`.
```rust
#[test]
fn migration_check_expansion_produces_wasm_export() {
    let input = quote! { fn check(old: AppV1, new: AppV2) -> bool { old.len() == new.len() } };
    let out = migration_check_impl(TokenStream::new(), input).to_string();
    assert!(out.contains("extern \"C\""), "{out}");
    assert!(out.contains("__calimero_migration_check"), "{out}");
    assert!(out.contains("read_raw"), "{out}");
    assert!(out.contains("value_return"), "{out}");
}
```
- [ ] **Step 2 — Run, expect FAIL** (`cargo test -p calimero-sdk-macros migration_check_expansion` → "function `migration_check_impl` not found").
- [ ] **Step 3 — Implement `migration_check_impl`:** parse the `ItemFn`; require exactly two params (`old: OldTy`, `new: NewTy`) and a `-> bool` return (emit a calimero-branded `compile_error!` otherwise). Generate the no-mangle export: `setup_panic_hook()`; bind `old` = `read_raw()`-then-`borsh::from_slice::<OldTy>` (panic-with-message on `None`/deser error, copying `migrate_impl`'s panic style); bind `new` = `borsh::from_slice::<NewTy>(&env::input())`; run the user block; `env::value_return(&Ok::<bool, Vec<u8>>(result))`. Keep a native stub for testing. **Do NOT** wrap in `with_merge_mode` (this is a read-only predicate, not a state producer; no deterministic-id assignment). Register `migration_check` in `lib.rs` (`migration::migration_check_impl(args.into(), input.into()).into()`) and re-export in `sdk/src/lib.rs`.
- [ ] **Step 4 — Run, expect PASS.** Also `cargo build -p calimero-sdk` to confirm the re-export resolves.
- [ ] **Step 5 — Commit:** `feat(sdk-macros): add #[app::migration_check] wasm export`.

```
Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
```

---

## Task 6d.2: Built-in `migration_check` helper library (TestHost-tested)

**Files:**
- Create: `crates/sdk/src/migration_check.rs` (`entity_count_parity`, `no_orphaned_refs`, `conservation`).
- Modify: `crates/sdk/src/lib.rs` (`pub mod migration_check;`).
- Test: in-module `#[cfg(test)]` using `calimero_sdk::testing::TestHost` + `assert_migrate_converges` (shipped #2674) to build real v1/v2 states deterministically.

- [ ] **Step 1 — Write failing unit tests** that exercise the three helpers against TestHost-built states:
  - `entity_count_parity<C1, C2>(old: &C1, new: &C2) -> bool` — true iff `old.len() == new.len()` for two collections (generic over a `Len`-like trait or `IntoIterator` count). Test: a migrate that drops an entry fails parity; a 1:1 carry passes.
  - `no_orphaned_refs(new, ref_extractor, key_set_extractor) -> bool` — true iff every reference returned by `ref_extractor(new)` is present in `key_set_extractor(new)`. Test: a v2 with a dangling foreign-key fails; a consistent one passes.
  - `conservation<T: PartialEq>(old_total: T, new_total: T) -> bool` — wraps an equality of an app-computed invariant (e.g. summed balances). Test: equal totals pass; off-by-one fails.
```rust
#[test]
fn entity_count_parity_detects_dropped_entry() {
    let host = TestHost::new(|| AppV1::seed_two());     // 2 entries
    let h2 = host.migrate(|| migrate_drop_one());        // drops to 1
    assert!(h2.view(|new| !entity_count_parity_against_v1(new))); // helper fails
}
```
- [ ] **Step 2 — Run, expect FAIL** (`cargo test -p calimero-sdk migration_check::tests` → missing module/functions).
- [ ] **Step 3 — Implement** the three helpers as small, allocation-light, generic-where-sensible pure functions. Keep them **dependency-free of any host call** — they operate on already-deserialized app values so the app author composes them inside their `#[app::migration_check]` body. Document each with a doc-example. Re-export under `calimero_sdk::migration_check::*`.
- [ ] **Step 4 — Run, expect PASS.**
- [ ] **Step 5 — Commit:** `feat(sdk): built-in migration_check helpers (entity-count parity, no-orphaned-refs, conservation)`.

```
Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
```

---

## Task 6d.3: Wire the pre-commit check + logical abort into the migrate flow

**Files:**
- Modify: `crates/context/src/handlers/update_application/mod.rs` — new `async fn run_migration_check(...)` + the insert between `:436` and `:445`; new error path. Gate on the `migration_v2` flag already threaded by 6a (`ContextClient::with_migration_v2`, `crates/context/src/lib.rs:438`).

- [ ] **Step 1 — Write the failing test:** in the `#[cfg(test)] mod tests` (`:899`), a test `failed_migration_check_logically_aborts` asserting that when `run_migration_check` returns `Ok(false)`, `update_application_with_migration` returns `Err(MigrationCheckFailed)` AND the committed root entry's `full_hash` is **identical** to the pre-migration v1 hash (clean rollback) AND the context's `application_id` is unchanged (finalize skipped). Also assert the documented child-entry shape from the plan preamble (v2-shaped child entries may exist under the still-v1 root — they are idempotent residue, not corruption; the v1 root references only v1 entries). Drive it with an in-memory store + a stub module whose `migration_check` export returns `false` (or factor `run_migration_check` to take an injectable `check_result: Option<bool>` for the unit seam, exercising the real wasm path in the merobox e2e at 6d.4).
```rust
#[tokio::test]
async fn failed_migration_check_logically_aborts() {
    // install v1 root; record full_hash_v1
    // run flow with migration_v2=true and a check that returns false
    let err = run_flow_expecting_err().await;
    assert!(matches!(err, ExecuteError::MigrationCheckFailed { .. }));
    assert_eq!(root_full_hash_after(), full_hash_v1, "v1 root must be untouched (logical abort)");
    assert_eq!(context_after.application_id, app_id_v1, "new app id must NOT be finalized");
}
```
- [ ] **Step 2 — Run, expect FAIL** (no `MigrationCheckFailed` variant / no `run_migration_check`).
- [ ] **Step 3 — Implement:**
  - Add `run_migration_check(module: &Module, context: &Context, new_state_bytes: &[u8], executor, node_client) -> eyre::Result<bool>`: probe the v2 module for the `__calimero_migration_check` export; **if absent, return `Ok(true)`** (no check defined ⇒ never block — backwards compatible). If present, `module.run(context.id, executor, "__calimero_migration_check", new_state_bytes, &mut ContextStorage::from(...), None, Some(node_client))` and borsh-decode the returned `Result<bool, Vec<u8>>` from `outcome.returns` (mirror `execute_migration`'s extraction at `:592-598`). A wasm trap ⇒ propagate as a check failure (treat as `false`/error — fail-closed). **Run it on a non-committing storage view** (the export only reads `read_raw` + its input; it must not persist — assert no delta is emitted, call `clear_pending_delta()` after like `write_migration_state` does at `:860`).
  - Insert after `:436` (post-`execute_migration`, **before** `write_migration_state` at `:445`), guarded by the `migration_v2` flag: `if migration_v2 { match run_migration_check(...).await { Ok(true) => {}, Ok(false) | Err(_) => { /* LOGICAL ABORT */ warn!(...); metric; return Err(ExecuteError::MigrationCheckFailed { context_id }); } } }`. The early `return` skips `write_migration_state` AND `finalize_application_update` — that **is** the logical abort. **Add a code comment stating explicitly: this is a logical abort — there is no byte snapshot to restore; v1 is intact because it was never mutated.**
  - Add the `MigrationCheckFailed` error variant to the handler's error enum.
- [ ] **Step 4 — Run, expect PASS.** Also `cargo test -p calimero-context` full lib suite (the 6a baseline was 87 passing — keep it green).
- [ ] **Step 5 — Commit:** `feat(context): run migration_check pre-commit; failed check logically aborts (migration_v2)`.

```
Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
```

---

## Task 6d.4: Admin abort RPC + HTTP route (flip the schema target back)

**Files:**
- Create: `crates/context/src/handlers/abort_migration.rs` — handler that flips the group/context upgrade target back to v1 (re-point the upgrade status to the prior `GroupUpgradeValue`/`Completed{prior}` and drop any pending migration marker). Reuse the upgrade-status write seam in `crates/context/src/handlers/upgrade_group.rs` (`GroupUpgradeStatus::Completed`, `:213`/`:473`) — set the target back to the pre-migration `app_key`. **No byte restore** — only the schema/target pointer flips; any already-committed v2 context stays as-is (this RPC is for aborting an *in-flight / not-yet-applied* migration, e.g. before a lazy context migrates).
- Modify: `crates/context/src/lib.rs` (ContextClient `pub async fn abort_migration(...)`), `crates/context-client/src/...` (`AbortMigrationRequest`, mirror `GetCascadeStatusRequest`).
- Create: `crates/server/src/admin/handlers/groups/abort_migration.rs` (mirror `get_cascade_status.rs` exactly: `Path<group_id>`, `Extension<AdminState>`, call `ctx_client.abort_migration(...)`, `parse_api_error`).
- Modify: `crates/server/src/admin/service.rs` (add route `POST /groups/:group_id/migration/abort` beside `cascade-status` at `:230`), `crates/server-primitives/src/admin.rs` (request/response API types).

- [ ] **Step 1 — Write the failing test:** unit-test the `abort_migration` handler: given a group whose upgrade status targets a v2 `app_key`, after `abort_migration` the persisted `GroupUpgradeValue` targets the prior v1 `app_key` and the upgrade status is no longer `InProgress`/pending-migration. Assert it is **idempotent** (aborting an already-aborted/never-started migration is a no-op `Ok`, not an error).
- [ ] **Step 2 — Run, expect FAIL** (no `abort_migration`).
- [ ] **Step 3 — Implement** the handler (flip target, clear pending marker, idempotent), the ContextClient method, the request type, the axum handler, the route, and the API primitives. Mirror the `get_cascade_status` request/response/handler shapes throughout. Authorize via the same admin gate as the other `/groups/:id/upgrade*` routes (no new auth surface).
- [ ] **Step 4 — Run, expect PASS** (`cargo test -p calimero-context abort_migration` + `cargo build -p calimero-server`).
- [ ] **Step 5 — Commit:** `feat(context,server): admin abort-migration RPC + HTTP route (logical target flip)`.

```
Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
```

---

## Task 6d.5: Shape the soak/check surface for a future canary (deferred — interface only)

**Files:**
- Modify: `crates/context/src/handlers/update_application/mod.rs` (the `run_migration_check` call site / a thin `MigrationGate` enum or `check_only` param) — **no behavior**, just the seam.

- [ ] **Step 1 — Write the failing test:** assert a `MigrationGateDecision` (or equivalent) type exists with variants `{ Commit, Abort }` produced from the check result, and that today only `Commit`/`Abort` are reachable (a future `Canary { subgroup }` variant is *documented as deferred*, not constructed). This is a compile-level seam test ensuring the call site funnels its commit/abort decision through one enum so canary can extend it later without touching the abort logic.
- [ ] **Step 2 — Run, expect FAIL.**
- [ ] **Step 3 — Implement** the small decision enum + funnel `run_migration_check`'s result through it. Add a doc-comment: "Canary-subgroup gating (spec §8, §10) plugs in here as a third decision — deferred; do NOT build it in 6d." **No soak timer, no canary subgroup, no gating** — those are out of scope per §10.
- [ ] **Step 4 — Run, expect PASS.**
- [ ] **Step 5 — Commit:** `refactor(context): funnel migration commit/abort through MigrationGateDecision (canary-ready, deferred)`.

```
Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
```

---

## Task 6d.6: e2e — check pass commits, check fail logically aborts, admin abort

**Files:**
- Create v2 fixture(s): `apps/migrations/*` — a v2 app exporting `#[app::migration_check]` that **passes**, and a sibling that deliberately **fails** (e.g. drops entries so `entity_count_parity` is false). Update `workflows/app-migration/build-wasms.sh` if a new fixture is added.
- Create: `workflows/app-migration/29-migration-check-pass.yml`, `30-migration-check-fail-abort.yml`, `31-admin-abort.yml` (model on existing app-migration scenarios + `21-reads-available-during-upgrade.yml`).
- Modify: `.github/workflows/app-migration-e2e.yml` matrix (+29, +30, +31).

- [ ] **Step 1 — Write `29-migration-check-pass.yml`:** node started `migration_v2=true`; namespace migration whose `migration_check` returns true → state migrates to v2, both nodes converge. Assert `assert_log_present "migration_check passed"` (add that log line in 6d.3).
- [ ] **Step 2 — Write `30-migration-check-fail-abort.yml`:** identical setup but the v2 app's `migration_check` returns false → assert the migration does NOT commit (`assert_log_present "migration_check failed: logical abort"`), the app still serves **v1** state correctly (a read returns pre-migration data), and the context's application_id is unchanged. This is the headline logical-abort test.
- [ ] **Step 3 — Write `31-admin-abort.yml`:** trigger a (lazy) namespace migration, call `POST /groups/:id/migration/abort` before contexts apply, assert the upgrade target is flipped back and contexts stay v1.
- [ ] **Step 4 — Run locally** (`merobox bootstrap run workflows/app-migration/29-...yml --image merod:local --e2e-mode`, then 30, 31), expect PASS. (Docker + `merod:local` image required — CI/Docker-gated like 6a.5; if unavailable, STOP and report as the blocking unknown, do not fake.)
- [ ] **Step 5 — Commit:** `test(e2e): migration_check pass/fail-abort + admin abort (scenarios 29-31)`.

```
Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
```

---

## Self-review

**Spec coverage:** §3 decision 10 (migration_check on produced v2 root before commit; logical abort; admin abort RPC; no byte snapshot) → 6d.1/6d.2/6d.3/6d.4. §5 PR-6d (built-in helpers: parity/orphans/conservation) → 6d.2. §6 author surface (`migration_check` export sits beside `#[app::migrate]`) → 6d.1. §7 invariant 5 (abort is logical, never restores bytes) → asserted in 6d.3 + comment + 6d.0 characterization. §8/§10 (canary deferred, never built) → 6d.5 seam only. Testing surface §11 (migration_check pass/fail, admin abort, mixed coexistence) → 6d.6.

**Logical-abort discipline:** every task that touches the abort path (6d.0, 6d.3, 6d.4, 6d.5) restates "no byte snapshot / no restore / v1 untouched because never mutated." Reviewer must reject any "restore v1 bytes" code. The one accepted residue — deterministic v2 child entries under a still-v1 root after a failed check — is documented (matches the existing crash/retry failure mode at `update_application/mod.rs:554-582`) and explicitly NOT cleaned up.

**Backwards compatibility:** no `#[app::migration_check]` export ⇒ `run_migration_check` returns `Ok(true)` ⇒ flow unchanged. All new behavior is behind the `migration_v2` flag (default off, threaded by 6a). master behavior unchanged with the flag off.

**Type consistency:** `__calimero_migration_check` export name, `run_migration_check` helper, `MigrationCheckFailed` error, `MigrationGateDecision` enum, `abort_migration` RPC used consistently across tasks. Helper names `entity_count_parity`/`no_orphaned_refs`/`conservation` match spec §5.

**Placeholder scan:** all six tasks are line-level TDD (failing test → impl → commit) with concrete anchors. 6d.6 is Docker-gated (same constraint as 6a.5/6a.6) and marked STOP-and-report if the image is unavailable.

---

## Blocking unknowns (resolve before / during implementation)

1. **#2674 in the working tree** — the 6a branch is based on `f25cab38`, which **predates** #2674. The `migrate_derive.rs`/`state.rs`/`testing.rs` surfaces this plan builds on live on `origin/master` (`e240223a`). 6d.0 Step 1 must rebase the 6a branch onto a #2674-inclusive base first; a non-trivial rebase conflict is a STOP-and-report.
2. **Second-export invocation cost** — `migration_check` runs as a separate `module.run` on the v2 module. Confirm the module is already loaded at the check site (it is — `execute_migration` already ran it at `:536`) so this is a second `run`, not a second load. If `module` is consumed by `execute_migration`'s `spawn_blocking` (it is `move`d at `:537`), `run_migration_check` needs its own clone/handle — verify `calimero_runtime::Module` is `Clone`/cheaply re-runnable, or restructure so the module isn't moved away before the check. **This is the most likely real friction point.**
3. **Admin-abort semantics for already-applied contexts** — `abort_migration` flips the *target* back for not-yet-applied (lazy) contexts; it cannot un-migrate a context that already committed v2 (that would be the recall we don't do). Confirm the product expectation is "abort the rollout going forward," not "roll back committed nodes." Stated as a non-goal in the handler doc; flag if the user expects otherwise.
4. **Docker/CI gating** (6d.6) — merobox needs Docker + `merod:local`, and CI needs an explicit user-authorized push (no-auto-push rule). Same constraint that left 6a.5/6a.6 unfinished.
