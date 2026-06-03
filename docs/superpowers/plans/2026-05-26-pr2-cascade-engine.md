# PR-2: Cascade engine (apply handlers + write-gate + e2e workflows) — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Wire the `GroupOp::CascadeTargetApplicationSet` and `GroupOp::CascadeGroupMigrationSet` variants (already on master from #2452 as wire-format-only) into an end-to-end working cascade engine. One signed cascade op fans out to every matching descendant subgroup + context in a single sync round, per the eager walk-and-write model (spec §3.2). Add the local write-gate that refuses writes against contexts in `InProgress` state. Cover with Rust integration tests + three merobox workflows (01 cascade-baseline, 04 heterogeneous-skip, 05 cascade-chain).

**Out of scope (this PR):** HLC sync fence and `get_cascade_status` RPC (PR-3); `force_complete_cascade` and workflow 02 multi-version coexistence (PR-4); offline-straggler workflow 03 and fence-rejection workflow 06 (PR-3); per-context `cascade_hlc` storage field (PR-3).

**Architecture:**

- **Apply handler placement.** Add new arms to `apply_group_op_mutations` in `crates/context/src/group_store/mod.rs` (currently silently no-ops the cascade variants via a catch-all at line 1793). Each arm runs the local tree walk via the existing `collect_descendant_groups`, applies `from_app_key` predicate inline, and for each matched descendant: (a) calls the same per-group settings mutation the non-cascade variants already use (`settings.set_target_application` / `settings.set_group_migration`) and (b) enumerates contexts via `enumerate_group_contexts` and enqueues per-context migration via the existing propagator entry point used by `upgrade_group`.
- **No new propagator.** The cascade apply path reuses the per-context propagator that single-group `upgrade_group` already drives. The cascade variant is "fan-out + status-mark", not a re-implementation of migration. This keeps the cascade engine ~600 LOC by leaning on PR-1's already-fixed `write_pre_merged_root_state` path.
- **Write-gate generalization.** The local execute/call handler (in `crates/context/src/handlers/execute.rs`) refuses writes when `GroupUpgradeStatus(ctx) == InProgress`. The today's per-group `UpgradePolicy::Coordinated` already implements roughly this — we generalize it so it triggers for cascade-set statuses too. The check is a per-context status read, not cascade-specific.
- **Base branch.** PR-2 branches from PR-1's branch `fix/migration-write-pre-merged` (NOT master), because PR-2 depends on PR-1's `write_pre_merged_root_state` switch in `update_application/mod.rs`. When PR-1 merges to master, PR-2 rebases onto master (clean rebase expected — PR-1 touches different lines than PR-2).
- **merobox dependency.** Workflows 01/04/05 invoke either a new `cascade_namespace_application` step or an extended `upgrade_group` step with a `cascade: bool` field (merobox#255 will decide). Until merobox publishes that, PR-2 ships the workflow YAMLs but the CI job skips them via a `merobox >= X.Y.Z` guard. Rust integration tests carry the regression-guard weight inside PR-2 itself.

**Tech Stack:** Rust (calimero-context, calimero-governance, calimero-storage), Borsh, Merobox (Python, docker-based e2e harness), GitHub Actions.

**Spec:** `docs/superpowers/specs/2026-05-26-namespace-cascade-migration-design.md` §3.2 (apply algorithm), §3.5 (local write gate), §5 (concurrent cascade safety), §7 (component layout), §8.1–§8.3 (unit/integration/e2e test matrices), §9.2 (PR-2 contents).

---

## File Structure

**Created:**
- `crates/context/src/cascade/mod.rs` — module entry; re-exports walk + predicate helpers.
- `crates/context/src/cascade/walk.rs` — small helper wrapping `collect_descendant_groups` + the `from_app_key` predicate so the apply handler stays narrow and the unit test surface is direct.
- `crates/context/src/cascade/state_machine.rs` *(small — types + transitions only; no I/O)*.
- `crates/context/tests/cascade_apply_walk.rs` — single-namespace cascade Rust integ test.
- `crates/context/tests/cascade_concurrent_safety.rs` — two cascade ops in non-determined order; predicate-skip on the loser.
- `crates/context/src/cascade/walk_predicate_tests.rs` — `#[cfg(test)]` unit module exercising predicate equality + skip.
- `crates/context/src/cascade/walk_depth_bound_tests.rs` — `#[cfg(test)]` unit module exercising `MAX_NAMESPACE_DEPTH` + cycle-detection.
- `workflows/app-migration/01-single-namespace-cascade.yml` — 2 nodes; namespace with 2 subgroups × 2 contexts; cascade v1→v2; all 4 contexts migrated on both nodes.
- `workflows/app-migration/04-cascade-skip-heterogeneous.yml` — namespace with subgroup A on v1 + B already on v2; cascade v1→v3; only A migrates.
- `workflows/app-migration/05-cascade-chain-v1-to-v3.yml` — 2 nodes; cascade v1→v2 then v2→v3 in DAG order.
- `apps/migrations/migration-suite-v3-add-list/` — new fixture (v2 + a list field) used by workflows 04 and 05; mirror existing `migration-suite-v2-add-field/` layout.

**Modified:**
- `crates/context/src/group_store/mod.rs:1793` — drop the silent `_ => Ok((false, None))` no-op for the cascade variants; route `CascadeTargetApplicationSet` and `CascadeGroupMigrationSet` to dedicated arms that call into `cascade::walk`.
- `crates/context/src/handlers/upgrade_group.rs` — add a `cascade: bool` field (default false) to `UpgradeGroupRequest`; when true, emit `GroupOp::Cascade*` instead of the per-group `TargetApplicationSet` / `GroupMigrationSet`. Use the resolved current `app_key` as `from_app_key`.
- `crates/context/src/handlers/execute.rs` — read `GroupUpgradeStatus(ctx)` before allowing user-initiated writes; bail with `UpgradeInProgress` when `InProgress`. Reads (call without write effects) are still allowed.
- `crates/context/src/lib.rs` — add `pub mod cascade;`.
- `workflows/app-migration/build-wasms.sh` — append `apps/migrations/migration-suite-v3-add-list` to the `SUITES` array.
- `.github/workflows/app-migration-e2e.yml` — add a `merobox >= 0.7.0` (or whatever merobox#255 lands as) version check; skip workflows 01/04/05 below that version with a clear log message. Workflow 00 remains unconditional.

**Out-of-tree (handled in plan tasks, not in PR diff):**
- Coordinate with merobox#255 author/landing to decide step name + version pin.
- After merobox#255 merges + publishes, follow-up commit on PR-2 bumps the version guard and activates the new workflows.

---

## Tasks

### Task 1: Pre-flight — verify PR-1 worktree state + create PR-2 worktree

**Files:** worktree creation only.

- [ ] **Step 1: Confirm PR-1 worktree is at the latest pushed head**

Run:
```bash
cd /Users/beast/Developer/Calimero/core/.worktrees/fix-migration-write-pre-merged
git status
git log --oneline -3
```

Expected: clean tree on `fix/migration-write-pre-merged`. Most recent commit matches the latest pushed head on PR #2477.

- [ ] **Step 2: Create PR-2 worktree branched off PR-1's branch**

Per user instruction: PR-2 bases on PR-1 until PR-1 merges, then rebases onto master.

Use the using-git-worktrees skill. Worktree directory:
- Path: `.worktrees/feat-cascade-engine`
- Branch: `feat/cascade-engine`
- Base: `fix/migration-write-pre-merged` (PR-1's local branch)

Equivalent direct command:
```bash
cd /Users/beast/Developer/Calimero/core
git worktree add -b feat/cascade-engine .worktrees/feat-cascade-engine fix/migration-write-pre-merged
```

Expected: new worktree at `.worktrees/feat-cascade-engine`; HEAD identical to PR-1's branch.

- [ ] **Step 3: Configure work-from directory for subsequent tasks**

All Rust + workflow edits below run from `.worktrees/feat-cascade-engine`. Subagent prompts must specify this path as the working directory.

---

### Task 2: Scaffolding — cascade module + walk helper

**Files:** `crates/context/src/cascade/mod.rs`, `crates/context/src/cascade/walk.rs`, `crates/context/src/lib.rs`.

- [ ] **Step 1: Create `cascade/mod.rs` with module skeleton**

Content:
```rust
mod walk;

pub use walk::{walk_for_predicate, WalkEntry};

#[cfg(test)]
mod walk_predicate_tests;
#[cfg(test)]
mod walk_depth_bound_tests;
```

- [ ] **Step 2: Implement `walk.rs` with `walk_for_predicate`**

Signature:
```rust
pub struct WalkEntry {
    pub group_id: ContextGroupId,
    pub matched: bool, // true if app_key matches from_app_key
}

pub fn walk_for_predicate(
    store: &Store,
    signed_group_id: ContextGroupId,
    from_app_key: [u8; 32],
) -> EyreResult<Vec<WalkEntry>> {
    let mut out = Vec::new();
    let mut group_ids = vec![signed_group_id];
    group_ids.extend(crate::group_store::namespace::core::collect_descendant_groups(
        store, &signed_group_id,
    )?);

    for gid in group_ids {
        let meta = read_group_meta(store, &gid)?;
        out.push(WalkEntry {
            group_id: gid,
            matched: meta.app_key == from_app_key,
        });
    }
    Ok(out)
}
```

Read-only — does NOT mutate state. Reused by both Cascade* apply arms.

- [ ] **Step 3: Register the module in `crates/context/src/lib.rs`**

Add `pub mod cascade;` near the other `pub mod` lines.

- [ ] **Step 4: Sanity build**

```bash
cd /Users/beast/Developer/Calimero/core/.worktrees/feat-cascade-engine
cargo check -p calimero-context
```

Expected: clean compile.

---

### Task 3: Unit tests for walk predicate + depth bound

**Files:** `crates/context/src/cascade/walk_predicate_tests.rs`, `crates/context/src/cascade/walk_depth_bound_tests.rs`.

Use TDD here — write the tests *first*, watch them fail, then implement walk helper guard rails until they pass.

- [ ] **Step 1: `walk_predicate_tests.rs` — predicate equality + signed group inclusion**

Tests:
- `predicate_match_includes_descendant` — fresh namespace with two children all on app_key A; walk with `from_app_key = A` returns 3 entries, all `matched: true`.
- `predicate_mismatch_skips_descendant` — namespace where one child has app_key B; walk with `from_app_key = A` returns the B-child with `matched: false`.
- `walk_includes_signed_group` — the signed_group_id itself is always in the output (it's the namespace root being cascaded).

- [ ] **Step 2: `walk_depth_bound_tests.rs` — depth limit + cycle detection**

The existing `collect_descendant_groups` already handles cycle detection (verified in Explore survey). What we need to test:
- `walk_respects_MAX_NAMESPACE_DEPTH` — namespace nested to a depth-bound stops at the limit without erroring.
- `walk_no_infinite_loop_on_cycle` — synthesized cycle returns Err or terminates cleanly (verify which behaviour `collect_descendant_groups` provides; our wrapper inherits it).

- [ ] **Step 3: Run unit tests**

```bash
cargo test -p calimero-context cascade::
```

Expected: all unit tests green. Output line count for this task: roughly 150 lines test code, 0 LOC in walk.rs beyond Task 2.

---

### Task 4: Apply handler arms for `CascadeTargetApplicationSet`

**Files:** `crates/context/src/group_store/mod.rs` (around line 1793 catch-all).

- [ ] **Step 1: Add the arm**

Replace the catch-all path for `CascadeTargetApplicationSet` with:

```rust
GroupOp::CascadeTargetApplicationSet {
    from_app_key,
    app_key,
    target_application_id,
} => {
    let entries = crate::cascade::walk_for_predicate(
        store, signed_group_id, *from_app_key,
    )?;

    let mut any_applied = false;
    for entry in entries {
        if !entry.matched {
            tracing::debug!(
                group_id = ?entry.group_id,
                "Cascade target-application: skip (app_key mismatch)"
            );
            continue;
        }

        // Reuse the existing single-group mutation; capabilities are already
        // signed at the namespace level, so we re-use the same settings mutation
        // path as the per-group TargetApplicationSet arm.
        settings_for(store, &entry.group_id)?
            .set_target_application(signer, *app_key, *target_application_id)?;

        // Mark every context in this group as InProgress.
        let contexts = enumerate_group_contexts(
            store, &entry.group_id, 0, usize::MAX,
        )?;
        for ctx in contexts {
            mark_context_upgrade_in_progress(store, &ctx)?;
            enqueue_migration_propagator(&ctx, *target_application_id)?;
        }
        any_applied = true;
    }
    Ok((any_applied, None))
}
```

- [ ] **Step 2: Helper functions**

Two thin helpers needed (likely already exist or are one-liner wrappers):
- `mark_context_upgrade_in_progress(store, ctx)` — writes `GroupUpgradeStatus::InProgress { cascade_hlc: None }` for now. The HLC field stays `None` in PR-2; PR-3 fills it in.
- `enqueue_migration_propagator(ctx, target_application_id)` — the per-context propagator entry that `upgrade_group` already drives. Reuse, don't re-implement.

Locate where `upgrade_group.rs` calls these today; lift them into pub(crate) helpers in `group_store/mod.rs` or a sibling module so the cascade arm and the single-group path share one code path.

- [ ] **Step 3: Build check**

```bash
cargo check -p calimero-context
```

Expected: clean compile.

---

### Task 5: Apply handler arm for `CascadeGroupMigrationSet`

**Files:** `crates/context/src/group_store/mod.rs`.

- [ ] **Step 1: Mirror the structure of Task 4 but for migration bytes**

```rust
GroupOp::CascadeGroupMigrationSet {
    from_app_key,
    migration,
} => {
    let entries = crate::cascade::walk_for_predicate(
        store, signed_group_id, *from_app_key,
    )?;

    let mut any_applied = false;
    for entry in entries {
        if !entry.matched {
            continue;
        }
        settings_for(store, &entry.group_id)?
            .set_group_migration(signer, migration.clone())?;
        any_applied = true;
    }
    Ok((any_applied, None))
}
```

This variant only changes the migration symbol; it does NOT mark contexts as InProgress or kick the propagator (those happen on the *next* `CascadeTargetApplicationSet` op that pairs with it). Document this in a code comment.

- [ ] **Step 2: Build check + run existing governance tests**

```bash
cargo test -p calimero-context group_store::
```

Expected: existing single-group tests still green; no new failures.

---

### Task 6: Extend `upgrade_group` RPC to emit cascade ops

**Files:** `crates/context/src/handlers/upgrade_group.rs`.

- [ ] **Step 1: Add `cascade: bool` field to `UpgradeGroupRequest`**

```rust
#[derive(Debug, Clone, BorshDeserialize, BorshSerialize, Serialize, Deserialize)]
pub struct UpgradeGroupRequest {
    pub group_id: ContextGroupId,
    pub target_application_id: ApplicationId,
    pub migrate_method: Option<String>,
    #[serde(default)]
    pub cascade: bool, // new — when true, emits Cascade* ops instead of per-group
}
```

`#[serde(default)]` keeps backward compat for existing clients (e.g. PR-1's workflow 00 doesn't set it).

- [ ] **Step 2: Branch emission on `cascade`**

```rust
let op = if request.cascade {
    let current_app_key = resolve_current_app_key(store, &request.group_id)?;
    let new_app_key = derive_app_key(&request.target_application_id);
    GroupOp::CascadeTargetApplicationSet {
        from_app_key: current_app_key,
        app_key: new_app_key,
        target_application_id: request.target_application_id,
    }
} else {
    GroupOp::TargetApplicationSet {
        app_key: derive_app_key(&request.target_application_id),
        target_application_id: request.target_application_id,
    }
};
```

If `migrate_method` is set: pair the above with a `CascadeGroupMigrationSet` op when `cascade=true`, otherwise the existing `GroupMigrationSet`. Both ops are emitted in the same governance round so they apply atomically per-receiver.

- [ ] **Step 3: Build + run existing upgrade_group tests**

```bash
cargo test -p calimero-context handlers::upgrade_group
```

Expected: existing tests pass (they don't set `cascade`, so default `false` keeps them on the single-group path).

---

### Task 7: Local write-gate in execute handler

**Files:** `crates/context/src/handlers/execute.rs` (or wherever the local execute entry-point lives — confirm during implementation).

- [ ] **Step 1: Pre-flight read of `GroupUpgradeStatus(ctx)`**

```rust
if request.intent_is_write() {  // i.e. not a pure call
    let status = read_group_upgrade_status(store, &context_id)?;
    if matches!(status, Some(GroupUpgradeStatus::InProgress { .. })) {
        bail!(ExecuteError::UpgradeInProgress);
    }
}
```

The `intent_is_write` predicate distinguishes write-mutating WASM calls from pure read calls. If the existing code doesn't have this distinction, conservatively block all `call` invocations during InProgress (we can refine later — better to block too much than too little here).

- [ ] **Step 2: Add `UpgradeInProgress` error variant**

```rust
#[derive(Debug, thiserror::Error)]
pub enum ExecuteError {
    // ... existing variants
    #[error("context upgrade in progress; writes refused until migration completes")]
    UpgradeInProgress,
}
```

- [ ] **Step 3: Build + run execute tests**

```bash
cargo test -p calimero-context handlers::execute
```

---

### Task 8: Rust integration test — `cascade_apply_walk.rs`

**Files:** `crates/context/tests/cascade_apply_walk.rs`.

Scenario: synthetic namespace `R` with descendants `R/A` (3 contexts) + `R/B/B1` (2 contexts). All on app_key `K1`. Emit `CascadeTargetApplicationSet { from_app_key: K1, app_key: K2, target_application_id: App2 }`.

Assertions:
- After apply: every group's `GroupMeta.app_key == K2`, `GroupMeta.target_application_id == App2`.
- Every context in `R`, `R/A`, `R/B`, `R/B/B1` has `GroupUpgradeStatus::InProgress { .. }`.
- The propagator was enqueued for each context (count == 5; verify via the test-instrumented propagator queue, or via assertion that `GroupUpgradeStatus` ultimately flips to `Completed` after running propagator inline).
- A sibling namespace `S` (unrelated to `R`) is untouched.

- [ ] **Step 1: Stand up the synthetic namespace fixture**

Use existing test helpers in `crates/context/tests/common/` if present; otherwise inline the setup. Look at how `cascade_apply_walk.rs` siblings build namespaces — `2026-05-13-opaque-leaf-sync.md` plan likely has reusable patterns.

- [ ] **Step 2: Run the test**

```bash
cargo test -p calimero-context --test cascade_apply_walk
```

Expected: green.

---

### Task 9: Rust integration test — `cascade_concurrent_safety.rs`

**Files:** `crates/context/tests/cascade_concurrent_safety.rs`.

Scenario: namespace on app_key `K1`. Two cascade ops sequenced in different orders on different replicas:
- Op A: `CascadeTargetApplicationSet { from_app_key: K1, app_key: K2, ... }`
- Op B: `CascadeTargetApplicationSet { from_app_key: K1, app_key: K3, ... }`

The DAG picks one as causally-first (e.g. A wins). After both apply:
- Whichever applies first: full walk + state mutation + propagator enqueue.
- Whichever applies second: predicate check `K1 == K1` is FALSE (state already shows K2 or K3); silently skips per spec §5.

Assertion: regardless of DAG ordering, both replicas converge to the same final `app_key` (whichever the DAG ordered first).

- [ ] **Step 1: Drive two replicas with the same governance DAG**

Build the DAG with both ops; replay on replicas A and B in different physical orders. Verify final state.

- [ ] **Step 2: Run**

```bash
cargo test -p calimero-context --test cascade_concurrent_safety
```

---

### Task 10: Migration suite v3 fixture

**Files:** `apps/migrations/migration-suite-v3-add-list/{Cargo.toml,build.sh,src/lib.rs}`.

Mirror `apps/migrations/migration-suite-v2-add-field/`. v3 adds a `list: Vec<String>` field with `migrate_v2_to_v3` populating it from `notes` (or similar deterministic derivation). v3 is used by workflows 04 (skip-heterogeneous: B starts on v2, cascade v1→v3 means B is already at v2 so does NOT match) and 05 (cascade-chain: v1→v2 then v2→v3).

- [ ] **Step 1: Copy `migration-suite-v2-add-field/` as template**

```bash
cp -r apps/migrations/migration-suite-v2-add-field apps/migrations/migration-suite-v3-add-list
```

- [ ] **Step 2: Edit Cargo.toml name + lib.rs schema**

```toml
[package]
name = "migration-suite-v3-add-list"
```

`src/lib.rs`:
- Bump `SCHEMA_VERSION` to `"3.0.0"`.
- Add `pub list: Vec<String>` field to the state struct.
- Add `pub fn migrate_v2_to_v3(...)` that initializes `list = vec![]`.
- Add `pub fn add_to_list(&mut self, item: String)` + `pub fn get_list(&self) -> Vec<String>` for round-trip assertion.

- [ ] **Step 3: Build + verify WASM produced**

```bash
bash apps/migrations/migration-suite-v3-add-list/build.sh
ls -lh apps/migrations/migration-suite-v3-add-list/res/migration_suite_v3_add_list.wasm
```

- [ ] **Step 4: Update `build-wasms.sh` SUITES array**

Append `"apps/migrations/migration-suite-v3-add-list"`.

---

### Task 11: merobox workflow 01 — single-namespace cascade

**Files:** `workflows/app-migration/01-single-namespace-cascade.yml`.

Setup: 2 nodes. Admin on node-1 creates namespace `NS` with `app_v1`. Inside NS: subgroup `G1` (2 contexts) + subgroup `G2` (2 contexts). Wait for node-2 to sync the namespace + groups + contexts. Issue cascade v1→v2 against `NS`. Assert all 4 contexts run `migrate_v1_to_v2` on both nodes and reach v2 schema.

- [ ] **Step 1: Author the YAML**

Skeleton (uses tentative `cascade_namespace_application` step name from merobox#255; adapt when merobox lands):

```yaml
name: 01 — Single-namespace cascade (v1 → v2)
description: >
  2 nodes, namespace with 2 subgroups × 2 contexts. One signed cascade op
  upgrades all 4 contexts to v2 on both nodes. Regression guard for the
  cascade engine's walk + per-context propagator dispatch.

no_docker: false
nuke_on_start: true
nuke_on_end: false

nodes:
  count: 2
  image: merod:local
  prefix: app-migration-namespace-cascade
  base_port: 9221
  base_rpc_port: 9321

steps:
  # ... install_application v1, v2 on both nodes
  # ... create_namespace
  # ... explicit set_member_auto_follow grant (per merobox#250 pattern)
  # ... create_group_in_namespace G1, G2
  # ... create_context × 4 (2 in each group)
  # ... v1 writes + sanity reads
  # ... cascade_namespace_application namespace_id=NS target=app_v2 migrate=migrate_v1_to_v2
  # ... wait 10s
  # ... assert_log_present per node: "Cascade target-application: applied" × 2
  # ... read schema_info on each of 4 contexts on each node; assert v2 schema
  # ... v2-only setter/getter round-trip on at least one context
  # ... teardown
```

- [ ] **Step 2: Adopt #250 auto-follow pattern**

Before creating contexts on a second node, explicitly grant auto-follow. This was the fix in merobox#250's example-project and the same pattern applies here.

- [ ] **Step 3: Document the assertion strategy in YAML comments**

Follow the workflow 00 precedent — log-pattern assertions need ANSI-aware patterns (no `key=value` matches because tracing wraps `=`).

---

### Task 12: merobox workflow 04 — heterogeneous skip

**Files:** `workflows/app-migration/04-cascade-skip-heterogeneous.yml`.

Setup: 1 node. Namespace `NS` contains subgroup `A` (on v1) and subgroup `B` (already on v2). Issue cascade v1→v3 against `NS`. Assert:
- A's contexts migrate to v3.
- B's contexts stay on v2 (predicate `from_app_key == v1` doesn't match B's v2 app_key).
- `get_group_upgrade_status` on B shows unchanged.

- [ ] **Step 1: Author the YAML**

Pre-create B's contexts on v2 directly (skip the cascade-from-v1 step for B), then issue the cascade and verify B is untouched.

- [ ] **Step 2: Note in comments**

This is the regression guard for spec §3.2 predicate-skip semantics. If a future code change accidentally widens the predicate match, B's contexts would corrupt — this workflow catches that immediately.

---

### Task 13: merobox workflow 05 — cascade chain v1 → v2 → v3

**Files:** `workflows/app-migration/05-cascade-chain-v1-to-v3.yml`.

Setup: 2 nodes, 1 context in a single subgroup under namespace. Issue cascade v1→v2; wait for completion. Issue cascade v2→v3; wait for completion. Assert final state is v3 on both nodes and that intermediate v2 state was applied (e.g. `notes` field set by `migrate_v1_to_v2` survives into v3 because `migrate_v2_to_v3` is additive).

- [ ] **Step 1: Author the YAML**

Sequential cascade ops; verify both apply in DAG order.

- [ ] **Step 2: Assertion**

After both cascades:
- `schema_version == "3.0.0"`
- `notes` from v2's migration still present
- `list == []` from v3's migration

---

### Task 14: Wire CI — extend `app-migration-e2e.yml` with version-guarded new workflows

**Files:** `.github/workflows/app-migration-e2e.yml`.

- [ ] **Step 1: Add merobox version check step**

```yaml
- name: Check merobox version supports cascade steps
  id: merobox_version
  run: |
    MEROBOX_VERSION=$(pip show merobox | grep Version | awk '{print $2}')
    REQUIRED="0.7.0"  # update once merobox#255 lands
    if [ "$(printf '%s\n%s\n' "$REQUIRED" "$MEROBOX_VERSION" | sort -V | head -1)" = "$REQUIRED" ]; then
      echo "supports_cascade=true" >> $GITHUB_OUTPUT
    else
      echo "supports_cascade=false" >> $GITHUB_OUTPUT
      echo "::warning::merobox $MEROBOX_VERSION is below $REQUIRED — skipping workflows 01,04,05 (cascade engine PR-2)"
    fi
```

- [ ] **Step 2: Guard the workflow iteration loop**

```yaml
- name: Run cascade workflows (requires merobox >= 0.7.0)
  if: steps.merobox_version.outputs.supports_cascade == 'true'
  run: |
    for wf in 01-single-namespace-cascade.yml 04-cascade-skip-heterogeneous.yml 05-cascade-chain-v1-to-v3.yml; do
      merobox run workflows/app-migration/$wf
    done
```

Workflow 00 (PR-1) runs unconditionally — it doesn't need cascade step support.

- [ ] **Step 3: Note for follow-up**

After merobox#255 merges + publishes, a small follow-up commit on PR-2 bumps the `REQUIRED` value and confirms the activation works. If merobox#255 lands BEFORE PR-2 opens, the guard can be set to the actual published version immediately.

---

### Task 15: Open PR-2 against `fix/migration-write-pre-merged` as base

**Files:** none — git/GH operations.

- [ ] **Step 1: Push branch**

```bash
cd /Users/beast/Developer/Calimero/core/.worktrees/feat-cascade-engine
git push -u origin feat/cascade-engine
```

- [ ] **Step 2: Create PR with PR-1's branch as base**

```bash
gh pr create \
  --base fix/migration-write-pre-merged \
  --head feat/cascade-engine \
  --title "feat(cascade): namespace-level cascade engine — apply handlers + write-gate + e2e workflows" \
  --body "$(cat <<'EOF'
## Summary

PR-2 of the 4-PR namespace-cascade-migration train. Wires the
`CascadeTargetApplicationSet` and `CascadeGroupMigrationSet` GroupOp variants
(merged as wire-format-only in #2452) into a working end-to-end cascade
engine.

One signed cascade op now fans out to every matching descendant subgroup +
context in a single sync round, replacing N separate `upgrade_group` RPCs.

## Architecture

- **Apply handlers** in `crates/context/src/group_store/mod.rs` walk the
  descendant tree via the existing `collect_descendant_groups`, apply the
  `from_app_key` predicate inline, reuse the single-group settings mutations
  for each match, and enqueue per-context migration via the existing
  propagator (no new propagator).
- **Local write-gate** in `crates/context/src/handlers/execute.rs` refuses
  writes when `GroupUpgradeStatus(ctx) == InProgress`. Generalizes today's
  per-group `UpgradePolicy::Coordinated` to trigger for cascade-set statuses.
- **Predicate-based heterogeneous skip:** descendants whose current
  `app_key != from_app_key` are silently skipped per spec §3.2.
- **Concurrent-cascade safety** via the predicate as optimistic-concurrency
  control (spec §5): a loser cascade arrives with stale `from_app_key`, the
  predicate-mismatch turns it into a no-op without locking.

## Train state

- PR-1 (#2477): MERGED / OPEN — base of this PR.
- PR-2 (this PR): cascade engine + 3 workflows + Rust integ tests.
- PR-3 (next): HLC fence + `get_cascade_status` RPC + workflow 03 (offline-
  straggler) + workflow 06 (fence-rejection, stretch).
- PR-4 (optional): `force_complete_cascade` + workflow 02 (multi-version
  coexistence).

## Out of scope this PR

- HLC sync fence (PR-3).
- `get_cascade_status` RPC (PR-3).
- Per-context `cascade_hlc` storage field (PR-3).
- `force_complete_cascade` admin RPC (PR-4).
- Workflow 02 (multi-version coexistence) — PR-4.
- Workflow 03 (offline-straggler) — PR-3, will use merobox#249 fault-injection
  (pause/restart) to model the offline straggler.
- Workflow 06 (fence-rejection stretch) — PR-3.

## Test plan

Rust integration tests (run unconditionally in CI):
- [ ] `cascade_apply_walk` — single op → all matched descendants updated, sibling untouched.
- [ ] `cascade_concurrent_safety` — two cascade ops in non-determined order; predicate skip on loser; replica convergence.

merobox e2e workflows (run when merobox >= 0.7.0):
- [ ] `01-single-namespace-cascade.yml` — 2 nodes, 4 contexts, cascade v1→v2.
- [ ] `04-cascade-skip-heterogeneous.yml` — predicate skip on already-v2 subgroup.
- [ ] `05-cascade-chain-v1-to-v3.yml` — sequential v1→v2 then v2→v3.

🤖 Generated with [Claude Code](https://claude.com/claude-code)
EOF
)"
```

- [ ] **Step 3: Confirm PR base + label**

```bash
gh pr view --json baseRefName,labels
```

Expected `baseRefName: fix/migration-write-pre-merged`. Tag `enhancement` + `cascade-migration`.

- [ ] **Step 4: When PR-1 (#2477) merges to master, rebase PR-2 onto master**

```bash
cd /Users/beast/Developer/Calimero/core/.worktrees/feat-cascade-engine
git fetch origin master
git rebase origin/master
git push --force-with-lease
gh pr edit --base master
```

Expected: clean rebase (PR-1's changes touched different lines). If conflicts arise in `update_application/mod.rs`, resolve preserving PR-1's `write_pre_merged_root_state` shape.

---

### Task 16: CI verification + bug-bot triage

**Files:** none — CI watching + iteration.

- [ ] **Step 1: Wait for CI green on PR-2**

Rust tests, format check, clippy, app-migration-e2e (which runs workflow 00 unconditionally).

- [ ] **Step 2: Triage bug-bot comments**

Same policy as PR-1: address only CRITICAL or HIGH severity findings before merge. Document deferred lower-severity findings inline in PR comments with rationale (TOCTOU windows, naming nits, etc.).

- [ ] **Step 3: Resolve addressed threads via GraphQL `resolveReviewThread` mutation**

Pattern from PR-1: GraphQL `resolveReviewThread` once a fix lands or rationale is posted.

- [ ] **Step 4: When ready, request human review**

Mark ready-for-review (it should already be open ready, per the no-draft rule). Wait for `reviewDecision: APPROVED` before merge.

---

## Acceptance Criteria

PR-2 is mergeable when:

1. ✅ Rust integration tests `cascade_apply_walk` + `cascade_concurrent_safety` pass in CI.
2. ✅ Unit tests under `cascade::walk_predicate_tests` + `cascade::walk_depth_bound_tests` pass.
3. ✅ All existing `calimero-context` tests still pass (no regression).
4. ✅ Workflow 00 (PR-1's regression guard) still passes — cascade engine doesn't break single-group migration.
5. ✅ Workflows 01/04/05 pass when run locally against a merobox build that supports the cascade step. CI may report them as skipped until merobox is bumped — this is acceptable per the version-guard design.
6. ✅ All CRITICAL/HIGH bug-bot findings addressed or documented as deferred.
7. ✅ PR description matches the standing rules: no spec doc reference (lives in `docs/superpowers/specs/` on main only), single chosen approach described (no A/B alternatives), opened ready-for-review.
8. ✅ Base branch is `master` at merge time (rebased from `fix/migration-write-pre-merged` once PR-1 merges).

---

## Risks & mitigations

| Risk | Mitigation |
|------|------------|
| Cascade variant arms duplicate single-group settings mutation logic, leading to divergence over time | Reuse existing `settings.set_target_application` / `settings.set_group_migration` directly — don't fork the mutation path. |
| Per-context propagator was designed for one-context-at-a-time use; enqueueing N at once stresses it | If N > 100 contexts in tests reveals queue contention, fall back to bounded concurrency (semaphore). Defer that knob to PR-3 if not hit in workflow tests. |
| `cascade: bool` on `UpgradeGroupRequest` is a breaking wire change for downstream SDK clients | Field is `#[serde(default)]`, so older clients submitting without it default to `false` (single-group path) — fully backward compatible. |
| Workflow 04 (skip-heterogeneous) requires pre-existing v2 subgroup; the setup ceremony is brittle | Use a deterministic setup path: install both v1+v2 on the node, create B's group with `application_id = v2` at creation time. Don't try to migrate B from v1 to v2 first. |
| Local write-gate in `execute.rs` blocks legitimate non-write calls if the intent classification is too coarse | Conservative initial behavior: block all WASM calls (including reads) when `InProgress`. PR-3 can refine to write-vs-read distinction if usability complaints surface. Track as known limitation in PR description. |
| merobox#255 isn't merged before PR-2 wants to land | Version-guard the new workflow activation in CI (Task 14); PR-2 ships with workflows-as-data + Rust-tests-as-regression-guard. Activation flips to live in a follow-up once merobox publishes. |

---

## What this PR does NOT cover (forward references)

- **PR-3 needs:** per-context `cascade_hlc` field added to the upgrade-status record (storage migration); HLC fence in `state_delta/mod.rs::apply_authorized_state_delta`; `get_cascade_status` RPC; workflow 03 (uses merobox#249 `pause`/`restart` for offline straggler scenario); workflow 06 stretch.
- **PR-4 needs:** `force_complete_cascade --evict-peer X` admin RPC; workflow 02 (multi-version coexistence in one node); cascade tombstone for evicted peers.

These are intentionally deferred — PR-2's scope is the engine that makes cascade work in the happy multi-node case + predicate-skip safety + concurrent safety.
