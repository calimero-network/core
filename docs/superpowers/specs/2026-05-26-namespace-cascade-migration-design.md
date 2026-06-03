# Namespace-cascade application migration — design

**Status:** brainstorm output, ready for review
**Date:** 2026-05-26
**Scope:** Core-only — calimero-core engine + Rust integration tests + merobox e2e workflows. Excludes app-registry, client SDKs (calimero-client-py / mero-js / mero-react), node UI, and any push-notification surface. Those are explicitly tracked as separate, later work.

## 1. Problem

Today's `upgrade_group` flow targets one group at a time. To migrate an entire namespace (the parent group plus all descendant subgroups and their contexts), an operator has to issue N separate `upgrade_group` RPCs. Even worse, the per-context migration write path itself is broken since #2433 changed `Root<T>` merge semantics — `write_migration_state` calls `Interface::save_raw`, which now bails with `MergeFailure(NoMergeFunctionRegistered)` because the host registry intentionally no longer holds app-type merges (those live in the WASM-side registry since #2465).

We need two things to land together:

1. **A working per-context migration write path** — without this, no migration succeeds anywhere, cascade or not.
2. **Namespace-level cascade migration** — one RPC migrates a namespace and every descendant subgroup that's currently on the same source application; descendants on a different application are left alone (heterogeneous deployments are first-class).

In parallel we need to address the corruption hazard during in-flight migration on a peer-to-peer mesh: a peer mid-migration shouldn't accept user writes against the soon-to-be-stale schema, and a long-offline peer shouldn't be able to push stale-schema deltas into already-migrated peers.

## 2. Out of scope

These are real follow-on work but not part of this design:

- App-registry surfaces (where the admin sees "update available")
- Client SDK changes (calimero-client-py / mero-js / mero-react)
- Node UI / push notifications for cascade events
- App-side delta-replay-through-migration helpers
- Automatic retry policies beyond blob-fetch backoff

## 3. Core model

### 3.1 Op shape (already merged in #2452)

```rust
GroupOp::CascadeTargetApplicationSet {
    from_app_key: [u8; 32],       // matching predicate
    app_key: [u8; 32],             // new app_key
    target_application_id: ApplicationId,
}
GroupOp::CascadeGroupMigrationSet {
    from_app_key: [u8; 32],
    migration: Option<Vec<u8>>,
}
```

`from_app_key` is the matching predicate the receiver applies during its local tree walk. `app_key` is the content-derived identifier of the new application binary; `target_application_id` is the WASM blob hash. Both Cascade variants are signed under the namespace's governance authority — descendant capabilities are not re-checked, because the cascade is by definition top-down.

### 3.2 Apply algorithm (eager, walk-and-write)

On every receiving node, the apply handler does:

```
walk = {signed_group_id} ∪ collect_descendant_groups(signed_group_id)
for each G in walk:
    if GroupMeta(G).app_key != op.from_app_key: skip   # heterogeneous safe-skip
    GroupMeta(G).target_application_id = op.target_application_id
    GroupMeta(G).app_key                = op.app_key
    record per-context cascade_hlc = op.hlc
    for each C in enumerate_group_contexts(G):
        GroupUpgradeStatus(C) = InProgress { cascade_hlc: op.hlc }
        enqueue_migration_propagator(C)         # async; one propagator task per context
```

The `enqueue_migration_propagator` step does not block the apply handler — it spawns or hands off to the existing per-context propagator. The apply handler returns once GroupMeta + status writes have committed; per-context migration progresses concurrently and reports completion through status transitions.

Both `CascadeTargetApplicationSet` and `CascadeGroupMigrationSet` walk with the same `from_app_key` predicate, guaranteeing they affect the same set on every node.

Convergence guarantee: every node walks its own local tree with the same predicate against the same signed op, so every node identifies the same affected set. The migration function is deterministic, so independent local migration on each node yields identical post-migration state.

### 3.3 Per-context state machine

```
                              ┌──────────────┐
                              │   Pending    │
                              └──────┬───────┘
                                     │ propagator picks up
                                     ▼
                ┌────────────────────────────────────┐
                │   InProgress { cascade_hlc }       │ writes refused
                └──┬──────────────┬──────────────────┘
                   │              │            │
       blob/meta   │              │ migrate    │ migrate
       missing     │              │ ok         │ errors
                   ▼              ▼            ▼
        ┌─────────────────┐    ┌───────────┐  ┌──────────────────┐
        │ WaitingForBlob  │    │ Completed │  │     Failed       │
        │ (auto-retry)    │    │           │  │ (no auto-retry)  │
        └─────────┬───────┘    └───────────┘  └────────┬─────────┘
                  │ blob arrives                       │ admin re-issues cascade
                  └────────► InProgress                └──► InProgress
```

`cascade_hlc` is sticky — retained on the context after `Completed` so the HLC fence keeps protecting against any later-arriving stale-schema delta.

### 3.4 HLC sync fence

Per-context, the receiver tracks `cascade_hlc[ctx] = HLC of most recent Cascade* op applied`. The fence sits in the state-delta apply path (`crates/node/src/handlers/state_delta/mod.rs::apply_authorized_state_delta`). On incoming delta:

```
if delta.app_key != ctx.target_application_id
   && delta.hlc > cascade_hlc[ctx]:
    reject with UpgradeFenced { cascade_hlc, current_app_key }
```

The sender's sync layer surfaces this as a structured error. Core's job stops there; how the app SDK handles the rejection (buffer, replay, surface to user) is app-side policy and not part of this design.

**Note on HLC as a causality proxy.** HLC alone is a best-effort approximation of causality, not a strict one — a peer with a forward-skewed clock can produce a delta whose HLC > cascade_hlc even though the write is causally independent of (or before) the cascade. The fence's two-condition rule (`app_key mismatch AND HLC > cascade_hlc`) is calibrated for the common case where clock drift is bounded. Genuine clock-pathology scenarios surface as `UpgradeFenced` for legitimate writes; the user re-does them post-migration. A future refinement could replace the HLC comparison with strict DAG causal-ancestor checks; for this design HLC is sufficient and avoids dragging the DAG walker into the delta-apply hot path.

### 3.5 Local write gate

A generalized invariant in the local execute/call handler: any context whose `GroupUpgradeStatus == InProgress` refuses local writes with `UpgradeInProgress`. This is the same write-pause that today's single-group `UpgradePolicy::Coordinated` aims for, generalized so it triggers from a cascade-set status as well.

This invariant is broader than cascade — it's the right rule for any in-flight structural change. Implementing it as a per-context status check rather than a cascade-specific check is a small generalization that comes free.

## 4. Late-peer handling

### 4.1 Offline at cascade time, comes back caught-up

The governance DAG is causally ordered, and governance ops are processed before state deltas during catch-up. So:

1. Peer syncs governance DAG, observes the cascade op.
2. Apply handler walks its local tree, marks affected contexts `InProgress`, pauses local writes.
3. Peer fetches v2 WASM blob via DHT (content-addressed by `target_application_id`).
4. Per-context propagator runs `migrate_method` against local state.
5. Status flips to `Completed`, writes resume, queued v2-shaped deltas merge.

### 4.2 Offline writes during the cascade window

If the user made writes on a peer while it was offline, two HLC sub-cases:

| HLC ordering | Outcome |
|---|---|
| `T_user_write < T_cascade` | Offline writes happened before the cascade in causal order. They merge into local v1 state first, cascade applies, `migrate` runs over the merged-in-offline-writes state. **Preserved.** |
| `T_user_write > T_cascade` | Offline writes happened after the cascade in causal order. Migrated peers reject them via the HLC fence. **User must re-do them post-migration.** |

The lossy case is real and we should be honest about it. Three mitigations:

- **Make it rare**: pause local writes the moment a peer reconnects, *before* it finishes syncing governance, so it can't produce stale-schema writes during catch-up.
- **Surface the rejection**: `UpgradeFenced { from_version, to_version, cascade_hlc }` is structured so the app SDK can render "your changes from yesterday couldn't be applied because the app was upgraded".
- **Document it**: this is the cost of p2p consistency; centralized systems hide it by serializing through a server, we expose it explicitly.

### 4.3 Multiple cascades while offline

The DAG enforces causal order. A peer offline through cascades v1→v2 and v2→v3 processes them sequentially on catch-up: apply v1→v2 cascade, run `migrate_v1_to_v2`, apply v2→v3 cascade, run `migrate_v2_to_v3`. Both binaries are fetched on demand from DHT.

### 4.4 Peer never comes back

`get_cascade_status` reports the peer as `Pending` indefinitely. The namespace stays fully usable for everyone else (no global wedge — each node's pause is local). Recovery knobs:

- **Default**: do nothing. The pending entry is informational.
- **`force_complete_cascade --evict-peer X`** *(PR-4 / later)*: tombstone X for cascade tracking. If X reappears, it gets `MembershipRevoked` and the admin re-admits, forcing full resync including the cascade.

## 5. Concurrent cascade safety

Two admins issue overlapping cascades (e.g. v1→v2 and v1→v3 racing). The DAG picks one as causally first on every receiver (same one everywhere, since the DAG is convergent). The loser arrives with `from_app_key == v1`, but local state already shows `app_key == v2` — predicate skip makes the loser a no-op. The `from_app_key` predicate doubles as optimistic-concurrency control; no new locking primitive needed.

## 6. Failure handling

| # | Failure | Detection | Recovery | What admin sees |
|---|---|---|---|---|
| 1 | `migrate_method` panics / returns Err | execute_migration returns Err | Context → `Failed`; **no auto-retry** | per-context `Failed { error_msg }` |
| 2 | v2 WASM blob unavailable | DHT fetch timeout | Auto-retry exponential backoff; cap → `Failed` | `WaitingForBlob { blob_id, since }` |
| 3 | `write_pre_merged_root_state` errors | Storage layer bubble | Stay `InProgress`; admin escalation | error log + `Failed` |
| 4 | Incoming stale-schema delta hits fence | Sync delta-apply gate | Reject with `UpgradeFenced` | metric `cascade_fence_rejections_total{ctx}` |
| 5 | Descendant deleted between op-publish and op-apply | Walk hits missing GroupMeta | Silent skip | debug log |
| 6 | Target ApplicationMeta missing | Apply handler read | DHT fetch on demand | `WaitingForBlob` |
| 7 | Cap changes mid-cascade (signer no longer authorized) | Sig verification at recv | Rejected before apply | per-peer log; cascade is partial |
| 8 | Per-context partial failure | Per-context status diverges | **No rollback** (see 6.1) | mixed map in `get_cascade_status` |
| 9 | Cascade op for deleted signed group | Validation at apply | Drop with warning | warn log |
| 10 | `migrate` writes malformed bytes | Caught at first read post-migration | Context unreadable; status retroactively → `Failed` | read errors + status flip |

### 6.1 Why no rollback on partial failure

If migration succeeds on C1+C2 but fails on C3, we do not undo C1/C2:

1. Storage cost — rollback would require keeping pre-migration state alongside post-migration for the full cascade window. Doubles storage during cascades.
2. C1/C2 are valid v2 state. Undoing them just means re-running the same migrate later. Identical result, wasted cycles.
3. C3 is the actual problem. It sits in `Failed`; admin sees it in `get_cascade_status` and decides: retry, abandon, or ship a patched app.

Trade-off: the namespace can sit in mixed-version state indefinitely. Users on C1/C2 see v2; users on C3 see v1 (writes blocked). This is honest about p2p reality — there is no global transaction abort.

## 7. Component layout

| Surface | File | New / Modified |
|---|---|---|
| Cascade op emission | `crates/context/src/handlers/upgrade_group.rs` | Modified: `cascade: bool` field; emit `Cascade*` ops when true |
| Cascade op apply | `crates/context/src/handlers/apply_signed_group_op.rs` | Modified: new arms for both `Cascade*` variants |
| Per-context propagator | `crates/context/src/handlers/upgrade_group.rs` | Unchanged: cascade apply calls existing per-context dispatch |
| Migration write fix | `crates/context/src/handlers/update_application/mod.rs:705` | Modified: `save_raw` → `write_pre_merged_root_state` |
| HLC sync fence | `crates/node/src/handlers/state_delta/mod.rs::apply_authorized_state_delta` | Modified: fence check on incoming delta |
| Local write gate | `crates/context/src/handlers/execute.rs` (or local entry-point) | Modified: refuse writes when status `InProgress` |
| `get_cascade_status` RPC | `crates/context/src/handlers/get_cascade_status.rs` | New |
| `force_complete_cascade` RPC | `crates/context/src/handlers/force_complete_cascade.rs` | New, deferred to PR-4 |
| Per-context `cascade_hlc` storage | `GroupUpgradeValue` or sibling record | Field add |

Reuses without change: `collect_descendant_groups`, `enumerate_group_contexts`, `GroupOp::Cascade*` (#2452), `migration-suite-v{1..5}` fixtures, `GroupUpgradeStatus::{InProgress, Completed}`, DHT blob announce/fetch.

## 8. Testing strategy

Three tiers; each targets failure classes the others can't catch.

### 8.1 Unit (fast, synthetic — `crates/context/src/cascade/...`)

| File | Targets |
|---|---|
| `walk_predicate.rs` | `from_app_key` equality; skip on mismatch; signed group included |
| `walk_depth_bound.rs` | `MAX_NAMESPACE_DEPTH` enforced; cycle-detection |
| `state_machine.rs` | Valid transitions; reject illegal |
| `hlc_fence.rs` | Boundary inclusivity; matching-app-key bypass |

### 8.2 Rust integration (`crates/context/tests/`)

| File | Proves |
|---|---|
| `cascade_apply_walk.rs` | Single signed op → every matched descendant updated; heterogeneous left alone |
| `cascade_status_transitions.rs` | Contexts flow Pending → InProgress → Completed; failure preserved |
| `cascade_concurrent_safety.rs` | Two cascade ops out of order: later one no-op via predicate |
| `hlc_fence_integration.rs` | Post-cascade stale-schema delta rejected; pre-cascade accepted |
| `migration_regression.rs` | **#2433 regression guard.** Single-context migrate via `write_pre_merged_root_state` succeeds |

### 8.3 e2e merobox (`workflows/app-migration/`)

| Workflow | Setup | Proves |
|---|---|---|
| `00-single-group-migration-baseline.yml` | 1 node, 1 group, 1 context, v1→v2 single-group | #2433 fix unblocks today's flow |
| `01-single-namespace-cascade.yml` | 2 nodes; namespace with 2 subgroups × 2 contexts; cascade v1→v2 | All 4 contexts migrated on both nodes; state preserved; `get_cascade_status` all-Completed |
| `02-multi-version-coexistence.yml` *(PR-4)* | 1 node; namespace A on v1 + namespace B on v2 | Both run independently, neither corrupts the other |
| `03-cascade-with-offline-straggler.yml` | 3 nodes; node-3 stopped during cascade, restarted | Catch-up migration runs; convergence; no fence rejections |
| `04-cascade-skip-heterogeneous.yml` | NS with subgroup A on v1 + B already on v2; cascade v1→v3 | A migrates; B skipped via predicate |
| `05-cascade-chain-v1-to-v3.yml` | 2 nodes, namespace, 1 context; cascade v1→v2 then v2→v3 | Both apply in DAG order on both nodes |
| `06-fence-rejects-straggler-v1-write.yml` *(stretch)* | Node-3 writes locally post-cascade-HLC, reconnects | Migrated peers reject via fence; metric increments |

### 8.4 New merobox step types

- `upgrade_group` step: add `cascade: bool` field (one-line additive change once core RPC accepts it).
- `get_cascade_status` step: new, wraps the new core RPC.
- *(Stretch)* `set_node_clock_offset` for workflow 06.

Stop/start node for workflow 03 already exists in merobox (opaque-leaf regression workflows use it).

### 8.5 CI shape

One new GHA job `.github/workflows/app-migration-e2e.yml`, modeled on `sync-regression.yml`: builds merod:local, builds migration-suite fixtures, iterates `workflows/app-migration/*.yml` sequentially (each nukes-on-start), captures per-node docker logs as artefact. Paths-filtered to migration-touching paths.

## 9. Delivery plan

### 9.1 Cleanup first

| Item | Action |
|---|---|
| PR #2449 (draft) | Close with comment: "superseded by cascade-migration PR train; workflows re-derived to cover cascade-aware testing". Salvage GHA job + `build-wasms.sh` into PR-1. |
| `.worktrees/feat-app-migration-coverage` | Remove |
| `.worktrees/feat-migration-overwrite-intent` | Remove |
| Stale local branches | Delete |

### 9.2 PR train (5 PRs across 2 repos)

```
   merobox PR  ──►  Core PR-1  ──►  Core PR-2  ──►  Core PR-3  ──►  Core PR-4
   (steps)         (write fix +     (cascade RPC +    (fence +         (recovery,
                    workflow 00)     workflows         get_status,      workflow 02,
                                     01,04,05)         workflows        defer)
                                                       03,06)
```

**merobox PR** (calimero-network/merobox): cascade step support + get_cascade_status step + (stretch) set_node_clock_offset.

**Core PR-1** (`fix/migration-write-pre-merged`, ~150 LOC): write-path fix + Rust regression test + e2e workflow 00 + CI job + helpers.

**Core PR-2** (`feat/cascade-engine`, ~600 LOC): cascade RPC + apply handler + local write gate + Rust integ tests + workflows 01, 04, 05. Depends on PR-1 + merobox.

**Core PR-3** (`feat/cascade-fence-and-status`, ~400 LOC): HLC fence + `get_cascade_status` RPC + per-context `cascade_hlc` storage + unit/integ tests + workflows 03 (and 06 if feasible). Depends on PR-2.

**Core PR-4** (`feat/cascade-recovery`, ~100 LOC, optional): `force_complete_cascade` + workflow 02. Defer.

### 9.3 Cross-repo ordering

To avoid stuck-CI:

1. Open merobox PR, get merged, publish, bump pin in core.
2. Core PR-1 — green on existing merobox features.
3. Core PR-2 — green after merobox pin bump.
4. Core PR-3 — green.
5. Core PR-4 — when needed.

## 10. Explicit non-goals

- No registry-side "update available" surface.
- No client SDK changes.
- No node UI / push notifications for cascade events.
- No app-side delta-replay-through-migration helpers.
- No automatic retry beyond blob-fetch backoff.
- No rollback on partial cascade failure.

These are real follow-on work but are separate PR trains and out of scope for this design.
