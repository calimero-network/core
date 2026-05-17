# Namespace Governance Anti-Entropy Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make a node that missed a namespace governance op recover it by acting on the `ReadinessBeacon` divergence signal it already receives.

**Architecture:** Receiver-side anti-entropy. Patch 1: the `ReadinessBeacon` receive handler compares the beacon's advertised DAG head against the local namespace governance DAG; on divergence it triggers `sync_namespace`, debounced per-namespace. Patch 2: the `Subscribed` event handler emits an out-of-cycle beacon when a peer subscribes to a `ns/<hex>` topic, shaving cold-start recovery from ~5s to ~1s.

**Tech Stack:** Rust, actix actors, libp2p gossipsub, calimero-store (RocksDB), merobox e2e workflows.

**Spec:** `docs/superpowers/specs/2026-05-16-namespace-governance-anti-entropy-design.md`

---

## File Structure

| File | Change |
|---|---|
| `crates/node/src/manager.rs` | Add `ns_beacon_sync_debounce: HashMap<[u8;32], Instant>` field to `NodeManager` + init in `new()` |
| `crates/node/src/handlers/network_event/readiness.rs` | Add two pure helpers (`beacon_indicates_divergence`, `debounce_allows_sync`) + `#[cfg(test)]` unit tests; wire Patch 1 into `handle_readiness_beacon` |
| `crates/node/src/handlers/network_event/subscriptions.rs` | Add `ns/<hex>` arm to `handle_subscribed` (Patch 2) |
| `workflows/sync-tests/opaque-leaf-regression.yml` | Add Phase 6: node-2 joins + `wait_for_sync` the second context (regression sentinel) |

No GHA workflow change — Phase 6 runs inside the already-wired `opaque-leaf-regression.yml` job. No wire-format change. No publisher change.

---

## Task 1: Pure helpers + unit tests (readiness.rs)

**Files:**
- Modify: `crates/node/src/handlers/network_event/readiness.rs`

- [ ] **Step 1: Add the helpers and tests**

At the top of `readiness.rs`, after the existing `use` block, add:

```rust
use std::collections::HashMap;
use std::time::{Duration, Instant};
```

Before `pub(super) fn handle_readiness_beacon`, add:

```rust
/// Per-namespace debounce window for beacon-triggered governance syncs.
/// One beacon interval (~5s): a Ready peer beacons every ~5s, so without
/// this a behind-node would fire one sync per beacon per peer.
const NS_BEACON_SYNC_DEBOUNCE: Duration = Duration::from_secs(5);

/// True if the beacon's advertised DAG head names a namespace governance
/// op this node has not applied locally — i.e. the beaconing peer is
/// ahead and we should pull the namespace governance DAG from it.
///
/// A zero head (`[0u8; 32]`) means the peer has applied nothing yet;
/// never sync towards an empty DAG.
fn beacon_indicates_divergence(dag_head: [u8; 32], head_op_present_locally: bool) -> bool {
    dag_head != [0u8; 32] && !head_op_present_locally
}

/// Per-namespace debounce gate. Returns `true` (and records `now`) when
/// no beacon-triggered sync fired for `namespace_id` within
/// `NS_BEACON_SYNC_DEBOUNCE`; returns `false` otherwise.
fn debounce_allows_sync(
    debounce: &mut HashMap<[u8; 32], Instant>,
    namespace_id: [u8; 32],
    now: Instant,
) -> bool {
    match debounce.get(&namespace_id) {
        Some(last) if now.duration_since(*last) < NS_BEACON_SYNC_DEBOUNCE => false,
        _ => {
            let _ = debounce.insert(namespace_id, now);
            true
        }
    }
}
```

At the end of `readiness.rs`, add:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn divergence_true_when_head_op_absent() {
        assert!(beacon_indicates_divergence([7u8; 32], false));
    }

    #[test]
    fn divergence_false_when_head_op_present() {
        assert!(!beacon_indicates_divergence([7u8; 32], false) == false || true);
        assert!(!beacon_indicates_divergence([7u8; 32], true));
    }

    #[test]
    fn divergence_false_for_zero_head() {
        // A peer that has applied nothing advertises a zero head; never
        // sync towards an empty DAG even though the op is "absent".
        assert!(!beacon_indicates_divergence([0u8; 32], false));
    }

    #[test]
    fn debounce_allows_first_then_blocks_within_window() {
        let mut d: HashMap<[u8; 32], Instant> = HashMap::new();
        let t0 = Instant::now();
        assert!(debounce_allows_sync(&mut d, [1u8; 32], t0));
        // Second beacon 1s later — inside the 5s window — is blocked.
        assert!(!debounce_allows_sync(&mut d, [1u8; 32], t0 + Duration::from_secs(1)));
    }

    #[test]
    fn debounce_reallows_after_window() {
        let mut d: HashMap<[u8; 32], Instant> = HashMap::new();
        let t0 = Instant::now();
        assert!(debounce_allows_sync(&mut d, [1u8; 32], t0));
        assert!(debounce_allows_sync(
            &mut d,
            [1u8; 32],
            t0 + NS_BEACON_SYNC_DEBOUNCE + Duration::from_millis(1)
        ));
    }

    #[test]
    fn debounce_is_per_namespace() {
        let mut d: HashMap<[u8; 32], Instant> = HashMap::new();
        let t0 = Instant::now();
        assert!(debounce_allows_sync(&mut d, [1u8; 32], t0));
        // Different namespace — independent budget, still allowed.
        assert!(debounce_allows_sync(&mut d, [2u8; 32], t0));
    }
}
```

(Remove the redundant first assert in `divergence_false_when_head_op_present` — keep only `assert!(!beacon_indicates_divergence([7u8; 32], true));`.)

- [ ] **Step 2: Run tests — expect FAIL (helpers compile-only; field not yet added, handler not wired — tests should already PASS since helpers are self-contained)**

Run: `cargo test -p calimero-node --lib network_event::readiness::tests`
Expected: PASS (these helpers are pure; they pass immediately). This task is the TDD anchor for the predicate logic.

- [ ] **Step 3: Commit**

```bash
git add crates/node/src/handlers/network_event/readiness.rs
git commit -m "test(node): beacon-divergence + debounce helpers for namespace anti-entropy"
```

---

## Task 2: NodeManager debounce field

**Files:**
- Modify: `crates/node/src/manager.rs`

- [ ] **Step 1: Add the field**

In `crates/node/src/manager.rs`, add to the `NodeManager` struct (after `divergence_detected: Counter,`):

```rust
    /// Per-namespace timestamp of the last beacon-triggered governance
    /// sync (#2367). Caps beacon-divergence syncs to one per namespace
    /// per `NS_BEACON_SYNC_DEBOUNCE` window — beacons arrive every ~5s
    /// from every Ready peer, so an un-debounced behind-node would fire
    /// a sync per beacon per peer.
    pub(crate) ns_beacon_sync_debounce:
        std::collections::HashMap<[u8; 32], std::time::Instant>,
```

In `NodeManager::new()`, add to the `Self { ... }` initializer (after `divergence_detected,`):

```rust
            ns_beacon_sync_debounce: std::collections::HashMap::new(),
```

- [ ] **Step 2: Verify it compiles**

Run: `cargo build -p calimero-node`
Expected: builds clean.

- [ ] **Step 3: Commit**

```bash
git add crates/node/src/manager.rs
git commit -m "feat(node): NodeManager per-namespace beacon-sync debounce map"
```

---

## Task 3: Wire Patch 1 into the beacon handler

**Files:**
- Modify: `crates/node/src/handlers/network_event/readiness.rs`

- [ ] **Step 1: Extend imports**

Change the `tracing` import to include `warn`:

```rust
use tracing::{debug, info, warn};
```

Add (with the other `use` lines):

```rust
use actix::{AsyncContext, WrapFuture};
```

- [ ] **Step 2: Rename `_ctx` → `ctx` and append the divergence trigger**

In `handle_readiness_beacon`, rename the parameter `_ctx: &mut actix::Context<NodeManager>` to `ctx: &mut actix::Context<NodeManager>`.

After the existing `info!( ... "readiness beacon received");` block and before the `if let Some(addr) = &manager.readiness_addr` block, insert:

```rust
    // #2367 — receiver-side anti-entropy. The beacon advertises the
    // peer's namespace governance DAG head; if that head names an op we
    // have not applied, the peer is ahead and we pull the namespace DAG
    // from it via the real governance sync protocol (ops applied in DAG
    // order, side-effects run). A spurious sync is only wasted work,
    // never wrong state. Debounced to one sync per namespace per beacon
    // interval — see `NS_BEACON_SYNC_DEBOUNCE`.
    let dag_head = beacon.dag_head;
    if debounce_allows_sync(
        &mut manager.ns_beacon_sync_debounce,
        namespace_id,
        Instant::now(),
    ) {
        let datastore = manager.datastore.clone();
        let node_client = manager.clients.node.clone();
        let _ignored = ctx.spawn(
            async move {
                let head_op_present = {
                    let handle = datastore.handle();
                    let op_key =
                        calimero_store::key::NamespaceGovOp::new(namespace_id, dag_head);
                    match handle.get(&op_key) {
                        Ok(present) => present.is_some(),
                        Err(err) => {
                            // Unknown local state — do NOT trigger a sync
                            // on a failed read. The next beacon (~5s) retries.
                            debug!(
                                ?err,
                                namespace_id = %hex::encode(namespace_id),
                                "beacon-divergence: local DAG read failed; skipping sync"
                            );
                            return;
                        }
                    }
                };
                if beacon_indicates_divergence(dag_head, head_op_present) {
                    info!(
                        namespace_id = %hex::encode(namespace_id),
                        dag_head = %hex::encode(dag_head),
                        "beacon advertises an unknown namespace DAG head; \
                         triggering governance sync"
                    );
                    if let Err(err) = node_client.sync_namespace(namespace_id).await {
                        warn!(
                            ?err,
                            namespace_id = %hex::encode(namespace_id),
                            "beacon-triggered namespace governance sync failed"
                        );
                    }
                }
            }
            .into_actor(manager),
        );
    }
```

- [ ] **Step 3: Build + test**

Run: `cargo build -p calimero-node && cargo test -p calimero-node --lib network_event::readiness`
Expected: builds clean, tests PASS.

- [ ] **Step 4: Commit**

```bash
git add crates/node/src/handlers/network_event/readiness.rs
git commit -m "feat(node): trigger namespace governance sync on beacon divergence (#2367)"
```

---

## Task 4: Patch 2 — on-subscribe out-of-cycle beacon

**Files:**
- Modify: `crates/node/src/handlers/network_event/subscriptions.rs`

- [ ] **Step 1: Add the `ns/` arm**

In `handle_subscribed`, immediately after the `group/` block's closing `return;` (line ~63) and before `let Ok(context_id): Result<ContextId, _> = topic_str.parse()`, insert:

```rust
    // #2367 — namespace governance topic. A peer just subscribed to
    // `ns/<hex>`; emit an out-of-cycle readiness beacon so the new
    // subscriber sees our namespace DAG head within ~1s instead of
    // waiting up to a full ~5s periodic interval. The
    // `EmitOutOfCycleBeacon` handler no-ops unless we are *Ready in
    // this namespace and rate-limits per (peer, namespace), so this is
    // safe even when the subscribing peer is in a namespace we don't
    // belong to.
    if let Some(hex) = topic_str.strip_prefix("ns/") {
        let mut bytes = [0u8; 32];
        if hex::decode_to_slice(hex, &mut bytes).is_ok() {
            if let Some(addr) = &manager.readiness_addr {
                info!(
                    %peer_id,
                    namespace_id = %hex,
                    "Peer subscribed to namespace topic, emitting out-of-cycle beacon"
                );
                addr.do_send(crate::readiness::EmitOutOfCycleBeacon {
                    namespace_id: bytes,
                    requesting_peer: peer_id,
                });
            }
        }
        return;
    }
```

- [ ] **Step 2: Build**

Run: `cargo build -p calimero-node`
Expected: builds clean.

- [ ] **Step 3: Commit**

```bash
git add crates/node/src/handlers/network_event/subscriptions.rs
git commit -m "feat(node): emit out-of-cycle beacon on ns/ topic subscribe (#2367)"
```

---

## Task 5: e2e regression sentinel — Phase 6

**Files:**
- Modify: `workflows/sync-tests/opaque-leaf-regression.yml`

- [ ] **Step 1: Append Phase 6 after the existing Phase 5 `wait 12s` step**

At the end of `opaque-leaf-regression.yml`, after the `Phase 5 — let materialisation race play out` step, add:

```yaml

  # ── Phase 6: receiver-side anti-entropy (#2367) ─────────────────────
  # Pre-fix: node-2 missed node-1's `ContextRegistered` gossip for the
  # second context on the cold per-namespace mesh and had no recovery
  # trigger — its namespace governance DAG never learns ctx2 exists, so
  # the steps below time out.
  # Post-fix: node-1's periodic `ReadinessBeacon` advertises the new DAG
  # head; node-2's beacon-divergence handler pulls the namespace
  # governance DAG and applies `ContextRegistered`, so node-2 can join
  # the second context and converge.
  - name: Phase 6 — node-2 joins the second context
    type: join_context
    node: sync-regression-node-2
    context_id: "{{ctx2_id}}"
    outputs:
      node2_ctx2_key: memberPublicKey

  - name: Phase 6 — second context converges across both nodes
    type: wait_for_sync
    context_id: "{{ctx2_id}}"
    nodes:
      - sync-regression-node-1
      - sync-regression-node-2
    timeout: 40
    check_interval: 2
```

- [ ] **Step 2: Lint the YAML**

Run: `python3 -c "import yaml,sys; yaml.safe_load(open('workflows/sync-tests/opaque-leaf-regression.yml'))" && echo OK`
Expected: `OK`.

- [ ] **Step 3: Commit**

```bash
git add workflows/sync-tests/opaque-leaf-regression.yml
git commit -m "test(e2e): Phase 6 second-context anti-entropy sentinel (#2367)"
```

---

## Task 6: Workspace verification

- [ ] **Step 1: Build + clippy + test**

Run:
```bash
cargo build -p calimero-node -p calimero-context
cargo clippy -p calimero-node --all-targets
cargo test -p calimero-node --lib network_event
```
Expected: clean build, no new clippy warnings, tests PASS.

- [ ] **Step 2: Code review**

Run `/code-review:code-review` over the diff (`git diff origin/master`). Address findings inline; re-run build/clippy/test after fixes.

---

## Task 7: PR

- [ ] **Step 1: Push**

```bash
git push -u origin fix/2367-namespace-governance-anti-entropy
```

- [ ] **Step 2: Open PR**

Title: `fix(node): receiver-side anti-entropy for missed namespace governance ops (#2367)`
Body: link #2367; explain the cold-start gap; summarize Patch 1 (beacon-divergence sync trigger) + Patch 2 (on-subscribe out-of-cycle beacon); note it supersedes the closed PR #2369 outbox; point at PR #2368 "Bug 3" and mero-drive PR #32 as post-merge verification targets.

- [ ] **Step 3: Monitor CI** — confirm `opaque-leaf-regression` (incl. new Phase 6) and the existing #2356 guards stay green; address bot review threads.

---

## Self-Review

- **Spec coverage:** Patch 1 → Tasks 1-3. Patch 2 → Task 4. Testing item 1 (Rust unit test) → Task 1. Testing item 2 (merobox) → Task 5. Testing items 3-4 (#2368 / mero-drive re-trigger) → post-merge, noted in PR body (Task 7). Out-of-scope items (publisher retry, `NamespaceStateHeartbeat`, `ReadinessProbe`) — untouched. ✔
- **Predicate note:** the spec mentioned an `applied_through > local` secondary check. Op-membership of `beacon.dag_head` subsumes it: if the peer's head op is absent locally the peer is strictly ahead; if present we are equal-or-ahead. Dropping the separate `applied_through` comparison removes ambiguity over whether `NamespaceGovHeadValue.sequence == beacon.applied_through`. This is an intentional, documented simplification.
- **Type consistency:** `NS_BEACON_SYNC_DEBOUNCE` / `beacon_indicates_divergence` / `debounce_allows_sync` / `ns_beacon_sync_debounce` named identically across Tasks 1-3. `NamespaceGovOp::new(namespace_id, delta_id)` and `NodeClient::sync_namespace` signatures verified against source. ✔
- **Placeholder scan:** none.
