# PR-6 Hybrid Zero-Downtime Migration — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make application migrations near-zero-downtime (reads always available; writes paused only briefly per-context, not cluster-wide), straggler-safe, identity-gated-aware, and observable — without quorum/voting.

**Architecture:** Hybrid. Convergent/Replayable data keeps the existing deterministic whole-root rebuild (every node re-derives a byte-identical v2 root; clean rollback because v1 is untouched until commit), made non-freezing by removing the group-wide `InProgress` write-gate for migration cascades and relying on absorb-don't-drop. Identity-gated (signed) data is migrated by per-entry, owner-online signed re-write. Shipped as a stacked PR train **6a→6b→6c→6d** behind a `migration_v2` feature flag.

**Tech Stack:** Rust workspace (`crates/{context,storage,store,node,governance-store,governance-types,sdk,server}`), borsh, RocksDB columns, WASM runtime (wasmtime), merobox e2e (`workflows/app-migration/`).

**Spec:** `docs/superpowers/specs/2026-06-03-pr6-expand-contract-design.md` (read it first).

---

## Train overview & dependencies

| PR | Delivers | Depends on | Flag-gated |
|----|----------|-----------|-----------|
| **6a** | Non-freezing whole-root migrate (Convergent/Replayable) + `migration_v2` flag | — | yes (default off) |
| **6b** | Absorb-don't-drop stragglers (durable buffer, replay original bytes, sync-path coverage, binary-version fence) | 6a | yes |
| **6c** | Identity-gated owner-driven re-write + completion visibility (`get_migration_status`) | 6a, 6b | yes |
| **6d** | Soak + `migration_check` + logical abort + admin abort RPC | 6a | yes |

**Flag flip rule:** `migration_v2` may only be turned **on** in a deployment once **6a AND 6b** have landed (6a removes the freeze; 6b is the safety net that makes no-freeze safe). 6c/6d extend capability behind the same flag.

**Confirmed current behavior (the thing 6a changes):** `crates/context/src/handlers/execute/mod.rs:115-174` consults `upgrade_blocks_write` (`:2170-2175`), which returns `true` only for `GroupUpgradeStatus::InProgress`. Single-group `LazyOnAccess` upgrades write `Completed` directly (no block; lazy per-context migration via `maybe_lazy_upgrade` `:2192`). A **namespace cascade** sets `InProgress` group-wide during propagation → that is the cluster-wide write-freeze 6a removes.

---

## File structure (what changes, by responsibility)

- **Feature flag**: `crates/context/...` config surface — a single `migration_v2: bool` read where the write-gate and migrate paths branch. (Confirm the existing node/context config struct during Task 6a.1; wire one bool, default `false`.)
- **6a write-gate**: `crates/context/src/handlers/execute/mod.rs` (the `block_writes_for_group` branch + `upgrade_blocks_write`), `crates/context/src/handlers/upgrade_group.rs` (cascade `InProgress` emission).
- **6b absorb**: `crates/store/src/db.rs` (new `Column::AbsorbBuffer`), new `crates/governance-store/src/absorb.rs` (repository + recovery scan), `crates/context/src/hlc_fence.rs`, `crates/node/src/handlers/state_delta/mod.rs`, `crates/node/src/sync/{helpers.rs, hash_comparison*.rs, level_sync.rs, snapshot.rs}`, loaded-binary-version plumbing.
- **6c identity-gated + visibility**: `crates/storage/src/{entities.rs, interface.rs}`, `crates/governance-types/src/{lib.rs, wire.rs}`, `crates/governance-store/*`, `crates/node/src/readiness.rs`-style TTL cache, new `crates/context/src/handlers/get_migration_status.rs`, `crates/server/src/admin/service.rs`, `crates/sdk/macros`.
- **6d soak/check/abort**: `crates/sdk/macros` + `crates/sdk/src` (`migration_check` export), `crates/context/src/handlers/{update_application/mod.rs, upgrade_group.rs}` (soak + logical abort + admin abort RPC).
- **Tests**: `workflows/app-migration/23..` (new merobox scenarios) + `apps/migrations/*` fixtures + Rust unit/integration per crate.

---

## PR-6a — Non-freezing whole-root migrate

**Outcome:** Behind `migration_v2`, a namespace cascade migration no longer blocks writes group-wide; each context migrates lazily/briefly. Convergent/Replayable convergence and the existing clean-rollback are preserved. Default-off, so master behavior is unchanged until the flag flips (after 6b).

> **STATUS (2026-06-04), branch `feat/2539-pr6a-migration-v2` off `f25cab38`:**
> - **6a.1 DONE** — `77720a02` `migration_v2: bool` on `ContextManagerConfig` (default false), threaded to execute handler.
> - **6a.2 DONE** — `fc721ac6` characterization test (InProgress blocks writes today).
> - **6a.3 DONE** — `60b87bd8` `should_block(migration_v2, status) = !migration_v2 && upgrade_blocks_write(status)`; gate swapped. **Full `calimero-context` lib suite: 87 passed, 0 failed.**
> - **6a.4 DROPPED from this PR (scope decision):** 6a.3 already delivers writes-available; the leftover eager+lazy interaction under the flag is serialized + idempotent (context lock + per-context `MigrationsRepository` marker), so it's safe. Switching cascade emission to lazy/`Completed`-direct is invasive and belongs with the completion/status model in **6c**. Documented here so it isn't lost.
> - **6a.5 / 6a.6 are CI/Docker-gated** (merobox needs Docker + a `merod:local` image; CI needs a push, which requires explicit user authorization per the no-auto-push rule). 6a.5 also needs `migration_v2` exposed on merod's *external* config (`ContextManagerConfig` is constructed programmatically, not serde-loaded) so a merobox node can flip it — small extra plumbing. These are the remaining 6a work, blocked on a push decision / config-exposure.

### Task 6a.1: Add the `migration_v2` feature flag (default off)

**Files:**
- Modify: the node/context config struct (locate via `rg "pub struct .*Config" crates/context crates/node crates/config`) — add `pub migration_v2: bool` with `#[serde(default)]`.
- Modify: the `ContextManager`/execute handler struct to carry the resolved bool.
- Test: `crates/context/src/handlers/execute/mod.rs` unit test module.

- [ ] **Step 1 — Write the failing test:** assert the flag defaults to `false` and is threaded to the execute handler.
```rust
#[test]
fn migration_v2_flag_defaults_off() {
    let cfg = NodeConfig::default();        // adjust to the real config type found in Step 0
    assert!(!cfg.migration_v2, "migration_v2 must default off so master behavior is unchanged");
}
```
- [ ] **Step 2 — Run it, expect FAIL** (`cargo test -p calimero-context migration_v2_flag_defaults_off`) with "no field `migration_v2`".
- [ ] **Step 3 — Implement:** add the `#[serde(default)] pub migration_v2: bool` field; thread it into the handler struct from config.
- [ ] **Step 4 — Run it, expect PASS.**
- [ ] **Step 5 — Commit:** `feat(context): add migration_v2 feature flag (default off)`.

### Task 6a.2: Characterize today's group-wide freeze (regression guard)

**Files:**
- Test: new `crates/context/src/handlers/execute/tests/upgrade_gate.rs` (or inline module).

- [ ] **Step 1 — Write a characterization test** asserting current behavior with the flag **off**: when `upgrade_blocks_write(&InProgress{..})` is `true`, a state-op write is refused (`ExecuteError::UpgradeInProgress`). This locks the existing contract so 6a only changes flag-on behavior.
```rust
#[test]
fn flag_off_inprogress_blocks_state_op_write() {
    use calimero_store::key::GroupUpgradeStatus::InProgress;
    assert!(super::upgrade_blocks_write(&InProgress { total: 1, completed: 0, failed: 0 }));
}
```
- [ ] **Step 2 — Run, expect PASS** (documents current truth).
- [ ] **Step 3 — Commit:** `test(context): characterize InProgress write-gate before 6a`.

### Task 6a.3: Gate the freeze behind the flag

**Files:**
- Modify: `crates/context/src/handlers/execute/mod.rs:122-147` (the `block_writes_for_group` branch).

- [ ] **Step 1 — Write the failing test:** with `migration_v2 = true`, an `InProgress` cascade does **not** set `block_writes_for_group` for a non-state-op user call (writes proceed; per-context lazy migration handles staleness; 6b will absorb any straggler). With the flag off, behavior is unchanged (Task 6a.2 still passes).
```rust
#[test]
fn flag_on_inprogress_does_not_block_user_write() {
    // table-style: (migration_v2, status) -> expected block_writes
    assert_eq!(should_block(true,  &inprogress()), false);
    assert_eq!(should_block(false, &inprogress()), true);
}
```
- [ ] **Step 2 — Run, expect FAIL** ("function `should_block` not found").
- [ ] **Step 3 — Implement:** extract a pure `fn should_block(migration_v2: bool, status) -> bool` = `!migration_v2 && upgrade_blocks_write(status)`; call it in the gate branch; keep state-op refusal only when blocking.
- [ ] **Step 4 — Run, expect PASS** (both flag states).
- [ ] **Step 5 — Commit:** `feat(context): gate cascade write-freeze behind migration_v2`.

### Task 6a.4: Stop emitting group-wide `InProgress` for migration cascades (flag-on)

**Files:**
- Modify: `crates/context/src/handlers/upgrade_group.rs` (cascade `InProgress` status write, ~`:263-284`).

- [ ] **Step 1 — Write the failing test:** with the flag on, a migration-carrying cascade records the per-context lazy markers and an immediate `Completed`-style status (no `InProgress` window), matching the single-group LazyOnAccess path; with the flag off, it still writes `InProgress`.
- [ ] **Step 2 — Run, expect FAIL.**
- [ ] **Step 3 — Implement:** branch the status write on `migration_v2`; flag-on uses the LazyOnAccess `Completed`-direct path for migration cascades (descendants are already required to be LazyOnAccess by the policy gate).
- [ ] **Step 4 — Run, expect PASS.**
- [ ] **Step 5 — Commit:** `feat(context): migration cascade skips group-wide InProgress under migration_v2`.

### Task 6a.5: e2e — reads AND writes available during a namespace migration

**Files:**
- Create: `workflows/app-migration/23-writes-available-during-migration.yml` (model on `21-reads-available-during-upgrade.yml`).
- Modify: `.github/workflows/app-migration-e2e.yml` matrix (+ `23-...`), `workflows/app-migration/build-wasms.sh` if a new fixture is needed.

- [ ] **Step 1 — Write the workflow:** 2-node namespace cascade migration with the node started with `migration_v2=true`; during the migration window issue BOTH a read and a write to a sibling context; assert both succeed (no `UpgradeInProgress`), and post-migration state converges across both nodes. Add `assert_log_absent "refusing state-op execute: group upgrade in progress"`.
- [ ] **Step 2 — Run locally** (`merobox bootstrap run workflows/app-migration/23-...yml --image merod:local --e2e-mode`), expect PASS.
- [ ] **Step 3 — Commit:** `test(e2e): writes available during migration under migration_v2 (scenario 23)`.

### Task 6a.6: Verify existing migration matrix stays green flag-off

- [ ] **Step 1 — Run** the full `app-migration` suite locally (00–22) with default config; expect unchanged PASS (flag off ⇒ no behavior change).
- [ ] **Step 2 — Commit (if any fixups):** `test: confirm app-migration 00-22 green with migration_v2 off`.

---

## PR-6b — Absorb-don't-drop (straggler safety) [scoped; detailed JIT after 6a lands]

**Outcome:** No straggler delta is silently dropped. A node offline across the window (including one still on the v1 binary) is absorbed on reconnect by buffering the **original signed bytes** and replaying them verbatim; coverage spans gossip AND the sync-repair paths; the fence keys on the node's loaded binary version.

**Key interfaces to introduce:**
- `Column::AbsorbBuffer` in `crates/store/src/db.rs`; key `context(32)‖producing_app_key(32)‖delta_id(32)` → serialized `BufferedDelta` (already carries every replay field: `crates/node/primitives/src/delta_buffer.rs:88-141`).
- `crates/governance-store/src/absorb.rs`: `AbsorbRepository { save, load, delete, enumerate_pending(context) }` (mirror `UpgradesRepository::enumerate_in_progress`, `crates/governance-store/src/upgrades.rs:50-67`) + startup recovery scan.
- `loaded_reader_version(context) -> u32/app_key`: the node's actually-loaded binary version (NOT `GroupMeta.app_key`), threaded into `hlc_fence::delta_is_fenced` and the sync-apply gate.

**Tasks (each TDD: failing test → impl → e2e):**
- [ ] **6b.1** New `Column::AbsorbBuffer` + `AbsorbRepository` save/load/delete/enumerate + restart-recovery scan (unit tests on the repo).
- [ ] **6b.2** At the gossip fence (`crates/node/src/handlers/state_delta/mod.rs:654-678`), replace `return Ok(())` (drop) with **persist `BufferedDelta` to AbsorbBuffer** for the migration-relevant fence case; keep the metric (add `absorbed_for_migration`). Non-migration fences still drop.
- [ ] **6b.3** On app_key/binary advance, drain the buffer: **re-feed original signed bytes** through `__calimero_sync_next` (the existing `crates/node/src/delta_store.rs:373-380` path) — NO translation (preserves signatures); delete on success; idempotent via the `delta_id` key.
- [ ] **6b.4** Binary-version fence: thread `loaded_reader_version` so a node lacking a reader for an incoming schema **buffers** instead of writing. Unit test the `should_fence` extension.
- [ ] **6b.5** Cover the sync-repair paths (`crates/node/src/sync/helpers.rs:227-358` + `hash_comparison*/level_sync/snapshot`): carry a per-leaf schema/version; a receiver lacking a reader **declines+buffers** the leaf instead of `apply_leaf_with_crdt_merge` storing unreadable bytes.
- [ ] **6b.6** e2e `24-straggler-absorbed.yml`: node offline across the whole migration window reconnects → its v1 writes are absorbed (not lost) and converge. `25-v1binary-not-corrupted.yml`: a v1-binary node syncing with v2 nodes buffers v2 leaves (assert no deserialization error, no corruption).

**Open item O2/O3** resolved here: replay-verbatim-then-deterministic-refold; loaded-binary-version threading.

---

## PR-6c — Identity-gated owner-driven re-write + completion visibility [scoped; detailed JIT]

**Outcome:** Signed data migrates via owner-online signed re-write; departed owners resolved by admin tombstone+rekey; admins can see per-node completion via `get_migration_status`.

**Key interfaces:**
- `Metadata.schema_version: Option<u32>` (`crates/storage/src/entities.rs:486`) — Merkle-invisible (NOT in `own_hash`, `interface.rs:2478`). Identity-gated entries only.
- Owner-driven convert in `apply_action`/`save_raw` (`crates/storage/src/interface.rs:821, 2975`): when an identity-gated entry's stored `schema_version < target` AND the writer is the owner, run the per-type migrate and re-sign with a **strictly-monotonic nonce** (`updated_at` advances; NOT merge-mode). Per-entry dispatch keyed `(crdt_type/field_name, schema_version)`.
- Force-carry GroupOp variant (`crates/governance-types/src/lib.rs`): governance-authorized **tombstone old User/Shared entry + create new entity under admin key** (verifies normally; sidesteps "can't change owner / can't forge sig").
- Residue = **local derived scan** of un-converted identity-gated entries (no replicated counter).
- `NamespaceTopicMsg::MigrationHeartbeat(SignedMigrationHeartbeat)` (`crates/governance-types/src/wire.rs`, model on `ReadinessBeacon`) → in-memory TTL cache (model on `crates/node/src/readiness.rs`). Signed `{node, version, residue_auto, residue_identity, synced_up_to_hlc}`.
- `get_migration_status(namespace)` handler + admin HTTP route (mirror `crates/server/src/admin/service.rs:231`): `expected_members` = inherited closure (`list ∪ enumerate_inherited` across `collect_descendants`, reusing #2371), cohort **pinned at expand-entry governance HLC**; `all_migrated` true ⟺ every pinned-cohort member reported v2+residue0; `unknown` for stale/never-reported.

**Tasks (TDD):**
- [ ] **6c.1** `Metadata.schema_version` field + Merkle-invisibility unit test (own_hash unchanged when only schema_version differs).
- [ ] **6c.2** Owner-driven convert path (monotonic nonce; only owner; per-entry dispatch) — unit tests incl. "non-owner cannot convert" and "convert replicates as normal signed Update".
- [ ] **6c.3** Force-carry GroupOp (tombstone+rekey under admin key) + apply handler + governance authorization; unit test it verifies under the existing apply-time owner check.
- [ ] **6c.4** Residue local-scan + a two-node test: both nodes converting the same entity → residue decreases by exactly 1 (idempotent), never 2.
- [ ] **6c.5** `MigrationHeartbeat` wire variant + signed body + TTL cache ingest (sig + membership verified on receive).
- [ ] **6c.6** `get_migration_status` rollup (inherited closure, cohort pinning, `unknown` staleness) + admin route.
- [ ] **6c.7** `#[derive(Migrate)]` ergonomics for identity-gated author surface (old-reader + register hook + version bump; NO L3 compile-time lint — rail stays L1 #2645 + L2 #2586).
- [ ] **6c.8** e2e `26-owner-driven-authored.yml`, `27-departed-owner-forcecarry.yml`, `28-get-migration-status.yml` (assert per-node states + `unknown` + cohort pinning).

**Open item O4** resolved here.

---

## PR-6d — Soak + migration_check + logical abort [scoped; detailed JIT]

**Outcome:** A produced v2 root is health-checked before commit; a failing check logically aborts (no byte restore needed — the whole-root path never mutated v1); admins can abort manually.

**Key interfaces:**
- App-exported `migration_check(old_root, new_root) -> bool` (sdk macro export) + built-in helpers (entity-count parity, no-orphaned-refs, conservation).
- In `update_application_with_migration` (`crates/context/src/handlers/update_application/mod.rs:394-501`): run `migration_check` on the produced v2 root **before** `write_migration_state` (`:756`); pass → commit; fail → discard produced root + flip target back (logical abort) + surface error/metric.
- Admin abort RPC (mirror existing admin handlers) → flips the context/group schema target back.

**Tasks (TDD):**
- [ ] **6d.1** `migration_check` sdk export + built-in helper library + unit tests.
- [ ] **6d.2** Wire the pre-commit check + logical abort into the migrate flow; unit test "failed check ⇒ v1 root unchanged, no commit".
- [ ] **6d.3** Admin abort RPC + route.
- [ ] **6d.4** e2e `29-migration-check-pass.yml` (commit) and `30-migration-check-fail-abort.yml` (logical abort, v1 preserved, app still serves), `31-admin-abort.yml`.

---

## Self-review

**Spec coverage:** §2 hybrid → 6a (convergent whole-root) + 6c (identity-gated). §5 6a→6a; §5 6b→6b; §5 6c→6c; §5 6d→6d. §7 invariants → tests in 6a.5/6b.6/6c.4/6d.2. §8 visibility → 6c.5/6c.6. §3 corrections (logical abort, absorb-original-bytes, force-carry tombstone+rekey, residue local, fence-on-binary-version, heartbeat TTL, stacked train) → each has a task. §10 deferred (hard contract, full canary, per-entity convergent) → explicitly out of scope. Open items O1 (decided: brief pause)/O2/O3/O4 → mapped to 6a/6b/6c tasks.

**Placeholder scan:** 6a is fully TDD-detailed. 6b/6c/6d are intentionally scoped-with-interfaces and explicitly marked **detailed JIT** — each predecessor's primitives (e.g., the loaded-binary-version plumbing, the schema_version tag) must exist before their dependents' exact code can be written, and O2/O3/O4 resolve during their phase. This is a deliberate train-sequencing choice, not a missing-detail gap; do NOT expand 6b/6c/6d to line-level code until 6a (resp. 6b) has landed.

**Type consistency:** flag name `migration_v2` used throughout; `should_block(migration_v2, status)` defined in 6a.3 and referenced in 6a tests; `AbsorbRepository`/`Column::AbsorbBuffer` consistent across 6b; `schema_version`/`get_migration_status`/`migration_check` names consistent with the spec.

---

## Execution handoff

Recommended: **subagent-driven execution**, one task per fresh subagent with two-stage review, **starting with PR-6a** (fully detailed). When 6a lands and is flag-verified, author the line-level 6b plan (its primitives now exist), then 6c, then 6d — JIT, each off the prior PR's branch per the standing test-placement rule.
