# Migrations — LIVE STATUS (single source of truth for hand-off)

> **Purpose:** one doc any agent can read to get up to speed on the whole app-state **migration** effort in `calimero-network/core` and pick up the next piece. Keep this updated as things land. Local/untracked working doc (like the other `docs/superpowers/**` plans).
>
> **Last updated:** 2026-06-04
> **Companion docs:** `2026-06-01-migration-execution-plan.md` (the 8-PR plan, file:line anchors), `../specs/2026-06-03-embed-abi-l1-downgrade-gate-design.md` (#2587 design), `2026-06-03-embed-abi-l1-downgrade-gate.md` (#2587 task plan).

---

## 0. TL;DR

Everything through **"migrate your data safely — across a whole group tree, converging byte-identically, with reads staying online, guarded against silently destroying ownership info"** is **done or in review.**

**NEW 2026-06-04 — PR #2674 (migration ergonomics, OPEN):** the dev guide (#2536) AND a standalone `#[derive(Migrate)]` (#2550 add/remove/rename + `with`/`emit`) are now DONE, plus merge-mode guards and an in-process migration test harness (`assert_migrate_converges`). See §2.5.

What's **left** = (a) turn the L1 guard on for all apps (embed-rollout), and (b) the big arc: build the **zero-downtime expand-contract framework (PR-6)**, then **owner-driven rewrite for Authored/Shared data (PR-7 / #2534)** and the **dual-read `#[derive(Migrate)]` + L3** (the rest of PR-8) on top of it.

---

## 1. The 8-PR roadmap — status board

| PR | Issue | What | Status |
|----|-------|------|--------|
| PR-1 | #2537 | UpgradePolicy guard + drop `Coordinated` | ✅ MERGED **#2585** |
| PR-2 | #2534/#2553 prereq | `collection_category` classifier + `SharedStorage` `CrdtCollectionType` variant | ✅ MERGED **#2582** |
| PR-4 | #2553 | L2 CI lint: `calimero-abi diff` + `UNSAFE_IDENTITY_DOWNGRADE` | ✅ MERGED **#2586** |
| **PR-3** | #2587 | **L1 node gate + embed ABI in wasm** | ✅ MERGED **#2645** (`f25cab38`, 2026-06-03). ⚠️ verify `22-scenario-identity-downgrade` merobox passed on master (didn't run pre-merge) |
| PR-5 | #2539 step 1 | reads-available during upgrade (write-only freeze) | ✅ DONE (merged; workflow `21-reads-available-during-upgrade`) |
| L3 | #2550 | derive-macro compile-time downgrade check | ⏳ folds into PR-8 |
| **PR-6** | #2539 | **expand-contract framework** (per-entity lazy migrate, absorb stragglers, vote-free completion, canary soak) | ⏳ **NOT STARTED** — XL, sub-train 6a–6d |
| **PR-7** | #2534 | **owner-driven rewrite** for AuthoredMap/AuthoredVector/SharedStorage | ⏳ NOT STARTED — depends on PR-6 |
| **PR-8** | #2550 | `#[derive(Migrate)]` ergonomics + L3 | 🔵 **SUBSET OPEN — PR #2674** (additive/remove/rename + `with`/`emit`, standalone — NOT PR-6-gated). L3 compile-lint still pending and NOT feasible in the derive (it only sees v2 types, can't diff vs v1) → the L2 `calimero-abi diff` (#2586) is the real gate; the dual-read variant stays PR-6-gated |
| docs | #2536 | developer guide for `#[app::migrate]` | ✅ **DONE — PR #2674** (`docs/migrations.md`, cross-linked from AGENTS.md + README) |
| **ergo** | #2551/#2550 + new | **migration ergonomics**: merge-mode guards (Counter/RGA/`set`) + TestHost migrate ext (`assert_migrate_converges`) + `#[derive(Migrate)]`(+`with`/`emit`) + dev guide + 9 scenario conversions | 🔵 **OPEN — PR #2674** (off master `abdb33b0`, 2026-06-04); built+tested+self-reviewed, awaiting CI/merobox |

Also merged earlier in the train (foundation): #2477 (repair migrate write path), #2493 (cascade engine), #2507 (4 determinism fixes), #2524 (atomic CascadeUpgrade + HLC fence + get_cascade_status), #2530 (status e2e), #2533 (CRDT migrate-coverage audit + AuthoredMap deterministic-id fix).

---

## 2. PR #2645 (the L1 gate) — current state

- **Branch:** `feat/2587-embed-abi-l1-gate`. **Mergeable: MERGEABLE**, `BLOCKED` only on maintainer review (`needs-team-review`/`external` — it adds a wasm artifact-format section + a new upgrade-rejection path; PR body asks for format sign-off).
- **Whole-PR review:** Ready to merge ("no way for a real downgrade to slip past the gate"). All inline bot threads resolved across 4 review cycles (the recurring "use wasmparser not the manual writer" + "serde round-trip" are answered-and-redundant; resolve any repeats with the standing rationale, don't churn).
- **What it does:** (1) each app embeds its `state-schema.json` into its wasm as a `calimero_abi_v1` custom section at build time (`mero-abi embed`, run AFTER `wasm-opt`); the section is covered by `blob_id` → tamper-evident. (2) At upgrade emit, core reads old+new embedded schema from blob bytes and **refuses** a migration that strips identity from a top-level state field. **Emitter-only, fail-open** (legacy apps with no section → warn + allow), **migration-only**.
- **Key files:** `crates/wasm-abi/src/{downgrade.rs,embed.rs}`, `tools/calimero-abi/src/embed.rs`, `crates/context/src/handlers/upgrade_group.rs` (gate at all 3 emit sites: single-group LazyOnAccess block, canary block, `dispatch_cascade` publish task), `crates/wasm-abi/tests/identity_downgrade_real_scenarios.rs`, `workflows/app-migration/22-scenario-identity-downgrade.yml`.

**Watch on merge:** the `22-scenario-identity-downgrade` merobox job — it was authored by structure-matching (merobox not runnable locally), so it may need a first-run tweak.

---

## 2.5 PR #2674 — migration ergonomics (OPEN, 2026-06-04)

Branch `feat/migration-ergonomics` off master `abdb33b0`. One PR bundling the SDK-UX/ergonomics strand (separate from the safety-rail / zero-downtime arc). Built + tested + self-reviewed; awaiting CI/merobox.

- **Merge-mode guards** (`crates/storage`): `Counter::increment`/`decrement`, `RGA::insert`/`insert_str` panic inside a migrate (merge mode) instead of silently forking; `LwwRegister::set` honors merge mode (zeroed stamp, matching `::new()`). Closes the silent-divergence footgun for the *replayable* category. (Authored/Shared owner-stamps are `#[borsh(skip)]` → don't diverge the merkle root, so intentionally not guarded; only `UserStorage::insert` is a narrow unguarded gap, nonsensical in a migrate anyway.)
- **TestHost migrate extension** (`crates/sdk`, extends #2551/#2568): bridges `read_raw` ↔ the committed mock root; adds `TestHost::migrate` + `assert_migrate_converges` (compares the **merkle root_hash**, folding in child collections). In-process can't catch *iteration-order* divergence (mock sorts children by id) → still needs merobox. Also fixes a latent `HARNESS_LIVE`-stuck-on-panic bug in the shipped `TestHost::new`.
- **`#[derive(Migrate)]`** (#2550 subset, standalone — NOT PR-6-gated): carry / `new` / `from` / drop + `with` (type-change, struct→enum) + struct-level `emit` (event). Generates a real `#[app::migrate]` (inherits merge-mode + deterministic-ids). **L3 compile-lint NOT included** — not feasible (the derive only sees v2 types, can't diff vs v1); the L2 abi-diff (#2586) is the real downgrade gate.
- **#2536 dev guide** = the user guide `docs/migrations.md` (in the PR; cross-linked from AGENTS.md + README).
- **9 scenario conversions**: the add/remove/rename/type-change/carry scenarios in `apps/migrations` now use the derive (exercises `with`/`emit` in the real e2e); builds host + wasm32, `state-schema.json` unchanged. `field-split` / `invariant-reshuffle` / `crdt-native` / `identity-downgrade` stay hand-written (cross-field or intentional-unsafe).

Verified: storage 569, sdk-macros 65, harness 9; adversarially code-reviewed (the root-hash convergence compare + the `decrement` guard were review fixes). **Watch on merge:** the `workflows/app-migration` e2e validates the 9 conversions (merobox not runnable locally). Memory: `project_migration_sdk_ergonomics`, `project_testhost_migrate_extension`, `project_2536_migration_dev_guide`.

---

## 3. What's LEFT — pick-up list

### 3A. Small / independent / ready NOW
- ✅ **#2536 — migration developer guide. DONE in PR #2674** (`docs/migrations.md`). Original brief retained below for reference: cover (1) when a migration is needed (additive=no, type/remove/rename=yes; `calimero-abi diff` tells you), (2) mechanics (`#[app::migrate]`, `read_raw()`, BorshDeserialize old struct; runs only under `LazyOnAccess`), (3) **the convergence rule** (migrate runs per-node, emits no causal delta → output MUST be a deterministic pure fn of old state; no `Id::random()`/timestamps/executor-ordering), (4) the **3 categories** (Convergent / Replayable Counter+RGA / Identity-gated Authored*+Shared), (5) the no-silent-downgrade rail (#2586 CI + #2645 node), (6) testing via `apps/migrations/scenario-*` + merobox. Source PRs: #2477/#2507/#2533/#2585/#2586/#2645.

### 3B. NOW READY — #2645 is MERGED (the `embed` subcommand + gate are in master)
> **Immediate verify:** confirm the `22-scenario-identity-downgrade` merobox job passes on master (post-merge run `f25cab38`) — it didn't run before merge. If it fails, the live gate wiring needs a hotfix.

- **Embed-rollout:** add `cargo run -q -p mero-abi -- embed ./res/<app>.wasm ./res/state-schema.json` (AFTER `wasm-opt`) to **every real app's `build.sh`** so the L1 gate covers the whole fleet (fail-open until an app carries the section). Mechanical; touch every `apps/**/build.sh` that builds an `#[app::state]` app. Consider folding into the shared build helper (#2547) instead of per-app duplication.
- **Fail-open metric:** add a counter when `verify_no_identity_downgrade` skips on a missing/unreadable schema (operators see "gate skipped (legacy app)").
- **`unsafe_strip_identity` override:** the governed escape hatch — macro attr `#[migrate(unsafe_strip_identity="reason")]` → field on `MigrationParams` (`crates/context/primitives/src/messages.rs`) → plumbed through the upgrade op → read by the gate → governance allowance check. The gate is hard-refuse until this exists.

### 3C. The BIG remaining arc (sequential; PR-6 → PR-7 → PR-8)
- **PR-6 / #2539 — expand-contract framework (XL, not started, research-gated).** Replace the single-root stop-the-world rewrite with **per-entity lazy migrate-on-access**: v2 dual-reads old+new and writes new; each entity migrates on first touch; a later v3 drops the old read path. Sub-train: **6a** per-entity version tags + lazy migrate (replaces the single-root rewrite at `update_application/mod.rs`); **6b** absorb-don't-drop — replace the HLC fence's silent drop (`crates/context/src/hlc_fence.rs`) with buffer-then-migrate; **6c** reversible reclamation via a shrink-only residue-CRDT, vote-free completion bounded to current membership; **6d** canary soak + app-exported `migration_check(old,new)->bool` + snapshot rollback. Answer #2539 Q1–Q7 first.
- **PR-7 / #2534 — owner-driven rewrite for identity-gated CRDTs.** The "migrate Authored/Shared *content*" capability. Why it's hard: ownership = running node's `executor_id` + entries are ed25519-signed over (data+nonce), so no node can rewrite another's entry (cryptographic block, verified). The model: **each owner re-migrates their OWN entries via normal signed writes** on next access → signature valid, gate satisfied (not bypassed). The identity-gated types are the "hard tenant" INSIDE PR-6's expand-contract. Plus C3: Counter/RGA/NestedMap replay-in-body determinism.
- **PR-8 / #2550 — `#[derive(Migrate)]`** for additive/remove/rename + the **L3** compile-time `is_identity_gated()` warning. Under SDK-UX epic #2561. (The "#9" in old issue comments = analysis-list item that became #2550, NOT GitHub issue 9.)

### 3D. Deferred edge cases / concerns
- **Departed-owner** identity-gated data — owner gone, can't re-sign → needs a "hard reset"; explicitly parked (the #2534 issue says don't block on it).
- **Nested** identity-gated CRDTs inside Record/Variant — #2645 is top-level scope only.
- **Code-only (migration-less) upgrade** that changes the app — could it drop ACL enforcement on future writes? Needs storage-layer verification (raised by bot on #2645; documented as migration-only there).
- **#2060** — signed bundle version upgrades without migration silently ignored (related, separate bug).

---

## 4. Locked design decisions (don't re-litigate)
- **No-silent-downgrade rail = 3 layers:** L1 core gate (authoritative, #2645) + L2 CI lint (#2586) + L3 derive (PR-8). L1 is the guarantee; L2 is fast feedback; L3 advisory.
- **L1 gate:** fail-OPEN on missing/unreadable schema; HARD-REFUSE downgrades (override deferred); EMITTER-ONLY (no receiver re-validation → no fork risk).
- **Embed mechanism:** `mero-abi embed` in each app's `build.sh` AFTER `wasm-opt` (Option A) — NOT `build-wasms.sh` (only builds test fixtures), NOT macro auto-embed (fights wasm-opt strip). Tamper-evidence requires the section be IN the wasm (covered by `blob_id`), not a spoofable side DB.
- **Identity-gated rewrite = owner-driven** (each owner self-migrates own signed entries) — NOT gate-relaxation, NOT forging signatures.
- **Convergence:** migrate output must be a deterministic pure fn of old state (no causal delta emitted).
- **#2539 safety = local + deterministic, NO quorum/voting** (explicit non-goal).
- Migrations only run under `UpgradePolicy::LazyOnAccess`.

## 5. Verified facts / anchors (for whoever picks up gate/embed work)
- Blob bytes: `node_client.get_application_bytes(&application_id, None).await` → `Option<Arc<[u8]>>` (async; returns unpacked `.wasm` with custom section intact). `NodeClient` on `ContextManager`.
- Old app id at upgrade = `meta.target_application_id` (in both `validate_upgrade` and `dispatch_cascade`); new = incoming `target_application_id`.
- `validate_upgrade` is sync + takes `&Store` (no node_client) → single-group gate runs in the async handler block, not inside it.
- Errors: `eyre` (no `UpgradeError` enum). Context crate = `calimero-context`.
- `collection_category` + `CrdtCollectionType` + `CollectionCategory` in `crates/wasm-abi/src/schema.rs`; IdentityGated = AuthoredMap/AuthoredVector/SharedStorage.
- ABI emitter: `calimero_wasm_abi::emitter::emit_manifest(src)` + `Manifest::extract_state_schema()` (what each scenario `build.rs` calls).
- Context integration tests deliberately AVOID the actor layer (store-level only) → actor-handler behaviour is proven by merobox, not Rust integration tests.

## 6. Cross-repo / dependencies
- merobox: `assert_log_present`/`assert_log_absent` (#243) + `expected_failure: true` on `upgrade_group` step already exist (used by `22-scenario-identity-downgrade`). cascade status steps = merobox #272 (0.6.32).
- SDK-UX epic **#2561** owns #2553 (done), #2550 (PR-8), #2547 (scaffolding/shared build.rs — the natural home for embed-rollout), #2548/#2555/#2546/#2549 (other UX issues).

## 7. Memory pointers (persist across sessions)
`project_2587_embed_abi_l1_gate`, `project_2553_abi_diff_downgrade_lint`, `project_2534_content_rewrite_migrations`, `project_2539_zero_downtime_migration`, `project_2536_migration_dev_guide`, `project_2537_upgrade_policy`, `project_2582_safety_rail_foundation`, `project_c4_migrate_crdt_coverage`, `project_cascade_migration_train`.
