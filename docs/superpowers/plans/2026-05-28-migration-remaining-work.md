# Migration — Remaining Work Roadmap

> **For agentic workers:** This is a **roadmap that decomposes into per-PR plans**, not a single executable plan. Each workstream below is shippable on its own and links to (or calls for) its own detailed bite-sized plan. Detailed TDD steps are given for the spec-settled work (PR-3 §A, the policy guard §C1); design-first items (§C3, §C5, §C6) list the open questions to resolve via `superpowers:brainstorming` before a detailed plan is written. Steps use checkbox (`- [ ]`) syntax.

**Goal:** Finish the namespace-cascade migration train (spec PR-3, optional PR-4) and close the migration gaps surfaced while shipping core#2507.

**Architecture:** Builds on the merged train foundation — PR-1 (#2477, write-path fix) and PR-2 (#2493, cascade engine) — plus #2507 (app_key derivation, ContextStorage commit, cross-node convergence + determinism, 14-scenario admin-only e2e matrix). The remaining work adds the cascade **HLC fence + status RPC + sticky `cascade_hlc`** (PR-3), optional recovery (PR-4), and hardening/coverage for the gaps #2507 exposed.

**Tech Stack:** Rust (`crates/context`, `crates/node`, `crates/storage`, `crates/sdk`), merobox e2e workflows (`workflows/app-migration/`), GitHub Actions (`app-migration-e2e.yml`).

---

## Source-of-truth references

- Design spec: `docs/superpowers/specs/2026-05-26-namespace-cascade-migration-design.md` (§3.4 fence, §3.5 write gate, §7 component layout, §8 testing).
- Train state: PR-1 #2477 (merged `32ce0176`), PR-2 #2493 (merged `5b9d368c`), follow-up #2494 (open), merobox#255 (open issue, no PR).
- #2507 (this train-adjacent PR): adds the matrix + the 4 cross-node bug fixes + determinism; **frozen** — do not add roadmap work to it.

## What #2507 already delivered (do NOT re-plan)

- `app_key` derivation from `blob_id` at namespace/subgroup creation + joiner-side re-derivation.
- `ContextStorage` Temporal commit after the migrate WASM run (fresh CRDT collections persist).
- Stale-`application_id` actor-cache refresh in `get_or_fetch_context` (the "Not all bytes read" panic).
- Cross-node sync-gate (`pending_upgrade_target`) — coarse interim protection during a pending upgrade (see §C2).
- Determinism: `#[app::migrate]` runs under storage merge mode (LwwRegister/Element stamps zeroed); `Vector`/`AuthoredVector` elements re-keyed by append index on reassign.
- 14-scenario, 2-node, admin-only-install e2e matrix (node 2 auto-fetches both versions over BlobShare).

## Workstream index & sequencing

| ID | Workstream | Repo | Depends on | Ready? |
|----|-----------|------|-----------|--------|
| **A** | PR-3: cascade HLC fence + `get_cascade_status` RPC + sticky `cascade_hlc` | core | PR-2 (merged) | **Spec-settled — ready to detail/execute** |
| **B** | PR-4: `force_complete_cascade` + multi-version coexistence (optional) | core | A | Scoped; optional/deferred |
| **C1** | Upgrade-policy fail-fast guard (`Automatic`/`Coordinated` + migrate) | core | — | **Ready — small** |
| **C2** | Reconcile sync-gate ↔ HLC fence | core | A | Decide during A |
| **C3** | Populated-during-migrate `RGA`/`Counter` determinism | core/sdk | — | **Design-first (brainstorm)** |
| **C4** | Migrate coverage for `AuthoredMap`/`UserStorage`/`FrozenStorage`/`SharedStorage` | core | merobox steps optional | Ready (scenario work) |
| **C5** | Concurrent-migration e2e | core + merobox | merobox primitive | Design-first |
| **C6** | Failed-migration recovery e2e | core + merobox | merobox primitive | Design-first |
| **D** | merobox#255 step types (`cascade_namespace_application`, `get_cascade_status`, `assert_cascade_complete`) | merobox | A (for status step) | Ready |
| **E** | Reconcile / close follow-up #2494 against #2507 | core | — | **Ready — triage** |

Suggested order: **E** (cheap triage) → **C1** (small, removes a footgun) → **D** (unblocks A's e2e) → **A** (the main piece) → **C2** (folded into A) → **C4** → **B** → **C3/C5/C6** (design-first).

---

## A. PR-3 — cascade fence + status + sticky `cascade_hlc`

**Branch:** `feat/cascade-fence-and-status`. ~400 LOC. Spec §3.3–3.5, §7, §8.

**Goal:** Add the per-context `cascade_hlc` (sticky), the state-delta HLC fence that rejects stale-schema deltas with `UpgradeFenced`, the generalized `InProgress` local write-gate, and a `get_cascade_status` RPC.

### Task A0 (MUST-FIX FIRST): cascade op apply-order bug on receivers

**Found in xilosada's review of #2507 (item #3); a real latent bug in the shipped PR-2 cascade engine.**

**Problem:** `dispatch_cascade` emits `CascadeGroupMigrationSet` then `CascadeTargetApplicationSet` as two separate ops. #2507 fixed the *publisher* emit order, but **receivers can apply them out of order** and reproduce the original bug:
- The two ops chain (`op2.parent_op_hashes = [op1]`), **but** the namespace receive path (`apply_signed_op` → group-op apply) does **not** enforce parent-before-child ordering — group-op ordering rests only on per-signer nonce + `state_hash`.
- `hash_group_state` (`crates/governance-store/src/meta.rs`) hashes `target_application_id` but **NOT `migration`/`app_key`**, so op1 (migration-set) and op2 (target-set) carry the **same** `state_hash` → nothing orders them; a late op1 is then dropped by the nonce guard (`nonce <= last`).
- Independently, the receiver-side cascade apply uses the same `from_app_key == descendant.app_key` predicate. If op2 lands first it rewrites `app_key`, op1's predicate then matches nothing, `GroupMeta.migration` stays unset, and `maybe_lazy_upgrade` has no method to run.
- Only the **cascade** path is exposed; the per-context (non-cascade) path is safe because there target-set changes the hashed field and migration-set is signed against the post-target state, so `state_hash` enforces order.

**Fix (preferred): single atomic cascade op.** Add a combined GroupOp variant (e.g. `CascadeUpgrade { target_application_id, app_key, migration }`) whose apply sets all three in one walk per matched descendant. Eliminates the inter-op ordering dependency entirely. Replaces the two-op emit in `dispatch_cascade`.

- [ ] **Step 1 — failing integration test** (`crates/context/tests/cascade_apply_walk.rs`): apply the cascade ops to a receiver **in reverse order** (target-set before migration-set) and assert each matched descendant ends with BOTH `target_application_id == new` AND `migration == Some(method)`. With today's two-op design this FAILS (migration unset).
- [ ] **Step 2 — run, expect FAIL.**
- [ ] **Step 3 — implement** the combined `CascadeUpgrade` GroupOp variant + apply arm (atomic set of target+app_key+migration per descendant under the `from_app_key` predicate); emit it from `dispatch_cascade` instead of the two separate ops. Keep the wire-format addition behind the same lockstep assumption noted for `app_key`.
- [ ] **Step 4 — run, expect PASS** (order-independent now — single op).
- [ ] **Step 5 — e2e:** extend a 2-node cascade workflow to assert node-2 `GroupMeta.migration` is set + it self-migrates, ideally with reordered/delayed delivery if a merobox primitive allows.
- [ ] **Step 6 — commit:** `fix(context): atomic cascade upgrade op (target+app_key+migration) to fix receiver apply-order`.

**Alternative (if a wire change is undesirable):** include `migration` (and `app_key`) in `hash_group_state` so op1 changes the state hash and op2 (signed post-op1) can't apply before it — but that is a consensus/wire change affecting **every** group's state hash, broad blast radius; prefer the combined op.

> #2507 ships the cascade path (scenario 01) with this latent bug; it didn't surface because the 2-node low-latency e2e delivers in publish order. Called out in the PR review thread; tracked here as the must-fix entry point of PR-3.

### File structure

- Modify: `crates/context/src/handlers/apply_signed_group_op.rs` — record `cascade_hlc = op.hlc` per context in the `Cascade*` apply arms.
- Modify/Add: per-context `cascade_hlc` storage — extend `GroupUpgradeValue` (or a sibling record) with a sticky `cascade_hlc` field; bump the stored type's borsh version.
- Modify: `crates/node/src/handlers/state_delta/mod.rs::apply_authorized_state_delta` — the fence check.
- Modify: `crates/context/src/handlers/execute/mod.rs` (local entry-point) — refuse writes when `GroupUpgradeStatus == InProgress` with `UpgradeInProgress` (generalize the existing pause).
- Create: `crates/context/src/handlers/get_cascade_status.rs` — the RPC.
- Create: `crates/context/src/cascade/hlc_fence.rs`, `state_machine.rs` (+ existing `walk_*` unit modules) for fast unit coverage.
- Test: `crates/context/tests/cascade_status_transitions.rs`, `hlc_fence_integration.rs`.
- Workflows (gated on §D): `workflows/app-migration/03-cascade-with-offline-straggler.yml`, `04-cascade-skip-heterogeneous.yml`, `05-cascade-chain-v1-to-v3.yml`, `06-fence-rejects-straggler-v1-write.yml` (stretch).

### Task A1: sticky `cascade_hlc` storage field

**Files:** Modify the `GroupUpgradeValue` record (locate via `grep -rn "struct GroupUpgradeValue" crates/`); Test: `crates/context/src/cascade/state_machine.rs`.

- [ ] **Step 1 — failing test:** add a borsh round-trip test asserting a `GroupUpgradeValue` with a populated `cascade_hlc: Option<HybridTimestamp>` serialises and deserialises, and that an old-format (no field) payload deserialises with `cascade_hlc == None`.
- [ ] **Step 2 — run, expect FAIL** (field doesn't exist): `cargo test -p calimero-context cascade_hlc_roundtrip`.
- [ ] **Step 3 — implement:** add `cascade_hlc: Option<HybridTimestamp>` to the record with a manual/backward-compatible borsh impl (mirror the `UpgradePolicy` tag-based pattern in `crates/primitives/src/context.rs` if a tagged enum is involved). "Sticky" = never cleared on `Completed`.
- [ ] **Step 4 — run, expect PASS.**
- [ ] **Step 5 — commit:** `feat(context): add sticky per-context cascade_hlc to upgrade record`.

### Task A2: record `cascade_hlc` in the cascade apply arms

**Files:** Modify `crates/context/src/handlers/apply_signed_group_op.rs` (the `CascadeTargetApplicationSet` / `CascadeGroupMigrationSet` arms added in PR-2); Test: `crates/context/tests/cascade_status_transitions.rs`.

- [ ] **Step 1 — failing test:** apply a `CascadeTargetApplicationSet` with `hlc = H`; assert every matched descendant context's stored `cascade_hlc == H` and status `InProgress`.
- [ ] **Step 2 — run, expect FAIL.**
- [ ] **Step 3 — implement:** in each `Cascade*` apply arm, after the existing GroupMeta mutation, write `cascade_hlc = op.hlc` per enumerated context (`enumerate_group_contexts`). Sticky: do not clear on later `Completed`.
- [ ] **Step 4 — run, expect PASS.**
- [ ] **Step 5 — commit:** `feat(context): record cascade_hlc per context on cascade apply`.

### Task A3: HLC fence in the state-delta apply path

**Files:** Modify `crates/node/src/handlers/state_delta/mod.rs::apply_authorized_state_delta`; Add `crates/context/src/cascade/hlc_fence.rs`; Test: `crates/context/tests/hlc_fence_integration.rs`.

- [ ] **Step 1 — failing unit test** (`hlc_fence.rs`): a pure `fn should_fence(delta_app_key, ctx_target_app_key, delta_hlc, cascade_hlc) -> bool` returns `true` iff `delta_app_key != ctx_target_app_key && delta_hlc > cascade_hlc`; cover boundary (`delta_hlc == cascade_hlc` ⇒ false) and matching-app-key bypass (⇒ false).
- [ ] **Step 2 — run, expect FAIL.**
- [ ] **Step 3 — implement `should_fence`** exactly as the two-condition rule in spec §3.4.
- [ ] **Step 4 — run unit, expect PASS.**
- [ ] **Step 5 — wire into `apply_authorized_state_delta`:** load the context's `cascade_hlc`; if `should_fence(...)`, reject with a new structured `UpgradeFenced { cascade_hlc, current_app_key }` error instead of applying.
- [ ] **Step 6 — integration test** (`hlc_fence_integration.rs`): post-cascade stale-schema delta (`app_key` = old, `hlc` > `cascade_hlc`) is rejected; a pre-cascade delta (`hlc <= cascade_hlc`) is accepted; a current-`app_key` delta is accepted.
- [ ] **Step 7 — run integration, expect PASS.**
- [ ] **Step 8 — commit:** `feat(node/sync): HLC fence rejects stale-schema deltas post-cascade`.

### Task A4: generalize the local write-gate to any `InProgress`

**Files:** Modify `crates/context/src/handlers/execute/mod.rs`; Test: add to `crates/context/tests/cascade_status_transitions.rs`.

- [ ] **Step 1 — failing test:** a context whose `GroupUpgradeStatus == InProgress` refuses a local state-op write with `UpgradeInProgress`; a `Completed` context accepts writes.
- [ ] **Step 2 — run, expect FAIL.**
- [ ] **Step 3 — implement:** in the local execute/call path, before a state-op, check the owning group's per-context status; bail `UpgradeInProgress` when `InProgress`. (Spec §3.5 — this is the generalization the cascade status feeds.)
- [ ] **Step 4 — run, expect PASS.**
- [ ] **Step 5 — commit:** `feat(context): refuse local writes while a context upgrade is InProgress`.

### Task A5: `get_cascade_status` RPC

**Files:** Create `crates/context/src/handlers/get_cascade_status.rs` + register the handler/route; Test: `crates/context/tests/cascade_status_transitions.rs`.

- [ ] **Step 1 — failing test:** after a cascade over a namespace with N contexts, `get_cascade_status(namespace_id)` returns a per-context status map; all `Completed` once migration finishes, with any `Failed` preserved.
- [ ] **Step 2 — run, expect FAIL.**
- [ ] **Step 3 — implement:** walk the namespace (`collect_descendant_groups` + `enumerate_group_contexts`), read each context's `GroupUpgradeStatus`, return the map. Read-only; no actor mutation.
- [ ] **Step 4 — run, expect PASS.**
- [ ] **Step 5 — commit:** `feat(context): get_cascade_status RPC reporting per-context migration status`.

### Task A6: C2 reconciliation (see §C2) + e2e workflows (gated on §D)

- [ ] Decide sync-gate vs fence layering (§C2) and adjust `pending_upgrade_target` accordingly; commit.
- [ ] Add cascade e2e workflows `03`/`04`/`05` (and `06` stretch) per spec §8.3, using the merobox `get_cascade_status` / `assert_cascade_complete` steps from §D. Each asserts cross-node convergence + (for `06`) a fence rejection metric.
- [ ] Commit workflows; confirm the `app-migration-e2e` matrix green.

---

## B. PR-4 — cascade recovery (optional, deferred)

**Branch:** `feat/cascade-recovery`. ~100 LOC. Spec §4.4, §8.3 `02`.

**Goal:** `force_complete_cascade --evict-peer X` admin RPC (tombstone a never-returning peer for cascade tracking) + `02-multi-version-coexistence.yml` (namespace A on v1 + namespace B on v2 on one node, neither corrupts the other).

- Create: `crates/context/src/handlers/force_complete_cascade.rs` + meroctl subcommand.
- Workflow: `workflows/app-migration/02-multi-version-coexistence.yml`.
- Acceptance: an evicted peer that reappears is `MembershipRevoked` and re-admit forces full resync; two namespaces on different app versions run independently.
- **Defer** until a real operator need appears; not on the critical path.

---

## C1. Upgrade-policy fail-fast guard (ready, small)

**Why:** `upgrade_group` only early-returns the working lazy path for `LazyOnAccess` (`crates/context/src/handlers/upgrade_group.rs:146`); `Automatic` and `Coordinated` fall through the eager-propagator path which migrates **only on the emitting node** — receivers have no migration trigger (`maybe_lazy_upgrade` is gated to `LazyOnAccess` at `execute/mod.rs:2107`). For a migrate-carrying upgrade under those policies, receivers are left on v1 bytes behind a v2 pointer (the Bug-3 panic). `Coordinated.deadline` is also inert (never read).

**Files:** Modify `crates/context/src/handlers/upgrade_group.rs`; Test: `crates/context/tests/` (or the existing upgrade_group test module).

- [ ] **Step 1 — failing test:** `upgrade_group` with a `migrate_method` set and `upgrade_policy ∈ {Automatic, Coordinated}` returns a clear error (e.g. `UnsupportedUpgradePolicyForMigration`) and does **not** publish a `TargetApplicationSet`/`GroupMigrationSet` op.
- [ ] **Step 2 — run, expect FAIL** (today it silently proceeds).
- [ ] **Step 3 — implement:** before emitting ops, if `migration.is_some()` and policy is not `LazyOnAccess`, bail with the structured error explaining only `LazyOnAccess` supports multi-node migration today (link follow-up). No-migration upgrades under those policies stay allowed.
- [ ] **Step 4 — run, expect PASS.**
- [ ] **Step 5 — commit:** `fix(context): reject migrate upgrades under Automatic/Coordinated until supported`.

> If product wants real `Automatic`/`Coordinated` *migration* later, that's a separate spec (needs a receiver-side eager trigger + real `deadline` enforcement) — brainstorm first.

---

## C2. Reconcile sync-gate ↔ HLC fence (decide during PR-3)

**Context:** #2507's `pending_upgrade_target` sync-gate declines ALL context-state sync (both directions) while a context's `application_id != group.target_application_id`. PR-3's HLC fence is finer: per-delta, rejects only `app_key`-mismatched deltas with `hlc > cascade_hlc`.

**Decision to make (no code until decided):**
- Does the fence **replace** the coarse sync-gate (finer, lets compatible deltas flow), or do they **layer** (gate during the brief InProgress window, fence for the sticky long-tail)?
- Recommended: keep the gate for the `InProgress` window (writes paused anyway per §A4) and let the **sticky fence** handle the post-`Completed` long-tail (stragglers/offline writers) — they cover different time windows. Document the division in `pending_upgrade_target`'s doc comment and the fence's.
- Acceptance: no double-rejection of legitimate post-migration deltas; the admin-only-install matrix (node 2 bootstrap via sync) stays green (the `current_app == ZERO` bootstrap guard added in #2507 must remain).

---

## C3. Populated-during-migrate `RGA` / `Counter` determinism (design-first)

**Today:** documented app-author contract in `crates/sdk/AGENTS.md` (RGA `insert` stamps a raw `env::hlc_timestamp()`; `Counter` increments key by node id) — both diverge if populated inside a migrate. NOT auto-handled like `LwwRegister`/`Vector`.

**Open questions for `superpowers:brainstorming` (resolve before a detailed plan):**
- For `RGA`: can merge mode suppress the raw `hlc_timestamp()` in `insert`, or does the SDK expose a migrate-only `insert_str_at_timestamp`-style deterministic path? RGA `CharId` is `(timestamp, seq)` — what deterministic timestamp is safe under merge mode without breaking live causal ordering?
- For `Counter`: increments are per-replica (node id keyed) by design. Is a deterministic "migrate-time replica id" meaningful, or should migrate be forced to set totals via a different primitive?
- Decision: SDK auto-handling vs. a compile-time/lint warning vs. leaving as documented contract.

Until resolved, the contract in AGENTS.md stands. Do **not** write bite-sized steps until the design lands.

---

## C4. Migrate coverage for other CRDT types (scenario work)

**Goal:** add scenario fixture pairs + workflows that migrate `AuthoredMap`, `UserStorage`, `FrozenStorage`, `SharedStorage` fields (their reassign hooks exist but no scenario exercises a migrate). Follow the existing `apps/migrations/scenario-*-v{1,2}` + `workflows/app-migration/NN-*.yml` pattern (now admin-only-install, 2-node, `lazy_on_access`, both-node lazy-migration log asserts).

- [ ] For each type: add a `scenario-<type>-migrate-v1`/`-v2` crate pair (v2 migrate populates/transforms the field), register in `build-wasms.sh` + workspace `Cargo.toml`, add a 2-node workflow asserting cross-node convergence (and, for `AuthoredVector`/owner-stamped types, that ownership survives — mirror the `storage_type`-preservation unit test from #2507).
- [ ] Confirm each is deterministic cross-node (watch for any `Id::random()`/timestamp entropy the matrix hasn't covered).
- Acceptance: each new scenario green on the matrix; any newly-found non-determinism fixed at the storage/SDK layer (new finding → its own fix).

---

## C5. Concurrent-migration e2e (design-first; needs merobox primitive)

**Goal:** prove two upgrades racing on the same group/context behave (later wins via predicate / fence; no corruption). PR-2's `cascade_concurrent_safety.rs` covers the apply-arm no-match case at unit level; this is the e2e.

**Blockers / open questions:** needs a merobox primitive to drive overlapping `upgrade_group` calls deterministically; define the expected resolution (HLC-fence ordering from PR-3). Brainstorm the test shape before planning.

---

## C6. Failed-migration recovery e2e (design-first; needs merobox primitive)

**Goal:** a migrate fn that traps mid-way → upgrade aborts, v1 intact → retry succeeds. The trap-aborts-leaving-v1-intact behaviour is by design (and #2507's deterministic IDs make a retry overwrite partial entries in place), but the recovery path isn't asserted end-to-end.

**Blockers / open questions:** needs a merobox primitive to inject a failing migrate (e.g. a fixture whose migrate panics on a flag); define what `get_cascade_status` should report (`Failed`) and the admin re-issue path (spec §3.3 `Failed → InProgress`). Brainstorm before planning.

---

## D. merobox#255 — cascade step types

**Repo:** calimero-network/merobox. Issue open, no PR.

- [ ] `cascade_namespace_application` step — wraps the cascade `upgrade_group` RPC (the `cascade: bool` field already shipped in merobox 0.6.23).
- [ ] `get_cascade_status` step — wraps the new core RPC from §A5 (so depends on PR-3).
- [ ] `assert_cascade_complete` step — asserts the status map is all-`Completed`.
- [ ] (Stretch) `set_node_clock_offset` for the §A6 workflow `06` fence-rejection test.
- Release a new merobox version; bump the pin in core CI.

---

## E. Reconcile / close follow-up #2494 (ready — triage)

**#2494** = "re-introduce cascade e2e workflows + v3 fixture once merobox#255 publishes" (descoped from PR-2).

- [ ] Audit what #2507 already shipped against #2494's scope: #2507 added the cascade workflow `01-namespace-cascade-migration.yml` + the v1..v5 chain scenarios (admin-only, 2-node). What #2494 still uniquely needs is the **cascade-specific** workflows `04-cascade-skip-heterogeneous` and `05-cascade-chain-v1-to-v3` (distinct from #2507's per-scenario `04`/`05`), which now live under PR-3 §A6.
- [ ] Either close #2494 as subsumed (redirect its remaining cascade-workflow scope into PR-3 §A6) or trim it to just those two cascade workflows. Comment with the mapping.

---

## Self-review notes

- **Spec coverage:** PR-3 §A covers spec §3.3 (state machine via A2/A4), §3.4 (fence via A3), §3.5 (write gate via A4), §7 (`get_cascade_status` via A5, `cascade_hlc` via A1), §8.1/8.2 (unit + integ tests inline). PR-4 §B covers §4.4 + workflow `02`. e2e workflows `03`/`04`/`05`/`06` in §A6 cover §8.3.
- **#2507 gaps:** C1 (policies), C2 (gate/fence), C3 (RGA/Counter), C4 (other CRDT types), C5 (concurrent), C6 (failed-recovery) — all captured.
- **Placeholders:** design-first items (C3/C5/C6) intentionally carry open-questions instead of fabricated TDD steps; they require `superpowers:brainstorming` before a detailed plan. Everything ready (A, C1, E) has concrete file paths + test shapes.
- **Verify before executing:** exact struct/handler names (`GroupUpgradeValue`, `apply_authorized_state_delta`, the upgrade_group early-return line) should be re-confirmed with `grep` at execution time — line numbers drift.
