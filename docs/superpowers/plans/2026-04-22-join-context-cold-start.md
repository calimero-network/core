# join_context cold-start fix Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `join_context` succeed deterministically on first cold-start attempt when gossip deltas arrive before sync, by (a) not letting an unrelated inbound stream kill the active sync session and (b) aggressively pulling missing parents across mesh peers before reporting sync success.

**Architecture:** Two in-file edits to `crates/node/src/sync/manager/mod.rs`. **Fix C** changes `internal_handle_opened_stream` to close unknown-context streams cleanly instead of bailing. **Fix B** adds a bounded cross-peer retry loop in `request_dag_heads_and_sync` that re-queries `DeltaStore::get_missing_parents` and pulls from other mesh peers until resolved or the budget is exhausted, bailing loud on exhaustion so the caller learns the sync failed instead of getting silent `Ok`.

**Tech Stack:** Rust 1.88, tokio, eyre, libp2p (gossipsub mesh), actix (NodeManager), RocksDB (store). Tests via `cargo test -p calimero-node`.

**Reference:** `docs/superpowers/specs/2026-04-22-join-context-cold-start-design.md`

---

## File map

**Modify:**

- `crates/node/src/sync/manager/mod.rs` — the two edit sites (around lines 1819-1860 and 2207-2209).
- `crates/node/src/sync/config.rs` — add two constants for the parent-pull retry budget.

**Test (create or extend):**

- `crates/node/src/sync/manager/tests.rs` — extend with unit-level assertions where feasible.
- `crates/node/tests/sync_scenarios/mod.rs` — new integration scenarios (this file `mod`-declares scenario tests; follow the same pattern the existing entries use).

**No changes to:** `crates/context/`, `crates/server/`, the `SyncProtocol` enum in `crates/node/primitives/src/sync/protocol.rs`, or any public API.

---

## Task 1: Baseline — reproduce current behaviour in a failing test

**Files:**
- Modify: `crates/node/src/sync/manager/mod.rs` (read only, to anchor line numbers)
- Test: `crates/node/src/sync/manager/tests.rs`

- [ ] **Step 1: Open `crates/node/src/sync/manager/mod.rs` and confirm the two edit sites**

Expected content at line 2207-2209:

```rust
let Some(context) = self.context_client.get_context(&context_id)? else {
    bail!("context not found: {}", context_id);
};
```

Expected content at line 1857-1860 (the "return success regardless of pending" site):

```rust
// Return a non-None protocol to signal success (prevents trying next peer)
Ok(SyncProtocol::DeltaSync {
    missing_delta_ids: vec![],
})
```

If either site has drifted, stop and re-locate via:

```bash
grep -n "context not found:" crates/node/src/sync/manager/mod.rs
grep -n "prevents trying next peer" crates/node/src/sync/manager/mod.rs
```

- [ ] **Step 2: Run the existing test suite once to confirm green baseline**

```bash
cargo test -p calimero-node --lib sync::manager::tests -- --nocapture
```

Expected: all existing tests in `manager/tests.rs` pass. Note the count; subsequent tasks should grow it, never shrink it.

- [ ] **Step 3: Commit nothing — this is a read-only baseline task**

No commit.

---

## Task 2: Add config constants for parent-pull retry budget

**Files:**
- Modify: `crates/node/src/sync/config.rs`

- [ ] **Step 1: Append two constants at the end of `crates/node/src/sync/config.rs`**

Add after the existing `DEFAULT_MESH_RETRY_DELAY_MS_UNINITIALIZED` constant (currently at line 58):

```rust
/// Maximum number of *additional* mesh peers to try for missing-parent fetches
/// after the initial sync peer returns without fully resolving the DAG.
///
/// The initial peer attempt is not counted toward this budget.
pub const DEFAULT_PARENT_PULL_ADDITIONAL_PEERS: usize = 3;

/// Total wall-clock budget (milliseconds) for the cross-peer missing-parent
/// fetch loop, including the initial peer attempt. When exhausted, the sync
/// session returns an error rather than reporting silent success.
pub const DEFAULT_PARENT_PULL_BUDGET_MS: u64 = 10_000;
```

- [ ] **Step 2: Compile-check**

```bash
cargo check -p calimero-node
```

Expected: clean build. No other file references these constants yet — that's wired up in Task 5.

- [ ] **Step 3: Commit**

```bash
git add crates/node/src/sync/config.rs
git commit -m "feat(node/sync): add parent-pull retry budget constants"
```

---

## Task 3: Fix C — write failing test for unknown-context stream

**Files:**
- Test: `crates/node/src/sync/manager/tests.rs`

This task proves the current `bail!` path propagates as an error. Because `internal_handle_opened_stream` takes a real `Stream`, we test the smallest observable behaviour via a scenario integration test, not a unit test on the private function. If the in-file `tests.rs` cannot construct a real `SyncManager` easily, place the test in `crates/node/tests/sync_scenarios/unknown_context_stream.rs` and register it in `crates/node/tests/sync_scenarios/mod.rs`.

- [ ] **Step 1: Locate the closest existing two-node scenario and model the new test on it**

```bash
grep -rln "SyncManager\|NodeManager" crates/node/tests/sync_scenarios/ | head
ls crates/node/tests/sync_scenarios/
```

Pick the simplest existing scenario (one that runs two nodes, one sends an `Init` stream message to the other). Re-use its harness. Name the new file: `crates/node/tests/sync_scenarios/unknown_context_stream.rs`.

- [ ] **Step 2: Write the failing scenario**

Create `crates/node/tests/sync_scenarios/unknown_context_stream.rs`:

```rust
//! Scenario: an inbound sync stream whose Init.context_id is unknown to the
//! receiver must close cleanly without killing a concurrent legitimate sync.
//!
//! Covers Fix C from docs/superpowers/plans/2026-04-22-join-context-cold-start.md

use super::harness::{TwoNodeHarness, random_context_id};

#[tokio::test(flavor = "multi_thread")]
async fn unknown_context_stream_closes_cleanly_and_does_not_kill_concurrent_sync() {
    let mut h = TwoNodeHarness::new().await;

    // Node B has docs_ctx and is in the middle of a legitimate sync with Node A.
    let docs_ctx = h.create_shared_context("docs").await;
    let legit_sync = h.spawn_sync(docs_ctx, /* initiator */ h.node_b());

    // While that sync runs, open a *second* stream from Node A -> Node B
    // with a context_id Node B does not know about.
    let unknown_ctx = random_context_id();
    let stray_result = h.send_raw_init_stream(h.node_a(), h.node_b(), unknown_ctx).await;

    // The stray stream must NOT panic, NOT corrupt Node B's state, and
    // MUST receive OpaqueError from Node B.
    assert!(stray_result.is_opaque_error(),
        "stray stream for unknown context should receive OpaqueError, got {:?}",
        stray_result);

    // The legitimate sync must complete successfully.
    let outcome = legit_sync.await.expect("legit sync join");
    assert!(outcome.is_ok(),
        "concurrent legitimate sync must succeed; got {:?}", outcome);
}
```

If `TwoNodeHarness::send_raw_init_stream` does not yet exist, add it to the harness alongside existing helpers. Its job: open a libp2p stream from the initiator to the target and send exactly one `StreamMessage::Init { context_id, party_id, payload: InitPayload::StateSync { .. }, next_nonce }` with a random `party_id` and nonce, then read one response and return it.

Register the new scenario in `crates/node/tests/sync_scenarios/mod.rs` by adding:

```rust
pub mod unknown_context_stream;
```

- [ ] **Step 3: Run the test and confirm it fails**

```bash
cargo test -p calimero-node --test sync_scenarios unknown_context_stream -- --nocapture
```

Expected failure mode today: either the assertion on `legit_sync` failing (because the `bail!` tears down Node B's sync dispatcher for the active session) or the `stray_result` returning a non-OpaqueError propagated error. Either is acceptable — both prove the behaviour under fix.

- [ ] **Step 4: Do not commit a failing test yet**

Move to Task 4 to make it pass before committing.

---

## Task 4: Fix C — change the bail to warn + OpaqueError + Ok(None)

**Files:**
- Modify: `crates/node/src/sync/manager/mod.rs`

- [ ] **Step 1: Replace the `else` arm at line 2207-2209**

Open `crates/node/src/sync/manager/mod.rs`. Find:

```rust
let Some(context) = self.context_client.get_context(&context_id)? else {
    bail!("context not found: {}", context_id);
};
```

Replace with:

```rust
let Some(context) = self.context_client.get_context(&context_id)? else {
    warn!(
        %context_id,
        ?their_identity,
        "inbound stream for unknown context, closing cleanly"
    );

    if let Err(err) = self
        .send(stream, &StreamMessage::OpaqueError, None)
        .await
    {
        error!(%err, %context_id, "failed to send OpaqueError for unknown context");
    }

    return Ok(None);
};
```

Notes:

- `warn!` is already imported at the top of the file.
- `error!` macro is already used elsewhere in this file (e.g. line 2155).
- `self.send(stream, &StreamMessage::OpaqueError, None)` matches the existing error-send call at `mod.rs:2152`.
- `return Ok(None)` matches the existing "no message" exit at `mod.rs:2163-2165`; the dispatcher already handles this case.
- The `?` operator on `get_context` still propagates real datastore errors — only the `None` case changes.

- [ ] **Step 2: Run the failing test from Task 3**

```bash
cargo test -p calimero-node --test sync_scenarios unknown_context_stream -- --nocapture
```

Expected: PASS. If it still fails, re-read the failure — it may reveal a harness bug in Task 3 rather than a Fix C bug. Only iterate on the production code if the failure points at it.

- [ ] **Step 3: Run the wider test suite to confirm no regressions**

```bash
cargo test -p calimero-node
```

Expected: all tests pass.

- [ ] **Step 4: Format + clippy**

```bash
cargo fmt
cargo clippy -p calimero-node -- -A warnings
```

Expected: clean.

- [ ] **Step 5: Commit**

```bash
git add crates/node/src/sync/manager/mod.rs \
        crates/node/tests/sync_scenarios/unknown_context_stream.rs \
        crates/node/tests/sync_scenarios/mod.rs
git commit -m "fix(node/sync): close unknown-context streams cleanly instead of bailing

Prevents an unrelated inbound stream from tearing down a concurrent
legitimate sync session. Fixes symptom #2 of calimero-network/core#2198."
```

---

## Task 5: Fix B — write failing test for cross-peer parent pull (success path)

**Files:**
- Test: `crates/node/tests/sync_scenarios/pending_parents_cross_peer.rs`
- Modify: `crates/node/tests/sync_scenarios/mod.rs`

- [ ] **Step 1: Inventory what the existing three-node scenarios give us**

```bash
grep -rln "three.*node\|3.*node\|ThreeNode" crates/node/tests/
grep -rn "build_dag\|seed_delta\|pre_seed" crates/node/tests/sync_scenarios/
```

We need:

- A three-node harness (A, B, C) or a way to compose two `TwoNodeHarness` instances with a shared address book.
- A way to pre-seed B's `DeltaStore` with a batch of child deltas whose parents are absent (i.e. simulate "gossip arrived before sync").
- A way to make A respond with DAG heads but *withhold* parent deltas (simulating the mero-drive race where the sync peer's DAG state lags).

If the harness already exposes these, great. If not, add the minimum needed to the harness — do NOT duplicate harness code into the test file.

- [ ] **Step 2: Write the failing scenario**

Create `crates/node/tests/sync_scenarios/pending_parents_cross_peer.rs`:

```rust
//! Scenario: when the initial sync peer returns without pulling all missing
//! parents, the sync session must iterate other mesh peers until the DAG
//! is fully resolved.
//!
//! Covers Fix B (happy path) from
//! docs/superpowers/plans/2026-04-22-join-context-cold-start.md

use super::harness::{ThreeNodeHarness, build_linear_dag};

#[tokio::test(flavor = "multi_thread")]
async fn cross_peer_parent_pull_succeeds_when_first_peer_lacks_parents() {
    let mut h = ThreeNodeHarness::new().await;

    let ctx = h.create_shared_context("docs").await;

    // Build a 20-delta linear DAG. Node C holds all 20. Node A holds only
    // the tip (delta 20). Node B is fresh but pre-seeded with the 19
    // children whose parents (1..=19) are not in local storage.
    let dag = build_linear_dag(ctx, /* length */ 20);

    h.seed_dag(h.node_c(), ctx, &dag[..]).await;          // full DAG
    h.seed_dag(h.node_a(), ctx, &dag[19..20]).await;       // tip only
    h.seed_pending_children(h.node_b(), ctx, &dag[1..20]).await;

    // Trigger B's sync. A is the initial peer (it has the tip but not
    // parents); C is reachable via the mesh for fallback.
    let outcome = h.trigger_sync(h.node_b(), ctx, /* initial_peer */ h.node_a()).await;

    assert!(outcome.is_ok(),
        "sync must succeed via cross-peer fallback; got {:?}", outcome);

    let pending = h.delta_store_missing_parents(h.node_b(), ctx).await;
    assert!(pending.is_empty(),
        "after sync, no missing parents should remain; got {} pending",
        pending.len());
}
```

Register the module in `crates/node/tests/sync_scenarios/mod.rs`:

```rust
pub mod pending_parents_cross_peer;
```

- [ ] **Step 3: Run it and confirm it fails today**

```bash
cargo test -p calimero-node --test sync_scenarios pending_parents_cross_peer -- --nocapture
```

Expected failure: sync returns `Ok` but `h.delta_store_missing_parents(...)` still has N entries — because today there is no cross-peer retry.

- [ ] **Step 4: No commit yet**

---

## Task 6: Fix B — implement cross-peer parent pull loop

**Files:**
- Modify: `crates/node/src/sync/manager/mod.rs`

- [ ] **Step 1: Locate the end of `request_dag_heads_and_sync`**

The edit site is the block around lines 1819-1860. Today:

```rust
// Phase 2: Now check for missing parents and fetch them recursively
let missing_result = delta_store_ref.get_missing_parents().await;

if !missing_result.cascaded_events.is_empty() {
    info!(
        %context_id,
        cascaded_count = missing_result.cascaded_events.len(),
        "Cascaded deltas from DB load during DAG head sync"
    );
}

if !missing_result.missing_ids.is_empty() {
    info!(
        %context_id,
        missing_count = missing_result.missing_ids.len(),
        "DAG heads have missing parents, requesting them recursively"
    );

    if let Err(e) = self
        .request_missing_deltas(
            context_id,
            missing_result.missing_ids,
            peer_id,
            delta_store_ref.clone(),
            our_identity,
        )
        .await
    {
        warn!(
            ?e,
            %context_id,
            "Failed to request missing parent deltas during DAG catchup"
        );
    }
}

// Return a non-None protocol to signal success (prevents trying next peer)
Ok(SyncProtocol::DeltaSync {
    missing_delta_ids: vec![],
})
```

- [ ] **Step 2: Replace with a bounded cross-peer retry loop**

Replace the block above (from `// Phase 2` through the final `Ok(SyncProtocol::DeltaSync { ... })`) with:

```rust
// Phase 2: Now check for missing parents and fetch them recursively
let missing_result = delta_store_ref.get_missing_parents().await;

if !missing_result.cascaded_events.is_empty() {
    info!(
        %context_id,
        cascaded_count = missing_result.cascaded_events.len(),
        "Cascaded deltas from DB load during DAG head sync"
    );
}

// First attempt: initial peer.
if !missing_result.missing_ids.is_empty() {
    info!(
        %context_id,
        missing_count = missing_result.missing_ids.len(),
        "DAG heads have missing parents, requesting them recursively"
    );

    if let Err(e) = self
        .request_missing_deltas(
            context_id,
            missing_result.missing_ids,
            peer_id,
            delta_store_ref.clone(),
            our_identity,
        )
        .await
    {
        warn!(
            ?e,
            %context_id,
            "Failed to request missing parent deltas from initial peer"
        );
    }
}

// Cross-peer fallback: if the initial peer didn't resolve everything,
// try other mesh peers for this context. Bounded by peer count and
// wall-clock budget. See docs/superpowers/specs/2026-04-22-...
let budget_started = std::time::Instant::now();
let budget = std::time::Duration::from_millis(
    super::config::DEFAULT_PARENT_PULL_BUDGET_MS,
);
let max_additional = super::config::DEFAULT_PARENT_PULL_ADDITIONAL_PEERS;

let topic = libp2p::gossipsub::TopicHash::from_raw(context_id.to_string());
let mut tried: std::collections::HashSet<libp2p::PeerId> =
    std::collections::HashSet::new();
tried.insert(peer_id); // don't retry the initial peer

let mut attempts = 0usize;
loop {
    let after_first = delta_store_ref.get_missing_parents().await;
    if after_first.missing_ids.is_empty() {
        break; // fully resolved
    }
    if attempts >= max_additional {
        break;
    }
    if budget_started.elapsed() >= budget {
        warn!(
            %context_id,
            elapsed_ms = budget_started.elapsed().as_millis() as u64,
            "parent-pull budget exhausted"
        );
        break;
    }

    let mesh_peers = self.network_client.mesh_peers(topic.clone()).await;
    let next_peer = mesh_peers.into_iter().find(|p| !tried.contains(p));
    let Some(next_peer) = next_peer else {
        debug!(
            %context_id,
            "no additional mesh peers available for parent pull"
        );
        break;
    };
    tried.insert(next_peer);
    attempts += 1;

    info!(
        %context_id,
        ?next_peer,
        attempt = attempts,
        still_missing = after_first.missing_ids.len(),
        "retrying missing-parent fetch against additional mesh peer"
    );

    if let Err(e) = self
        .request_missing_deltas(
            context_id,
            after_first.missing_ids,
            next_peer,
            delta_store_ref.clone(),
            our_identity,
        )
        .await
    {
        warn!(
            ?e,
            %context_id,
            ?next_peer,
            "cross-peer parent-pull attempt failed"
        );
    }
}

// Final check: if pending parents still remain, the sync did NOT
// fully restore the DAG. Return an error so the caller (join_context)
// surfaces a real failure instead of silent success.
let final_missing = delta_store_ref.get_missing_parents().await;
if !final_missing.missing_ids.is_empty() {
    warn!(
        %context_id,
        remaining = final_missing.missing_ids.len(),
        "DAG sync ended with unresolved missing parents"
    );
    bail!(
        "pending parents unresolved for context {}: {} remaining after \
         {} peer attempt(s)",
        context_id,
        final_missing.missing_ids.len(),
        attempts + 1,
    );
}

// Success: DAG is fully resolved.
Ok(SyncProtocol::DeltaSync {
    missing_delta_ids: vec![],
})
```

Notes on the edit:

- `libp2p::gossipsub::TopicHash::from_raw(context_id.to_string())` matches the pattern already used at `mod.rs:468`. Verify with `grep` — if the call site uses a slightly different `context_id` stringification, mirror that form.
- `self.network_client.mesh_peers(topic)` is already used at `mod.rs:468, 2659, 2739`.
- `super::config::DEFAULT_PARENT_PULL_ADDITIONAL_PEERS` / `_BUDGET_MS` were added in Task 2.
- `bail!` here is a behaviour change: previously this function returned `Ok` on partial-DAG; it now returns `Err`. That is the intended semantic.

- [ ] **Step 3: Run the Task 5 scenario test**

```bash
cargo test -p calimero-node --test sync_scenarios pending_parents_cross_peer -- --nocapture
```

Expected: PASS. If it fails because the harness is missing a helper, extend the harness minimally — do not work around with test-only mutation of production state.

- [ ] **Step 4: Run the full node crate test suite**

```bash
cargo test -p calimero-node
```

Expected: all green. If any previously-passing test now fails with a `pending parents unresolved` error, that test was depending on the silent-success-on-partial-DAG behaviour — inspect it. The likely fix is to extend the test's DAG fixture so there are no missing parents at sync end; the production change is correct.

- [ ] **Step 5: Format + clippy**

```bash
cargo fmt
cargo clippy -p calimero-node -- -A warnings
```

- [ ] **Step 6: Commit**

```bash
git add crates/node/src/sync/manager/mod.rs \
        crates/node/tests/sync_scenarios/pending_parents_cross_peer.rs \
        crates/node/tests/sync_scenarios/mod.rs
git commit -m "fix(node/sync): pull missing parents across mesh peers before reporting success

Iterates mesh peers for the context (bounded by count and wall-clock
budget) when the initial sync peer returns with pending parents. On
budget exhaustion, returns an error instead of silent success so
join_context fails fast. Fixes symptom #1 of
calimero-network/core#2198."
```

---

## Task 7: Fix B — write test for budget-exhausted path

**Files:**
- Test: `crates/node/tests/sync_scenarios/pending_parents_budget_exhausted.rs`
- Modify: `crates/node/tests/sync_scenarios/mod.rs`

- [ ] **Step 1: Write the test**

Create `crates/node/tests/sync_scenarios/pending_parents_budget_exhausted.rs`:

```rust
//! Scenario: when no peer holds the missing parents, the sync session
//! must fail loud (typed error) within the wall-clock budget, rather
//! than succeed silently on a partial DAG.
//!
//! Covers Fix B (budget-exhausted path) from
//! docs/superpowers/plans/2026-04-22-join-context-cold-start.md

use super::harness::{ThreeNodeHarness, build_linear_dag};

#[tokio::test(flavor = "multi_thread")]
async fn sync_fails_loud_when_no_peer_has_missing_parents() {
    let mut h = ThreeNodeHarness::new().await;

    let ctx = h.create_shared_context("docs").await;

    // 20-delta DAG. No peer holds deltas 1..=10 — the middle of the DAG
    // is genuinely unavailable. Node B needs them.
    let dag = build_linear_dag(ctx, /* length */ 20);

    h.seed_dag(h.node_a(), ctx, &dag[10..20]).await; // upper half only
    h.seed_dag(h.node_c(), ctx, &dag[10..20]).await; // upper half only
    h.seed_pending_children(h.node_b(), ctx, &dag[10..20]).await;

    let start = std::time::Instant::now();
    let outcome = h.trigger_sync(h.node_b(), ctx, /* initial_peer */ h.node_a()).await;
    let elapsed = start.elapsed();

    assert!(outcome.is_err(),
        "sync must return an error when parents cannot be resolved; got {:?}",
        outcome);
    let err_string = format!("{:#}", outcome.unwrap_err());
    assert!(err_string.contains("pending parents unresolved"),
        "error must identify the pending-parents failure; got: {}", err_string);

    // Must not hang past the configured budget (default 10s) plus a margin.
    assert!(elapsed < std::time::Duration::from_secs(15),
        "sync took {:?}, exceeding wall-clock budget", elapsed);
}
```

Register in `crates/node/tests/sync_scenarios/mod.rs`:

```rust
pub mod pending_parents_budget_exhausted;
```

- [ ] **Step 2: Run it**

```bash
cargo test -p calimero-node --test sync_scenarios pending_parents_budget_exhausted -- --nocapture
```

Expected: PASS (Fix B from Task 6 already covers this path; the test just validates the loud-failure contract).

- [ ] **Step 3: Commit**

```bash
git add crates/node/tests/sync_scenarios/pending_parents_budget_exhausted.rs \
        crates/node/tests/sync_scenarios/mod.rs
git commit -m "test(node/sync): cover parent-pull budget exhaustion path"
```

---

## Task 8: Grep for silent-success consumers

**Files:**
- Read-only across `crates/`

The semantic change in Task 6 turns an `Ok(SyncProtocol::DeltaSync { missing_delta_ids: vec![] })` into `Err(...)` when pending parents remain. Any caller that matched on `Ok` assuming "sync reported success implies DAG is usable" is now correct, but any caller that depended on the old tolerant behaviour will observe a new error. We do not guess — we check.

- [ ] **Step 1: Enumerate callers of `request_dag_heads_and_sync` and of the `sync` RPC client method**

```bash
grep -rn "request_dag_heads_and_sync\|\.sync(" crates/ --include='*.rs' | grep -v "test\|//\|^Binary"
```

- [ ] **Step 2: Inspect each caller and classify**

For each hit, determine:

- Does the caller rely on `Ok` implying DAG-applied? (If so, no change needed — we've made the contract real.)
- Does the caller explicitly tolerate a partial DAG? (If so, add a comment near the call site referencing issue #2198 and the new error form; do not change behaviour unless a regression appears.)

- [ ] **Step 3: Write findings into the commit message below even if no changes are required**

This is documentation work: an empty finding is still a finding, and future grepping for "#2198" should surface it.

- [ ] **Step 4: Commit (only if caller changes were made)**

If no changes were needed:

```bash
# no commit
```

If changes were made:

```bash
git add <files>
git commit -m "chore(node/sync): update sync callers for new pending-parents error contract

Issue: #2198"
```

---

## Task 9: Full verification

**Files:**
- Read-only

- [ ] **Step 1: Full test suite**

```bash
cargo test
```

Expected: all green across the workspace.

- [ ] **Step 2: Format check**

```bash
cargo fmt --check
```

Expected: no diff.

- [ ] **Step 3: Clippy**

```bash
cargo clippy -- -A warnings
```

Expected: clean.

- [ ] **Step 4: License audit (no dep changes expected, but confirm)**

```bash
cargo deny check licenses sources
```

Expected: pass. (If it fails and no deps changed, investigate separately — not scope for this PR.)

- [ ] **Step 5: Manual two-node smoke (optional but recommended)**

Follow the pattern in root-level CLAUDE.md:

```bash
merod --node node1 init --server-port 2428 --swarm-port 2528
merod --node node1 run &
merod --node node2 init --server-port 2429 --swarm-port 2529
merod --node node2 config --swarm-addrs /ip4/127.0.0.1/tcp/2528
RUST_LOG=info merod --node node2 run &
```

Create a context on node1, invite node2, call `join_context` from node2. Repeat 5 times with `merobox nuke --force` between runs. Before this fix: ≥1 failure on first attempt. After this fix: 5/5 success.

If the smoke tests fail, stop — the scenario tests passed but something is different in a live node run. Investigate before opening the PR.

- [ ] **Step 6: No commit — this is a verification pass**

---

## Task 10: Open the PR

- [ ] **Step 1: Push the branch and open a PR**

```bash
git push -u origin <branch-name>
gh pr create \
  --title "fix(node/sync): resolve cold-start join_context failure (#2198)" \
  --body "$(cat <<'EOF'
## Summary

- Close unknown-context inbound streams cleanly instead of bailing, so an unrelated stream cannot tear down a concurrent legitimate sync.
- Iterate mesh peers to pull missing parents within a bounded budget; return a typed error on exhaustion so `join_context` fails fast instead of succeeding silently on a partial DAG.

Fixes #2198.

Design: `docs/superpowers/specs/2026-04-22-join-context-cold-start-design.md`
Plan:   `docs/superpowers/plans/2026-04-22-join-context-cold-start.md`

## Test plan

- [x] `cargo test -p calimero-node --test sync_scenarios unknown_context_stream`
- [x] `cargo test -p calimero-node --test sync_scenarios pending_parents_cross_peer`
- [x] `cargo test -p calimero-node --test sync_scenarios pending_parents_budget_exhausted`
- [x] `cargo test`
- [x] `cargo fmt --check` / `cargo clippy -- -A warnings`
- [ ] mero-drive e2e: 5/5 first-attempt success (tracking separately)
EOF
)"
```

---

## Rollback

If any task fails in production after merge:

```bash
git revert <merge-commit>
```

Both fixes are contained in one PR, one file each, and carry their own tests. A revert is safe.
