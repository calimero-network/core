# PR-6 — Zero-Downtime Migration Framework (Hybrid Design, v2)

- **Date:** 2026-06-03
- **Status:** Design approved (brainstorm); revised after adversarial code review (11 critical + 13 high findings folded in)
- **Issues:** core #2539 (umbrella); folds in #2534 (owner-driven Authored/Shared rewrite); #2550 (`#[derive(Migrate)]`) descoped to ergonomics (see §4F)
- **Builds on (merged):** #2585, #2582, #2586 (L2 abi-diff), #2645 (L1 embed-ABI + identity-downgrade gate), #2644 (reads-available during upgrade), #2524 (atomic CascadeUpgrade + sticky cascade_hlc + get_cascade_status)
- **Grounded against master:** `f25cab38`

---

## 1. Problem

Today a migration is a **stop-the-world whole-root rewrite**. `execute_migration` (`crates/context/src/handlers/update_application/mod.rs:504`) runs the wasm `migrate()` in `with_merge_mode`, then `write_migration_state` (`:756`) commits the entire new root via `Interface::write_pre_merged_root_state` (`crates/storage/src/interface.rs:2676`) and sets `dag_heads = vec![root_hash]` (`:455`) — no causal delta. While a cascade is `InProgress`, the execute gate (`execute/mod.rs:115-174`, `upgrade_blocks_write` `:2154`) blocks **writes** (reads were freed in #2644). The HLC straggler fence (`crates/context/src/hlc_fence.rs:20`; drop at `crates/node/src/handlers/state_delta/mod.rs:654-678`) **silently drops** stale-schema deltas — data loss for a node offline across the upgrade.

**Goal:** near-zero-downtime migrations (reads always available; writes paused only briefly per-context, not cluster-wide), straggler-safe (no silent drop), with admin visibility — and **no quorum/voting** (a hard #2539 non-goal).

## 2. Architecture decision — HYBRID

The adversarial review established that **per-entity lazy conversion fights the Merkle-convergence model**: rewriting any entity's bytes diverges its leaf hash and competes in last-writer-wins (LWW), and the host cannot transform app-typed bytes (only the wasm binary can). The deterministic whole-root rebuild avoids all of that. So we split by CRDT category:

- **Convergent + Replayable** (re-derivable data): keep the **existing deterministic whole-root migrate** (`execute_migration`) — every node re-derives a byte-identical v2 root from its own v1 state, so there is no cross-node byte divergence and no host-side translation. We make it **non-freezing-ish** (per-context, brief; reads served via #2644) instead of a cluster freeze, and **straggler-safe** (6b). Its existing **clean-rollback** property (v1 root untouched until the final commit) gives us a real abort for free (6d).
- **Identity-gated** (`AuthoredMap`/`AuthoredVector`/`SharedStorage`, signed per-entry): cannot be rebuilt by a non-owner (crypto). Handled by **owner-driven, per-entry, online signed re-write**, lazily — the genuinely hard part, now isolated.

This is the unifying insight from review themes A (convert-is-a-real-write) and B (only-wasm-can-transform): apply the whole-root rebuild where it converges cleanly, and pay the per-entry cost only where ownership forces it.

## 3. Locked decisions

1. **Stacked PR train** 6a → 6b → 6c → 6d behind a feature flag, each independently reviewable/mergeable (the cascade train shipped as #2477→#2493→#2524). `/code-review` after each.
2. **Hybrid** category split (§2).
3. **Convergent/Replayable**: deterministic whole-root rebuild, made non-freezing per-context; existing `#[app::migrate]` author surface (no per-entity tag, no host translation).
4. **Identity-gated**: owner-driven, per-entry, **online signed** re-write with a **strictly-monotonic nonce** (NOT entropy-suppressed, NOT "byte-identical across nodes" — correctness comes from it replicating as one owner's normal signed `Action::Update`). Departed owners → **admin force-carry = tombstone-old + new-entity-under-admin-key** (not an in-place re-sign, which is cryptographically impossible).
5. **Straggler safety (6b)**: replace the fence's silent drop with **absorb = buffer the original signed bytes and replay them verbatim** through the receiver's current wasm `__calimero_sync_next` (translating bytes would break per-action signatures). Durable buffer in a new store column with restart recovery. Coverage must include the **HashComparison / LevelSync / snapshot** sync-repair paths, not just the gossip fence.
6. **Binary-version fence**: the fence must key on the **receiver's locally-loaded binary/reader version**, not the replicated governance `app_key` (under LazyOnAccess the governance `app_key` advances for all members at cascade-apply, but each node's wasm binary swaps lazily — so a v1-binary node can otherwise be fed unreadable v2 bytes by ordinary sync and corrupt on read).
7. **Completion visibility (6c)**: per-node **signed migration heartbeat** as an **ephemeral TTL gossip beacon** (a new `NamespaceTopicMsg` variant modeled on the existing `ReadinessBeacon`, NOT replicated governance state). `get_migration_status(namespace)` rolls it up over admin HTTP. `expected_members` = the **inherited-membership closure over the subtree** (reuse the #2371 `list ∪ enumerate_inherited` across `collect_descendants`), with the cohort **pinned at expand-entry** (governance HLC cutoff) so mid-migration joins/leaves don't flip the signal. Observability, **never a gate** on correctness.
8. **Residue** = a **local derived scan** (count of locally-unconverted identity-gated entries, or equivalently `target - |converted-id set|`), NOT a replicated shrink-CRDT (the only counter double-counts under concurrent convert). For Convergent/Replayable the per-context version *is* the residue (a context is atomically v1 or v2).
9. **Soft contract**: stop work + mark converged when local residue==0 + cohort heartbeats show all-on-v2 + soak elapsed. **Keep the old reader** (absorb stays intact). Hard reclamation deferred.
10. **6d**: app-exported `migration_check(old_root,new_root)->bool` run on the **produced v2 root before commit** (clean because the whole-root path never mutates v1 until commit); failed check → **logical abort** (discard the produced v2 root / flip the schema target back; keep all committed user data); **admin abort RPC**. **No byte snapshot/restore** (none exists; #2644 is a write-freeze, not a checkpoint; replicated deltas can't be recalled).

## 4. Corrections folded in from the adversarial review

| # | Finding (sev) | Resolution |
|---|---|---|
| A | convert-as-LWW-write; equal-ts content-hash coin-flip; nested-id divergence (3×crit/high) | Hybrid: Convergent/Replayable use whole-root rebuild (existing determinism machinery), no per-entity convert. |
| B | host can't translate; translate breaks signatures; sweeper host→wasm; dual-read no runtime type (4×crit/high) | No host translation. Whole-root migrate runs in wasm. Absorb replays **original signed bytes**. Identity-gated translation lives in the v2 binary. |
| C | snapshot/rollback contradiction; no COW; can't recall replicated deltas (3×) | **Logical abort** only. Whole-root path keeps clean-rollback (v1 untouched pre-commit). Drop all "snapshot restore" language. |
| D | force-carry cryptographically impossible; owner-convert determinism vs nonce (2×) | Force-carry = **tombstone + new-entity-under-admin-key**. Identity-gated convert = **owner-online signed, monotonic nonce**, not entropy-suppressed. |
| E | residue not a CRDT; heartbeat not replicated; expected_members inherited; cohort flip; v1-binary fed v2 bytes; HashComparison bypasses fence (7×) | Residue = local scan. Heartbeat = TTL gossip beacon. expected_members = inherited closure, cohort pinned. Fence on **loaded binary version** + cover sync-repair paths. |
| F | mega-PR unreviewable; derive macro is the registry producer (2×) | **Stacked train.** `#[derive(Migrate)]` descoped to ergonomics for the identity-gated author surface (not an L3 lint — a single-crate derive can't see the old schema; the rail stays **L1 #2645 + L2 #2586**). |

## 5. Phases (stacked train)

### PR-6a — non-freezing whole-root migrate (Convergent/Replayable)
- Make `execute_migration` / the `InProgress` write-gate non-cluster-freezing: the deterministic rebuild runs **per-context** with a **brief local write-pause** only (reads served from the committed root via #2644). See **O1** for the freeze-vs-shadow trade-off.
- Reuse the existing `#[app::migrate]` whole-root author surface; no per-entity tag for this category.
- Feature-flag so the current path stays default until 6a is proven.

### PR-6b — absorb-don't-drop (straggler safety)
- Replace the fence's silent drop (`hlc_fence.rs`; `state_delta/mod.rs:654-678`) with **buffer-then-replay-verbatim**: persist the original `BufferedDelta` (it already carries every replay field) in a **new `Column::AbsorbBuffer`** keyed `context(32)‖producing_app_key(32)‖delta_id(32)`, with a startup recovery scan (mirror `UpgradesRepository::enumerate_in_progress`). On binary/app_key advance, re-feed the **original signed bytes** through `__calimero_sync_next` (no translation), then let the deterministic whole-root migrate fold them in.
- **Fence on loaded binary version** (decision 6); a node lacking a reader for incoming schema **buffers** rather than writes.
- Extend coverage to **HashComparison / LevelSync / snapshot** apply paths (`crates/node/src/sync/helpers.rs:227-358`), carrying a per-leaf schema/version so a v1-binary receiver declines+buffers v2 leaves instead of storing unreadable bytes.

### PR-6c — identity-gated owner-driven re-write + completion visibility
- **Owner-driven convert**: when the owner's v2 binary next writes an identity-gated entry whose stored shape is old, it re-writes+re-signs in the new shape as a **normal online signed `Action::Update` with a strictly-monotonic nonce** (`updated_at`). Per-entry dispatch keyed on `(crdt_type/field_name, schema_version)`; only identity-gated entries carry a `schema_version` tag (Merkle-invisible, `Metadata.schema_version`).
- **Departed owners**: admin **force-carry** = governance-authorized **tombstone of the old entry + new entity under the admin's own key** (verifies normally; sidesteps the "can't change owner / can't forge signature" walls). Trigger/authorization lives in the governance layer.
- **Residue** = local derived scan of un-converted identity-gated entries (decision 8).
- **Heartbeat + `get_migration_status`** (decisions 7): new `NamespaceTopicMsg::MigrationHeartbeat(Signed…)` → in-memory TTL cache (model on `ReadinessCache`); rollup reads cache + local `UpgradesRepository` (target) + inherited membership closure (expected). Staleness is first-class (`unknown`). Admin HTTP route mirrors `get_cascade_status` (`server/src/admin/service.rs:231`).

### PR-6d — soak + migration_check + logical abort
- App-exported `migration_check(old_root, new_root) -> bool` (built-in helpers: entity-count parity, no-orphaned-refs, conservation). Run on the **produced v2 root before commit**; pass → commit; fail → **logical abort** (discard produced root / flip target back; the whole-root path never touched v1, so committed user data is intact).
- **Admin abort RPC.** No byte snapshot. API shaped so canary-subgroup gating drops in later.

## 6. Author surface

Convergent/Replayable: **existing `#[app::migrate]`** (whole-root, reads old via `read_raw`, returns new root). Identity-gated: a thin `#[derive(Migrate)]`/attribute generating the per-type old-reader + `migrate(old)->new` + version bump for the owner-driven path — **ergonomics only**, no compile-time L3 downgrade lint (a single-crate derive can't see the old schema; the no-silent-downgrade rail remains **L1 core gate #2645 + L2 CI diff #2586**, optionally a runtime no-silent-downgrade panic in merge mode).

## 7. Determinism & safety invariants (corrected, split by category)

1. **Convergent/Replayable**: whole-root migrate is deterministic (existing `__assign_deterministic_ids` + `with_merge_mode`); every node re-derives a byte-identical v2 root. No per-entity byte divergence.
2. **Identity-gated**: convert is an **owner-online signed write with a strictly-monotonic nonce**; correctness comes from single-owner authorship replicating as a normal convergent delta — NOT cross-node byte-identity, NOT merge-mode.
3. **No node persists/merges bytes of a schema version newer than its loaded reader**; such deltas/leaves are buffered (6b) until the binary swaps.
4. **Schema tag (identity-gated only) is Merkle-invisible** (`schema_version` not hashed).
5. **Abort is logical**: never restores bytes, never recalls replicated deltas; the whole-root path's clean-rollback (v1 untouched pre-commit) is the only "rollback."
6. Entry-before-index ordering preserved (#2319). Correctness is independent of completion knowledge (convergence + absorb), which is observability only.

## 8. Completion & visibility (the "how does the admin know?" answer)

Automatic correctness needs no admin (convergence + absorb). For *visibility*, `get_migration_status(namespace)` returns per-node state + a rollup with `all_migrated` (true ⟺ every **pinned-cohort** member reported v2 + residue 0); offline/unreachable nodes show `unknown` and keep it `false` — no false green. The heartbeat is ephemeral TTL gossip (decision 7); the rollup is a snapshot of one node's gossip view, explicitly non-authoritative, and the irreversible hard-contract (deferred) additionally requires explicit force-carry/exclude of every `unknown` member, never `all_migrated` alone.

```jsonc
GET /admin/contexts/migration-status/{namespace_id}
{ "target_version":"v2","expected_members":12,
  "rollup":{"migrated":9,"in_progress":2,"unknown":1,"total":12,"all_migrated":false},
  "cohort_pinned_at_hlc":"…", "members":[ /* per-node version, residue_auto, residue_identity, synced_up_to_hlc, reported_at, state */ ] }
```

## 9. Open problems to resolve during implementation (not blockers, but design-care)

- **O1 — non-freezing whole-root for *writes* — DECIDED: brief per-context write-pause (O1-a).** First cut pauses writes to ONLY the context being rebuilt, for ONLY its rebuild duration (≈ms typical; reads stay live via #2644). No cluster-wide freeze. The per-context **shadow keyspace** (true zero write-downtime even for very large contexts) is a deferred, cleanly-layered follow-up (O1-b), pulled forward only if large-context pauses prove painful.
- **O2 — absorbing a straggler's late v1 delta into a v2 context**: replay-verbatim then deterministic whole-root re-fold vs version-aware merge. Detail in 6b.
- **O3 — threading the *loaded binary version*** into `hlc_fence::delta_is_fenced`, `save_internal`, and the sync-repair apply paths. Detail in 6b.
- **O4 — identity-gated owner-driven nonce/merge semantics** and the governance authority for the force-carry tombstone+rekey op. Detail in 6c.

## 10. Non-goals / deferred

Hard contract (aggressive reclamation + v3 old-reader-removal); full canary-subgroup gating; per-entity lazy conversion for Convergent/Replayable (the hybrid deliberately avoids it). No quorum/voting; no change to the deterministic-execution / CRDT-merge model.

## 11. Testing surface

Extend merobox `workflows/app-migration/` (today 00–22): non-freezing whole-root migrate (writes only briefly paused, reads available); **straggler-absorb** (node offline across the window → absorbed, not dropped; incl. a **v1-binary node fed v2 bytes via HashComparison** → buffers, not corrupts); owner-driven authored re-write (owner returns); departed-owner tombstone+rekey force-carry; `migration_check` pass (commit) and fail (logical abort); admin abort RPC; mixed v1/v2 coexistence; `get_migration_status` rollup with `unknown`/cohort-pinning assertions. Unit/integration in `storage`, `context`, `node` (sync paths), `governance-store`, `sdk-macros`. Two-node determinism test for the whole-root path.

## 12. File-touch map (per PR)

- **6a**: `context/src/handlers/{update_application/mod.rs, execute/mod.rs}` (non-freezing gate), feature flag.
- **6b**: `context/src/hlc_fence.rs`, `node/src/handlers/state_delta/mod.rs`, `node/src/sync/{helpers.rs,hash_comparison*,level_sync,snapshot}.rs`, **new `store/src/db.rs` `Column::AbsorbBuffer`** + repository + recovery; loaded-binary-version plumbing.
- **6c**: `storage/src/{entities.rs (schema_version), interface.rs (owner-driven apply path)}`, `governance-types/src/{lib.rs (force-carry op), wire.rs (MigrationHeartbeat)}`, `governance-store/*` (residue scan, force-carry apply), `node/src/readiness.rs`-style TTL cache, `context/src/handlers/get_migration_status.rs` (new), `server/src/admin/service.rs` (route), `sdk/macros` (`#[derive(Migrate)]` ergonomics).
- **6d**: `sdk/macros` + `sdk/src` (`migration_check` export), `context` (soak + logical abort + admin abort RPC).
