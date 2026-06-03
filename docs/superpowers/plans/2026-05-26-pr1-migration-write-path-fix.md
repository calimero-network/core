# PR-1: Migration write-path fix (closes #2433 regression) — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fix the per-context migration write path that has been silently broken since #2433 changed `Root<T>` merge semantics. Replace the failing `Interface::save_raw` call site in `write_migration_state` with `Interface::write_pre_merged_root_state` (introduced by #2465), and ship an e2e merobox workflow as the regression guard so future merge-layer changes can't silently break migration again.

**Architecture:** One-line code change in `crates/context/src/handlers/update_application/mod.rs::write_migration_state` switching the storage primitive. Surrounding result-unwrapping simplifies because the new primitive returns `[u8; 32]` directly (no `Option`). Test-bed is a single-node single-group merobox workflow (`workflows/app-migration/00-single-group-migration-baseline.yml`) that installs `migration-suite-v1`, writes state, installs `migration-suite-v2-add-field`, runs `upgrade_group` with `migrate_v1_to_v2`, and asserts the `Migrated` event + v2 schema. New GHA job `app-migration-e2e.yml` runs the workflow on every PR touching the relevant paths.

**Tech Stack:** Rust (calimero-context, calimero-storage), Borsh, Merobox (Python, docker-based e2e harness), GitHub Actions.

**Spec:** `docs/superpowers/specs/2026-05-26-namespace-cascade-migration-design.md` §3.4 (migration write fix), §7 (component layout, file `update_application/mod.rs:705`), §8.3 (workflow 00), §9.2 (PR-1 contents).

---

## File Structure

**Created:**
- `workflows/app-migration/00-single-group-migration-baseline.yml` — e2e workflow, single-node v1→v2 migration. Acts as the regression guard.
- `workflows/app-migration/build-wasms.sh` — convenience wrapper that builds `migration-suite-v{1,2-add-field}` fixtures (extensible to v3/v4/v5 in later PRs).
- `workflows/app-migration/README.md` — explains the directory's purpose, the fixtures, how to run locally.
- `.github/workflows/app-migration-e2e.yml` — CI job; builds merod:local + fixtures, runs every workflow under `workflows/app-migration/`, captures docker logs as artefact.

**Modified:**
- `crates/context/src/handlers/update_application/mod.rs:705-792` — replace `save_raw` with `write_pre_merged_root_state`; drop the now-unreachable `Ok(None)` branch.

**Out-of-tree cleanup before starting:**
- Close PR #2449 (draft, superseded by this PR train).
- Remove `.worktrees/feat-app-migration-coverage` worktree and `feat/app-migration-coverage` branch.
- Remove `.worktrees/feat-migration-overwrite-intent` worktree and `feat/migration-overwrite-intent` branch.

---

## Tasks

### Task 1: Pre-flight cleanup (close superseded PR + stale worktrees)

**Files:** none (out-of-tree git/GH operations only)

- [ ] **Step 1: Close PR #2449 with explanatory comment**

Run:
```bash
gh pr close 2449 --comment "Superseded by the new namespace-cascade-migration PR train (see docs/superpowers/specs/2026-05-26-namespace-cascade-migration-design.md). Workflows re-derived in PR-1 (\`fix/migration-write-pre-merged\`) to cover the actual write-path regression (#2433) and in PR-2 to cover cascade-aware testing. The GHA job structure and build helper are salvaged into PR-1."
```

Expected: PR closed; comment posted.

- [ ] **Step 2: Remove stale app-migration-coverage worktree**

Run:
```bash
git worktree remove .worktrees/feat-app-migration-coverage --force
git branch -D feat/app-migration-coverage
```

Expected: worktree gone; branch deleted. (`--force` because the worktree may have uncommitted local changes from prior sessions.)

- [ ] **Step 3: Remove stale migration-overwrite-intent worktree**

Run:
```bash
git worktree remove .worktrees/feat-migration-overwrite-intent --force
git branch -D feat/migration-overwrite-intent
```

Expected: worktree gone; branch deleted.

- [ ] **Step 4: Confirm clean worktree state**

Run:
```bash
git worktree list
```

Expected output should NOT contain `feat-app-migration-coverage` or `feat-migration-overwrite-intent`. The unrelated worktrees (`fix-cors-expose-auth-error`, `issue-2308-crdt-traits`, etc.) stay.

---

### Task 2: Create the PR-1 worktree

**Files:** worktree creation only.

- [ ] **Step 1: Fetch latest master**

Run:
```bash
git fetch origin master
```

Expected: fetch summary. The cleanup of stale branches above ensures no conflicts.

- [ ] **Step 2: Create worktree branched off origin/master**

This plan assumes the using-git-worktrees skill is invoked at execution time. The worktree directory should be:
- Path: `.worktrees/fix-migration-write-pre-merged`
- Branch: `fix/migration-write-pre-merged`
- Base: `origin/master`

Equivalent direct command:
```bash
git worktree add -b fix/migration-write-pre-merged .worktrees/fix-migration-write-pre-merged origin/master
```

Expected: worktree created at `.worktrees/fix-migration-write-pre-merged/`; new branch tracks `origin/master`.

- [ ] **Step 3: Cd into worktree and verify base**

Run:
```bash
cd .worktrees/fix-migration-write-pre-merged && git log --oneline -1
```

Expected: shows the current `origin/master` HEAD commit. All subsequent task steps run inside this worktree.

---

### Task 3: Add workflows directory scaffolding (helpers, README, GHA job)

**Files:**
- Create: `workflows/app-migration/build-wasms.sh`
- Create: `workflows/app-migration/README.md`
- Create: `.github/workflows/app-migration-e2e.yml`

These three are written first (no e2e workflow YAML yet) so the CI infrastructure is in place before we add the failing workflow that proves the regression.

- [ ] **Step 1: Create the build helper script**

Create `workflows/app-migration/build-wasms.sh` with these exact contents:

```bash
#!/usr/bin/env bash
# Convenience wrapper that builds every migration-suite WASM fixture
# used by the workflows in this directory. Each suite has its own
# build.sh; this script just invokes them in order so CI and local
# devs have one entry-point.
#
# Add new suites here as later PRs introduce them.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
cd "$REPO_ROOT"

SUITES=(
    "apps/migrations/migration-suite-v1"
    "apps/migrations/migration-suite-v2-add-field"
)

for suite in "${SUITES[@]}"; do
    if [ ! -d "$suite" ]; then
        echo "ERROR: $suite not found" >&2
        exit 1
    fi
    if [ ! -x "$suite/build.sh" ]; then
        echo "ERROR: $suite/build.sh missing or not executable" >&2
        exit 1
    fi
    echo ">>> Building $suite"
    bash "$suite/build.sh"
done

echo ">>> All migration-suite fixtures built."
```

- [ ] **Step 2: Make the build helper executable**

Run:
```bash
chmod +x workflows/app-migration/build-wasms.sh
```

Expected: file permissions allow direct execution.

- [ ] **Step 3: Create the README**

Create `workflows/app-migration/README.md` with these exact contents:

````markdown
# `workflows/app-migration/` — application migration e2e

End-to-end merobox workflows that exercise the per-context application
migration pipeline introduced in [#1911](https://github.com/calimero-network/core/pull/1911)
and the namespace-cascade additions designed in
`docs/superpowers/specs/2026-05-26-namespace-cascade-migration-design.md`.

## Workflows in this directory

| File | What it proves |
|---|---|
| `00-single-group-migration-baseline.yml` | Single-node, single-group `v1 → v2` migration via `upgrade_group(cascade=false)`. **Regression guard for [#2433](https://github.com/calimero-network/core/pull/2433)** — the per-context migration write path that #2433 silently broke and PR-1 of the cascade train repairs. |

Later PRs (PR-2, PR-3) add workflows `01`..`06` covering namespace cascade,
HLC fence, multi-version coexistence, etc.

## Fixtures

These workflows use the [`apps/migrations/migration-suite-v{1..5}`](../../apps/migrations)
fixtures (introduced in #1911). Each suite has a `#[app::state]` struct, a
`#[app::migrate]` function from the previous version, and a `schema_info()`
view returning the current `schema_version` plus the live field shape — the
workflows pipe this through `json_assert` for post-upgrade verification.

PR-1 only depends on v1 and v2-add-field; later PRs add the v3/v4/v5
fixtures to the build helper.

## Building the fixtures

```bash
bash workflows/app-migration/build-wasms.sh
```

Or build a single suite:

```bash
bash apps/migrations/migration-suite-v1/build.sh
```

## Running locally

Requires `merobox >= 0.6.16` and Docker. Use the published `merod:edge`
image, or build a local `merod:local`:

```bash
# Build fixtures
bash workflows/app-migration/build-wasms.sh

# Run against edge
merobox bootstrap run workflows/app-migration/00-single-group-migration-baseline.yml --verbose

# Or against locally built merod
merobox bootstrap run workflows/app-migration/00-single-group-migration-baseline.yml \
    --image merod:local --e2e-mode --verbose
```

## CI

`.github/workflows/app-migration-e2e.yml` runs every workflow in this
directory on every PR touching migration-related paths. Per-node docker
logs are uploaded as artefacts for triage.
````

- [ ] **Step 4: Create the GHA job**

Create `.github/workflows/app-migration-e2e.yml` with these exact contents:

```yaml
name: App migration e2e

# End-to-end coverage for the per-context application migration
# pipeline and its later namespace-cascade extensions. Runs every
# workflow under workflows/app-migration/ against a locally built
# merod:local image, exercising the migration-suite fixtures under
# apps/migrations/migration-suite-v*.

permissions:
  contents: read

on:
  push:
    branches: [master]
    paths:
      - "crates/context/src/handlers/update_application/**"
      - "crates/context/src/handlers/upgrade_group.rs"
      - "crates/context/src/handlers/apply_signed_group_op.rs"
      - "crates/context/primitives/src/local_governance/**"
      - "crates/storage/src/interface.rs"
      - "crates/sdk/macros/**"
      - "apps/migrations/**"
      - "workflows/app-migration/**"
      - ".github/workflows/app-migration-e2e.yml"
      - ".github/actions/build-local-merod/**"
      - ".github/actions/setup-merobox/**"

  pull_request:
    branches: [master]
    paths:
      - "crates/context/src/handlers/update_application/**"
      - "crates/context/src/handlers/upgrade_group.rs"
      - "crates/context/src/handlers/apply_signed_group_op.rs"
      - "crates/context/primitives/src/local_governance/**"
      - "crates/storage/src/interface.rs"
      - "crates/sdk/macros/**"
      - "apps/migrations/**"
      - "workflows/app-migration/**"
      - ".github/workflows/app-migration-e2e.yml"
      - ".github/actions/build-local-merod/**"
      - ".github/actions/setup-merobox/**"

  workflow_dispatch:

concurrency:
  group: ${{ github.workflow }}-${{ github.ref }}
  cancel-in-progress: true

env:
  CARGO_TERM_COLOR: always
  RUST_BACKTRACE: 1
  RUST_LOG: info,calimero_=debug
  CARGO_TARGET_DIR: ${{ github.workspace }}/target

jobs:
  app-migration-e2e:
    name: App migration e2e
    runs-on: ubuntu-24.04-8cpu
    timeout-minutes: 20
    steps:
      - name: Checkout code
        uses: actions/checkout@v4
        with:
          ref: ${{ github.event.pull_request.head.sha || github.sha }}

      - name: Setup Rust CI
        uses: ./.github/actions/setup-rust-ci
        with:
          toolchain: stable
          targets: wasm32-unknown-unknown
          shared-key: app-migration-e2e
          save-if: ${{ github.ref == 'refs/heads/master' }}

      - name: Build local merod image
        uses: ./.github/actions/build-local-merod
        env:
          CALIMERO_WEBUI_FETCH_TOKEN: ${{ secrets.GITHUB_TOKEN }}
          CALIMERO_AUTH_FRONTEND_FETCH_TOKEN: ${{ secrets.GITHUB_TOKEN }}

      - name: Build migration-suite WASM fixtures
        run: bash workflows/app-migration/build-wasms.sh

      - name: Setup merobox
        uses: ./.github/actions/setup-merobox

      - name: Run app-migration workflows
        run: |
          mkdir -p docker-logs
          LOGDIR="$GITHUB_WORKSPACE/docker-logs"
          WATCHER_STATE_DIR="$(mktemp -d)"
          (
            while [ ! -f "$WATCHER_STATE_DIR/stop" ]; do
              for c in $(docker ps --filter "label=calimero.node=true" \
                                   --format "{{.Names}}" 2>/dev/null \
                         | grep -v -- '-init$' || true); do
                mark="$WATCHER_STATE_DIR/${c}"
                if [ ! -f "$mark" ]; then
                  touch "$mark"
                  echo "[log-watcher] following $c"
                  docker logs -f --timestamps "$c" \
                    > "$LOGDIR/${c}.log" 2>&1 &
                fi
              done
              sleep 1
            done
          ) &
          WATCHER_PID=$!

          set +e
          set -o pipefail
          overall_exit=0
          for wf in workflows/app-migration/*.yml; do
              name=$(basename "$wf" .yml | tr -cd 'A-Za-z0-9_-')
              echo "::group::Running $name"
              merobox bootstrap run "$wf" \
                  --image merod:local \
                  --e2e-mode \
                  --verbose 2>&1 | tee "docker-logs/merobox-${name}.log"
              wf_exit=${PIPESTATUS[0]}
              echo "::endgroup::"
              if [ "$wf_exit" -ne 0 ]; then
                  echo "::error::Workflow $name failed with exit $wf_exit"
                  overall_exit=$wf_exit
              fi
          done
          set +o pipefail

          touch "$WATCHER_STATE_DIR/stop"
          sleep 2
          kill "$WATCHER_PID" 2>/dev/null || true
          for pid in $(jobs -p); do kill "$pid" 2>/dev/null || true; done
          wait 2>/dev/null || true

          exit "$overall_exit"

      - name: Upload docker logs
        if: always()
        uses: actions/upload-artifact@v4
        with:
          name: app-migration-e2e-docker-logs
          path: docker-logs/
          retention-days: 7
          if-no-files-found: ignore
```

- [ ] **Step 5: Verify YAML syntax**

Run:
```bash
python3 -c "import yaml; yaml.safe_load(open('.github/workflows/app-migration-e2e.yml'))" && \
echo "GHA workflow YAML valid"
```

Expected: prints `GHA workflow YAML valid`. If you see a YAMLError, fix the YAML before continuing.

- [ ] **Step 6: Commit scaffolding**

```bash
git add workflows/app-migration/build-wasms.sh \
        workflows/app-migration/README.md \
        .github/workflows/app-migration-e2e.yml
git commit -m "ci(app-migration): scaffold workflows directory + GHA job

Adds build helper, README, and CI job for the new workflows/app-migration/
directory. No workflows yet — they land alongside the code they exercise:
PR-1 adds 00-single-group-migration-baseline.yml as the #2433 regression
guard; later PRs add 01-06."
```

Expected: commit lands cleanly.

---

### Task 4: Write workflow 00 (the failing test) and verify the regression

This task captures the bug **before** fixing it, so we have empirical proof the fix changes behavior. TDD applied to e2e workflows: write the workflow, run it, watch it fail with the actual regression error, *then* fix the code.

**Files:**
- Create: `workflows/app-migration/00-single-group-migration-baseline.yml`

- [ ] **Step 1: Create the workflow YAML**

Create `workflows/app-migration/00-single-group-migration-baseline.yml`:

```yaml
name: 00 — Single-group migration baseline (v1 → v2)
description: >
  Single-node, single-group end-to-end migration. Installs
  migration-suite-v1, creates a context, writes v1 state, installs
  migration-suite-v2-add-field, runs upgrade_group with
  migrate_v1_to_v2. Asserts the Migrated event + v2 schema shape.

  Acts as the regression guard for #2433: that PR changed Root<T>
  merge semantics, breaking write_migration_state's save_raw call
  with MergeFailure(NoMergeFunctionRegistered). Pre-fix this
  workflow fails at the upgrade_group step; post-fix it passes.

no_docker: false
nuke_on_start: true
nuke_on_end: false

nodes:
  count: 1
  image: merod:local
  prefix: app-migration-baseline
  base_port: 9220
  base_rpc_port: 9320

steps:
  - name: Install migration-suite-v1
    type: install_application
    node: app-migration-baseline-1
    path: apps/migrations/migration-suite-v1/res/migration_suite_v1.wasm
    dev: true
    outputs:
      app_v1: applicationId

  - name: Install migration-suite-v2-add-field
    type: install_application
    node: app-migration-baseline-1
    path: apps/migrations/migration-suite-v2-add-field/res/migration_suite_v2_add_field.wasm
    dev: true
    outputs:
      app_v2: applicationId

  - name: Create namespace
    type: create_namespace
    node: app-migration-baseline-1
    application_id: "{{app_v1}}"
    outputs:
      namespace_id: namespaceId

  - name: Capture admin key (for teardown)
    type: get_namespace_identity
    node: app-migration-baseline-1
    namespace_id: "{{namespace_id}}"
    outputs:
      admin_key: publicKey

  - name: Create subgroup
    type: create_group_in_namespace
    node: app-migration-baseline-1
    namespace_id: "{{namespace_id}}"
    group_name: "baseline-migration"
    outputs:
      group_id: groupId

  - name: Create context in subgroup (v1)
    type: create_context
    node: app-migration-baseline-1
    application_id: "{{app_v1}}"
    group_id: "{{group_id}}"
    outputs:
      ctx_id: contextId

  - name: Wait for context propagation
    type: wait
    seconds: 3

  - name: Set description (v1)
    type: call
    node: app-migration-baseline-1
    context_id: "{{ctx_id}}"
    method: set_description
    args:
      description: "set-before-migration"

  - name: Increment counter (v1)
    type: call
    node: app-migration-baseline-1
    context_id: "{{ctx_id}}"
    method: increment_counter

  - name: Read v1 schema_info (sanity)
    type: call
    node: app-migration-baseline-1
    context_id: "{{ctx_id}}"
    method: schema_info
    outputs:
      v1_schema: result

  - name: Assert v1 schema before migration
    type: json_assert
    statements:
      - 'json_subset({{v1_schema}}, {"output": {"schema_version": "1.0.0", "description": "set-before-migration", "counter": "1"}})'

  - name: Set upgrade policy to coordinated
    type: update_group_settings
    node: app-migration-baseline-1
    group_id: "{{group_id}}"
    upgrade_policy: coordinated

  - name: Upgrade with migrate_v1_to_v2
    type: upgrade_group
    node: app-migration-baseline-1
    group_id: "{{group_id}}"
    target_application_id: "{{app_v2}}"
    migrate_method: migrate_v1_to_v2

  - name: Wait for upgrade + migration to apply
    type: wait
    seconds: 8

  - name: Read upgrade status
    type: get_group_upgrade_status
    node: app-migration-baseline-1
    group_id: "{{group_id}}"

  - name: Assert Migrated event observed (v1 → v2)
    type: assert_log_present
    nodes:
      - app-migration-baseline-1
    patterns:
      - "Migrated"
      - "1.0.0"
      - "2.0.0"
    min_matches: 1

  - name: Read post-migration schema_info
    type: call
    node: app-migration-baseline-1
    context_id: "{{ctx_id}}"
    method: schema_info
    outputs:
      v2_schema: result

  - name: Assert v2 schema after migration
    type: json_assert
    statements:
      - 'json_subset({{v2_schema}}, {"output": {"schema_version": "2.0.0"}})'
      - 'json_subset({{v2_schema}}, {"output": {"description": "set-before-migration", "counter": "1"}})'
      - 'json_subset({{v2_schema}}, {"output": {"notes": "added in v2"}})'

  - name: Call v2-only setter set_notes
    type: call
    node: app-migration-baseline-1
    context_id: "{{ctx_id}}"
    method: set_notes
    args:
      notes: "written-post-migration"

  - name: Read notes via v2 getter
    type: call
    node: app-migration-baseline-1
    context_id: "{{ctx_id}}"
    method: get_notes
    outputs:
      notes_after: result

  - name: Assert v2 setter/getter round-tripped
    type: json_assert
    statements:
      - 'json_equal({{notes_after}}, {"output": "written-post-migration"})'

  - name: Delete context
    type: delete_context
    node: app-migration-baseline-1
    context_id: "{{ctx_id}}"
    requester: "{{admin_key}}"

  - name: Delete subgroup
    type: delete_group
    node: app-migration-baseline-1
    group_id: "{{group_id}}"
    requester: "{{admin_key}}"

  - name: Delete namespace
    type: delete_namespace
    node: app-migration-baseline-1
    namespace_id: "{{namespace_id}}"
    requester: "{{admin_key}}"

  - name: Uninstall v1
    type: uninstall_application
    node: app-migration-baseline-1
    application_id: "{{app_v1}}"

  - name: Uninstall v2
    type: uninstall_application
    node: app-migration-baseline-1
    application_id: "{{app_v2}}"

stop_all_nodes: false
```

- [ ] **Step 2: Verify workflow YAML parses**

Run:
```bash
python3 -c "import yaml; yaml.safe_load(open('workflows/app-migration/00-single-group-migration-baseline.yml'))" && \
echo "Workflow YAML valid"
```

Expected: prints `Workflow YAML valid`.

- [ ] **Step 3: Build the migration-suite WASM fixtures**

Run:
```bash
bash workflows/app-migration/build-wasms.sh
```

Expected: each suite's `build.sh` runs, producing `apps/migrations/migration-suite-v1/res/migration_suite_v1.wasm` and `apps/migrations/migration-suite-v2-add-field/res/migration_suite_v2_add_field.wasm`. Final line: `>>> All migration-suite fixtures built.`

If a build fails: read the cargo error, fix the fixture, retry. (The fixtures themselves are upstream from PR #1911 and should still build cleanly — if they don't, that's a separate-and-load-bearing issue to surface to the user before proceeding.)

- [ ] **Step 4: Build local merod docker image**

Run:
```bash
./scripts/build-local-merod.sh 2>&1 | tail -20
```

(Or use whatever the project's local-merod build invocation is — check `.github/actions/build-local-merod/` for the canonical command if the script path differs.)

Expected: `merod:local` image present after completion. Verify with:
```bash
docker images merod:local --format "{{.Repository}}:{{.Tag}}"
```
Output: `merod:local`.

- [ ] **Step 5: Run workflow 00 — expect FAILURE (pre-fix regression)**

Run:
```bash
merobox bootstrap run workflows/app-migration/00-single-group-migration-baseline.yml \
    --image merod:local --e2e-mode --verbose 2>&1 | tee /tmp/workflow00-prefix.log
```

Expected: workflow fails at the `Upgrade with migrate_v1_to_v2` step. The merod node log (visible via `docker logs <node-container-name>`) should contain `MergeFailure(NoMergeFunctionRegistered)` or `Failed to write migrated state` (the bail in `write_migration_state`). This is the #2433 regression manifesting.

If the failure mode is different: read the actual error carefully — it may indicate the regression has shifted (perhaps another merge-layer change landed) or there's an unrelated fixture/runtime issue. Surface the divergence to the user before proceeding with the fix.

- [ ] **Step 6: Capture failure evidence in commit message**

Don't commit yet. Note the exact error string from step 5's log — it goes into the fix's commit message as proof of the regression.

---

### Task 5: Apply the fix (write_migration_state → write_pre_merged_root_state)

**Files:**
- Modify: `crates/context/src/handlers/update_application/mod.rs:741-792`

- [ ] **Step 1: Verify exact pre-fix shape of `write_migration_state`**

Run:
```bash
sed -n '705,795p' crates/context/src/handlers/update_application/mod.rs
```

Expected: matches the `fn write_migration_state(...)` signature and body shown in the spec — uses `Interface::<MainStorage>::save_raw(...)`, returns `Result<...>` wrapping an `Option`. If the file has drifted, re-read the function carefully and adapt the Edit string below to match the actual current content.

- [ ] **Step 2: Apply the fix**

Edit `crates/context/src/handlers/update_application/mod.rs`. Find this block (currently lines ~745-770 inside the `with_runtime_env` closure):

```rust
        let write_result = (|| -> Result<_, StorageError> {
            // Write the entry — this updates the entry's Index and propagates hashes
            // up to the Merkle tree root via recalculate_ancestor_hashes_for.
            let save_result =
                Interface::<MainStorage>::save_raw(root_entry_id, entry_bytes, metadata)?;

            if save_result.is_none() {
                return Ok(None);
            }

            // Read the Merkle tree root hash — same as the normal execution flow
            // save_raw returns the *entry node's* full_hash, not the tree root's.
            let root_hash = Index::<MainStorage>::get_hashes_for(Id::root())?
                .map(|(full_hash, _)| full_hash)
                .unwrap_or([0; 32]);

            Ok(Some(root_hash))
        })();
```

Replace with:

```rust
        let write_result = (|| -> Result<[u8; 32], StorageError> {
            // Write the migrated root state via the pre-merged primitive
            // introduced by #2465. The caller (this function) is the
            // source of truth for the new bytes — the wasm migrate
            // function already produced fully-resolved v2-shaped state,
            // so there is no host-side merge to dispatch and no app-type
            // entry in the host's merge registry (which lives in the
            // wasm runtime since #2465's host/WASM split). Using
            // `save_raw` here hit `MergeFailure(NoMergeFunctionRegistered)`
            // because save_raw expects a registered Mergeable for
            // root-class entries — that's the #2433 regression this
            // function existed to trigger.
            let _entry_own_hash = Interface::<MainStorage>::write_pre_merged_root_state(
                root_entry_id,
                &entry_bytes,
                metadata,
            )?;

            // Read the Merkle tree root hash — write_pre_merged_root_state
            // returns the *entry node's* full_hash, but the migration
            // caller (system.rs's normal execution flow analogue) needs
            // the tree root hash for ContextMeta.root_hash.
            let root_hash = Index::<MainStorage>::get_hashes_for(Id::root())?
                .map(|(full_hash, _)| full_hash)
                .unwrap_or([0; 32]);

            Ok(root_hash)
        })();
```

Also update the surrounding `match result { ... }` block (currently lines ~771-792). Find:

```rust
    match result {
        Ok(Some(root_hash)) => {
            debug!(
                %context_id,
                root_hash = ?root_hash,
                "Migrated state written successfully with Index update"
            );
            Ok(root_hash)
        }
        Ok(None) => {
            error!(
                %context_id,
                "Migration state write was unexpectedly skipped - timestamp conflict"
            );
            bail!(
                "Migration state write was unexpectedly skipped - timestamp conflict. \
                 This indicates a concurrent update conflict that prevented the migration \
                 state from being written. The migration must be retried."
            )
        }
        Err(e) => {
            error!(
                %context_id,
                error = ?e,
                "Failed to write migrated state"
            );
            bail!("Failed to write migrated state: {:?}", e)
        }
    }
```

Replace with:

```rust
    match result {
        Ok(root_hash) => {
            debug!(
                %context_id,
                root_hash = ?root_hash,
                "Migrated state written successfully with Index update"
            );
            Ok(root_hash)
        }
        Err(e) => {
            error!(
                %context_id,
                error = ?e,
                "Failed to write migrated state"
            );
            bail!("Failed to write migrated state: {:?}", e)
        }
    }
```

The `Ok(None)` branch is unreachable — `write_pre_merged_root_state` returns `[u8; 32]` directly, with the LWW-skip path returning the existing hash (not `None`). For migration the LWW-skip behavior is actually correct: if local state is already newer than what the migration wants to write, the migration is effectively a no-op (some concurrent op already advanced the timestamp) and we return the existing hash, which the caller will record as the post-migration root_hash.

- [ ] **Step 3: Type-check the change**

Run:
```bash
cargo check -p calimero-context 2>&1 | tail -20
```

Expected: compiles cleanly with no errors. If there's a "no method `write_pre_merged_root_state`" error, double-check the worktree is on the latest origin/master (which contains #2465). If there's a "mismatched types" error, re-read the function signature in `crates/storage/src/interface.rs:2200` and adapt the call site (signature is `pub fn write_pre_merged_root_state(id: Id, merged: &[u8], metadata: Metadata) -> Result<[u8; 32], StorageError>`).

- [ ] **Step 4: Run the existing unit tests to confirm no regression in surrounding code**

Run:
```bash
cargo test -p calimero-context --lib update_application 2>&1 | tail -10
```

Expected: all 16 `verify_appkey_continuity` tests pass. (These don't touch `write_migration_state` directly — they test the validation that runs before the write — so they should be unaffected. A failure here indicates an unrelated regression and should be surfaced before proceeding.)

---

### Task 6: Verify the fix via workflow 00

**Files:** none (verification only).

- [ ] **Step 1: Rebuild merod:local with the fix**

Run:
```bash
./scripts/build-local-merod.sh 2>&1 | tail -10
```

Expected: `merod:local` image rebuilt; the new code is baked in.

- [ ] **Step 2: Re-run workflow 00 — expect PASS (post-fix)**

Run:
```bash
merobox bootstrap run workflows/app-migration/00-single-group-migration-baseline.yml \
    --image merod:local --e2e-mode --verbose 2>&1 | tee /tmp/workflow00-postfix.log
```

Expected: workflow completes with exit code 0. The final summary should show every step PASSED. In particular:
- `Upgrade with migrate_v1_to_v2` succeeds (no `MergeFailure(NoMergeFunctionRegistered)`).
- `Assert Migrated event observed` finds the `Migrated`/`1.0.0`/`2.0.0` log lines.
- `Assert v2 schema after migration` passes (state shape is v2; v1 data preserved; v2-only `notes` field populated).
- `Assert v2 setter/getter round-tripped` confirms post-migration writes work.

If anything fails: capture the merod node logs (`docker logs <container-name>` or via the workflow's stdout) and surface the failure. Don't proceed to commit until workflow 00 is green.

- [ ] **Step 3: Verify the diff before commit**

Run:
```bash
git diff crates/context/src/handlers/update_application/mod.rs
```

Expected: shows ONLY the changes from Task 5 step 2 — the `save_raw` → `write_pre_merged_root_state` swap and the simplified match arm. No collateral edits.

---

### Task 7: Commit the fix

**Files:** none (git operations only).

- [ ] **Step 1: Stage the fix + workflow**

Run:
```bash
git add crates/context/src/handlers/update_application/mod.rs \
        workflows/app-migration/00-single-group-migration-baseline.yml
git status
```

Expected: two files staged (the Rust fix and the e2e workflow). No other files in the staging area.

- [ ] **Step 2: Commit with regression-evidence message**

Run:
```bash
git commit -m "$(cat <<'EOF'
fix(context): use write_pre_merged_root_state for migration writes

write_migration_state calls into Interface::save_raw to persist the
v2-shaped state returned by a wasm migrate function. Since #2433
changed Root<T> merge semantics, save_raw routes root-class entries
through the host's merge registry — which since #2465 is intentionally
empty for app types (those merges live in the wasm-side registry,
dispatched via __calimero_merge_root_state). The result: every
migration that went through write_migration_state failed with
MergeFailure(NoMergeFunctionRegistered) at the storage layer, and
bail'd up to the operator as "Failed to write migrated state".

write_pre_merged_root_state (added by #2465 at interface.rs:2200) is
the right primitive here. The migrate function has already produced
fully-resolved v2-shaped bytes — there is no host-side merge to
dispatch. The pre-merged path also has a built-in LWW guard so
concurrent timestamp races are handled by the primitive instead of
the surrounding match arm (the prior `Ok(None)` branch becomes
unreachable; an existing-newer-state case now returns the existing
hash rather than bailing).

Regression guard: workflows/app-migration/00-single-group-migration-baseline.yml
installs migration-suite-v1, writes state, installs
migration-suite-v2-add-field, runs upgrade_group with migrate_v1_to_v2,
and asserts the Migrated event + v2 schema. Pre-fix the workflow fails
at upgrade_group with the regression error; post-fix it passes.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

Expected: commit lands on `fix/migration-write-pre-merged`.

- [ ] **Step 3: Verify commit**

Run:
```bash
git log --oneline -3
```

Expected: top three commits are (most recent first): the fix commit, the scaffolding commit from Task 3, and the origin/master HEAD the branch was based on.

---

### Task 8: Push and open the PR

**Files:** none (git/GH operations only).

- [ ] **Step 1: Push the branch**

Run:
```bash
git push -u origin fix/migration-write-pre-merged
```

Expected: branch pushed; tracking set.

- [ ] **Step 2: Open the PR (NOT draft — per the user's PR-discipline rule)**

Run:
```bash
gh pr create --title "fix(context): repair per-context migration write path broken by #2433" --body "$(cat <<'EOF'
## Summary

- `write_migration_state` has been silently broken since #2433: it called `Interface::save_raw` for the migrated v2 state, but #2433 + #2465 made `save_raw` require a registered Mergeable for root-class entries — and app types are deliberately absent from the host-side merge registry post-#2465 (they live in the wasm registry).
- Swap the call to `Interface::write_pre_merged_root_state` (added in #2465 specifically for this shape: caller-owned merged bytes, no host-side merge dispatch, built-in LWW guard).
- Add `workflows/app-migration/00-single-group-migration-baseline.yml` as the regression guard, plus the supporting CI job (`app-migration-e2e.yml`), build helper, and README. The workflow installs v1, writes state, installs v2-add-field, runs `upgrade_group` with `migrate_v1_to_v2`, and asserts the `Migrated` event + v2 schema shape. Pre-fix it fails; post-fix it passes.

This is PR-1 of a four-PR train designed in `docs/superpowers/specs/2026-05-26-namespace-cascade-migration-design.md`. PR-2 builds the cascade engine on top of this fix.

## Test plan

- [x] `cargo check -p calimero-context` clean
- [x] Existing 16 `verify_appkey_continuity` unit tests still pass
- [x] `workflows/app-migration/00-single-group-migration-baseline.yml` PASSES locally with the fix
- [x] Same workflow FAILS locally without the fix (regression reproduced)
- [ ] CI `app-migration-e2e` job passes on this PR
- [ ] Other existing CI jobs unaffected

🤖 Generated with [Claude Code](https://claude.com/claude-code)
EOF
)"
```

Expected: PR opened in ready-for-review state (not draft). PR URL returned — surface it to the user.

- [ ] **Step 3: Watch CI**

Run:
```bash
gh pr checks --watch
```

(or report the PR URL and pause for the user to monitor CI themselves.)

Expected: `app-migration-e2e` job goes green. If other jobs go red, triage and decide whether they're flaky/unrelated or caused by this change.

---

## Self-Review

Spec coverage check (against `docs/superpowers/specs/2026-05-26-namespace-cascade-migration-design.md` §9.2 "Core PR-1"):
- ✓ Write-path fix at `update_application/mod.rs:705` — Task 5
- ✓ e2e workflow `00-single-group-migration-baseline.yml` — Task 4
- ✓ `.github/workflows/app-migration-e2e.yml` CI job — Task 3
- ✓ `build-wasms.sh` + README helpers — Task 3
- ✗ Rust integ test `migration_regression.rs` — **deliberately deferred**. The simplest regression guard is the e2e workflow (Task 4); a Rust integ test for `write_migration_state` directly requires a runtime/wasm test harness that doesn't exist in the repo today. Building that harness is its own work and isn't load-bearing for this fix. Logged as a follow-up worth doing alongside PR-2's Rust integ tests where the harness will be partly built anyway.

Placeholder scan: none of the "TBD/TODO/handle edge cases/similar to Task N" red-flag patterns appear. Every code block contains the actual content to write or run.

Type consistency: `write_pre_merged_root_state` signature `(Id, &[u8], Metadata) -> Result<[u8; 32], StorageError>` is consistent across Tasks 5.2 and the surrounding match-arm rewrite. The `_entry_own_hash` binding name in 5.2 matches the doc comment's "returns the entry node's full_hash" language.

Cross-task consistency: workflow filename `00-single-group-migration-baseline.yml` is identical in Tasks 3 (README, GHA job glob `workflows/app-migration/*.yml`), 4 (creation), 6 (run), 7 (stage). Branch name `fix/migration-write-pre-merged` is consistent in Tasks 2, 7, 8.

Risk note: the local pre-fix verification step (Task 4 step 5) requires a working `merod:local` build environment — if the contributor's local docker/Rust setup can't build merod, they can skip steps 4.4/4.5/6.1/6.2 and let CI do the verification post-push. The fix itself (Task 5) is small enough that this fallback is acceptable; just call it out in the PR description that local verification was skipped.

---

## Execution Handoff

Plan complete and saved to `docs/superpowers/plans/2026-05-26-pr1-migration-write-path-fix.md`.

Two execution options:

**1. Subagent-Driven (recommended)** — I dispatch a fresh subagent per task, review between tasks, fast iteration.

**2. Inline Execution** — Execute tasks in this session using executing-plans, batch execution with checkpoints.

Which approach?
