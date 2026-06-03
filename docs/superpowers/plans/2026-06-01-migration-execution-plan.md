# App-Migration Execution Plan — PR Roadmap

- **Date:** 2026-06-01
- **Author:** rtb-12
- **Repo state when written:** `calimero-network/core` master @ `fb30cc4c`
- **Companion to:** `docs/superpowers/plans/2026-05-28-migration-remaining-work.md` (the workstream roadmap). This doc does **not** replace it — the roadmap says *what work remains*; this doc says *how it ships as PRs*: scope, which phases combine, the issue (or part) each solves, the solution in plain language, and the implementation plan down to file:line.
- **Status of this doc:** working/local (kept out of feature PRs per standing practice, like the other `docs/superpowers/`** plans).

> All file:line references were verified against master `fb30cc4c` on 2026-06-01. Where a referenced symbol is part of code that shipped this week, the verifying commit is noted.

---

## Part 1 — Context: what's already done (the last week in core)

GitHub login `rtb-12`. The week of **2026-05-25 → 2026-06-01** landed the bulk of the **namespace cascade-migration train** end-to-end, plus the CRDT migrate-coverage audit. Six migration PRs merged; zero left open.

### 1.1 Merged PRs


| #        | Title                                                                                    | Merged | SHA        | What it solved                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                            |
| -------- | ---------------------------------------------------------------------------------------- | ------ | ---------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| **2452** | feat(governance): namespace-level cascade for application upgrades                       | 05-22  | `c490a943` | **PR-0 (precursor).** Adds the wire-format `GroupOp` variants `CascadeTargetApplicationSet` / `CascadeGroupMigrationSet` (schema v6). Foundation only; no engine yet.                                                                                                                                                                                                                                                                                                                                                                                                                                                     |
| **2477** | fix(context): repair per-context migration write path broken by #2433                    | 05-26  | `32ce0176` | **PR-1.** Migrations had been silently dead since #2433 (`MergeFailure(NoMergeFunctionRegistered)` because root-class writes routed through the host merge registry #2465 left empty for app types). Switches the migrate tail to `Interface::write_pre_merged_root_state` (skips merge dispatch, still updates the Merkle index) bookended by a pre-flight LWW check + post-write metadata verify (closes a TOCTOU). Adds e2e workflow `00-baseline` as the regression guard.                                                                                                                                            |
| **2493** | feat(context): cascade engine for namespace-level application migration                  | 05-26  | `5b9d368c` | **PR-2.** Turns one signed cascade op into a fan-out across every matching descendant subgroup/context in a single sync round (replacing N `upgrade_group` RPCs), via `cascade::walk_for_predicate` (cycle-safe). Apply handlers (settings-only), per-descendant propagator spawn on the emitter, fail-fast capability pre-scan, and `from_app_key` as optimistic-concurrency control. Heterogeneous app versions skipped safely.                                                                                                                                                                                         |
| **2507** | fix(context): app_key derivation + migration storage commit, with 14-scenario e2e matrix | 05-30  | `b31e56ac` | Fixes **four** latent multi-node defects a new all-2-node matrix exposed: (1) `app_key` random/zero → cascade predicate skipped every descendant (now derived from bytecode `blob_id`, inherited on apply/invite/join); (2) `ContextStorage` Temporal buffer never committed post-WASM → migrate-created CRDT entries dropped; (3) cross-node migration never reached receivers (stale actor `application_id` cache + lazy-only marker + sync gate); (4) non-deterministic migrate output (merge-mode + Vector `Id::random()` re-keying + the `**with_merge_mode` re-entrancy fix** = scenario-13 divergence root cause). |
| **2524** | feat: atomic CascadeUpgrade op + cascade_hlc HLC fence + get_cascade_status RPC          | 05-30  | `9b431e5f` | **PR-3.** Replaces the ordered two-op cascade pair with a single atomic `GroupOp::CascadeUpgrade` (schema **v6→v7**), fixing a receiver apply-order bug (target-set-before-migration-set silently dropped the migration). Adds sticky initiator-stamped `cascade_hlc` + an HLC fence (`producing_app_key` mismatch && `delta_hlc > cascade_hlc`) that drops stale-schema deltas in **both** receive paths, a generalized `InProgress` write-gate, and the `get_cascade_status` RPC end-to-end (actor → ContextMessage → client → admin HTTP). Legacy v6 variants kept one release for wire-compat.                        |
| **2530** | test(app-migration): e2e for get_cascade_status + assert_cascade_complete                | 05-30  | `994ea682` | First e2e consumer of `get_cascade_status` (workflow `14-cascade-status-rpc`), now that merobox 0.6.32 shipped the step types. Polls `assert_cascade_complete` on each node, asserts the roll-up (`pending==0`, `completed==total==2`) on emitter + receiver. Documents why `06-fence-rejects-straggler` can't be driven yet (no merobox partition-with-RPC-alive primitive).                                                                                                                                                                                                                                             |
| **2533** | test(app-migration): CRDT migrate-coverage audit + AuthoredMap deterministic-id fix      | 05-31  | `67d187d8` | Classifies every `collections` type by whether a migrate-rebuild converges cross-node; adds scenarios **17** (FrozenStorage real re-freeze), **19** (AuthoredVector carry-through), **20** (UnorderedSet rebuild) + carry-through for AuthoredMap/UserStorage/SharedStorage. Fixes a latent `#[app::state]` macro bug: `AuthoredMap` was excluded from `is_collection_type`, so its wrapper id stayed `Id::random()` and diverged across nodes. Counter/RGA/NestedMap deferred → #2534.                                                                                                                                   |


### 1.2 Issues filed this week

- **#2494** — follow-up to #2493 (re-introduce cascade e2e once merobox steps land; largely satisfied by #2530).
- **#2534** — content-rewrite migrations for identity-stamped CRDTs (AuthoredMap/AuthoredVector/SharedStorage).
- **#2536** — developer guide for writing `#[app::migrate]`.
- **#2537** — UpgradePolicy: migrate only works under LazyOnAccess; guard Automatic/Coordinated + deprecate Coordinated.

(Plus **#2553**, the `calimero abi diff` SDK-UX issue, which the safety-rail work plugs into.)

### 1.3 The week's arc, in one paragraph

Un-break the foundation (**#2477** repairs the dead migration write path) → build the cascade engine (**#2493** fans one op out to all descendants) → reality-check it on real multi-node shapes and fix the four defects that surfaced (**#2507**, incl. the determinism fixes that make migrate output byte-identical across nodes) → harden the wire protocol (**#2524**: atomic op + HLC fence + status RPC) → prove the status surface e2e (**#2530**) → close the CRDT-coverage loop + a macro determinism fix (**#2533**). By week's end the cascade pipeline is functionally complete and regression-guarded; the deliberate spin-outs are #2534 (identity-gated rewrite), #2536 (dev guide), #2537 (upgrade policy), and the safety-rail idea that connects #2534/#2553/#2539.

### 1.4 Roadmap workstream status (from the 05-28 roadmap, current)


| WS     | Title                                                                    | Status                                                                                                                                                                                         |
| ------ | ------------------------------------------------------------------------ | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| **A**  | PR-3: cascade HLC fence + `get_cascade_status` + sticky `cascade_hlc`    | **DONE** — shipped as #2524 + #2530                                                                                                                                                            |
| **B**  | PR-4: `force_complete_cascade --evict-peer` + multi-version coexistence  | Deferred (no operator need yet)                                                                                                                                                                |
| **C1** | Upgrade-policy fail-fast guard (Automatic/Coordinated + migrate)         | **Ready — small** → this doc's **PR-1** (= #2537)                                                                                                                                              |
| **C2** | Reconcile sync-gate ↔ HLC fence                                          | **DONE** — settled in #2524: gate covers the InProgress window, sticky `cascade_hlc` + post-Completed fence cover the long-tail; division documented at `node/src/sync/manager/mod.rs:696-698` |
| **C3** | Populated-during-migrate RGA/Counter determinism                         | **Design decided** (replay-in-body contract = category 2); only *scenario* work remains → **PR-7** / #2534                                                                                     |
| **C4** | Migrate coverage for AuthoredMap/UserStorage/FrozenStorage/SharedStorage | **DONE** — shipped as #2533                                                                                                                                                                    |
| **C5** | Concurrent-migration e2e                                                 | Pending — design-first; needs merobox primitive (see Loose ends)                                                                                                                               |
| **C6** | Failed-migration recovery e2e                                            | Pending — design-first; needs merobox primitive (see Loose ends)                                                                                                                               |
| **D**  | merobox#255 cascade step types                                           | **DONE** — merobox 0.6.32 (#272), consumed by #2530. *Stretch `set_node_clock_offset` + the fence-rejection e2e remain (Loose ends).*                                                          |
| **E**  | Reconcile/close #2494 vs #2507                                           | **Pending — ready triage** (#2494 still OPEN; see Loose ends)                                                                                                                                  |


---

## Part 2 — The mental model (read before the PR plan)

Everything below rests on four facts. They're the reason the PRs are shaped the way they are.

### 2.1 The convergence rule

`#[app::migrate]` runs **independently on every node**, under storage merge mode, and emits **no causal delta** (the migrate apply path sets `context.dag_heads = vec![*context.root_hash.as_bytes()]` — `update_application/mod.rs:453`, with the comment "Migration does not create a causal delta"). Therefore **migrate output must be a pure deterministic function of old state.** A migrate that builds identically on every node converges; one that builds a per-node-different result diverges.

### 2.2 Three identity categories (the single most useful classification)


| Category                            | Types                                                                    | Migrate behaviour                                                                                        |
| ----------------------------------- | ------------------------------------------------------------------------ | -------------------------------------------------------------------------------------------------------- |
| **1. Convergent**                   | `UnorderedMap`, `Vector`, `UnorderedSet`, `UserStorage`, `FrozenStorage` | key/index/content-addressed → rebuild freely; auto-converges                                             |
| **2. Deterministically replayable** | `Counter`/`GCounter`/`PNCounter`, `RGA`                                  | per-executor/per-position; converges *iff the body replays* (`increment_for`, `insert_str_at_timestamp`) |
| **3. Identity-gated**               | `AuthoredMap`, `AuthoredVector`, `SharedStorage`                         | ownership = running node's `env::executor_id()` → naive rebuild diverges → **carry-through only** today  |


### 2.3 Authored entries are cryptographically signed (verified)

This is the load-bearing finding for the identity-gated case. A local User-entry write **unconditionally stamps** `signature_data: Some(SignatureData { signature, nonce, signer })` (`interface.rs:2491-2502`); the signed payload commits to **(data + nonce)** ("the signed payload commits to data + nonce, both of which just changed", `interface.rs:2486-2487`). The real ed25519 signature is computed post-execution by `sign_authorized_actions` (`execute/mod.rs:1750`, over `Action::payload_for_signing` = id+data+nonce+access-control) and written by `persist_signed_signatures` (`execute/mod.rs:1886`); receivers `ed25519_verify` against the owner key. `signature_data: None` is **rejected as `InvalidSignature`** (`interface.rs:291-294`; `SignatureData` struct at `entities.rs:405` = `{ signature:[u8;64], nonce:u64, signer:Option<PublicKey> }`).

**Consequence:** rewriting an authored entry's *content* needs a fresh signature only the owner's key can produce. → carry-through needs no re-sign (bytes+sig untouched); content-rewrite by anyone-but-the-owner is a **cryptographic** block, not a policy choice.

### 2.4 Stable-fleet implication

Most "leftover old data" risk is an *offline-node* problem. On an always-online TEE/cloud fleet where **the nodes are the membership**, convergent + replayable migrations complete in one execution round (≈ zero residue), and even identity-gated owner-driven sweeps complete promptly. The only residual that fleet stability does **not** fix is **owner = an external end-user identity** who never returns — that's bounded by an explicit re-sign authority, not by node uptime (see Parked items).

---

## Part 3 — Open issues map


| Issue     | One-line                                     | Chefsale review direction (2026-05-31)                                                                                                                                                               |
| --------- | -------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| **#2534** | content-rewrite for identity-gated CRDTs     | **Drop the gate-relaxation**; do **owner-driven expand-contract** (owner self-migrates own entries via normal signed writes — gate satisfied, not bypassed) + a **no-silent-downgrade safety rail**. |
| **#2537** | migrate only works under LazyOnAccess        | **Collapse to two policies**: `LazyOnAccess` (migrate) + `Automatic` (code-only), behind a fail-fast guard; **deprecate `Coordinated`**.                                                             |
| **#2539** | zero-downtime upgrades (research)            | **Expand-contract** umbrella: per-entity dual-read, lazy migrate-on-access, no quorum, canary soak + snapshot rollback. Identity-gated types are a *tenant* of this model, not a separate scheme.    |
| **#2553** | `calimero abi diff` breaking-change detector | Add an `**UNSAFE_IDENTITY_DOWNGRADE`** finding class = the **CI layer** of #2534's safety rail.                                                                                                      |


> **Cross-reference note:** #2534/#2539/#2553 comments cite "**#9**" as the `#[derive(Migrate)]` item. That "#9" is an **item number from the original SDK-UX analysis list**, *not* GitHub issue #9 (which is an unrelated merged DID/DHT PR). The work is really tracked as **#2550** — *"[SDK UX] `#[derive(Migrate)]` for additive/remove/rename migrations"* — under SDK-UX epic **#2561** (which also owns **#2553** abi diff, ranked there as priority #2, *"makes migrations safe"*). So PR-8 maps to **#2550**; no fresh issue needed.

The four issues form one tree:

```
#2539  expand-contract framework (general zero-downtime model)
  ├─ #2534  identity-gated collections = the hard tenant inside it
  │     ├─ owner-driven rewrite (replaces gate-relaxation)
  │     └─ no-silent-downgrade rail (3 layers):
  │           ├─ L1 core upgrade gate (authoritative)  → #2534
  │           ├─ L2 calimero abi diff CI lint          → #2553
  │           └─ L3 derive-macro compile-time check    → with PR-8
  └─ #2537  interim guard (fail-fast) until #2539's eager path exists
```

One shared prerequisite threads through #2534 + #2553: `**SharedStorage` is not a `CrdtCollectionType` variant** (`schema.rs` has 8 variants, none is SharedStorage — `grep` returns 0; it's flattened to its inner `T` at `normalize.rs:427`), so the schema can't *see* a `SharedStorage → UnorderedMap` downgrade until the variant is added. Additive (`#[non_exhaustive]` at `schema.rs:180`).

---

## Part 4 — The PR plan

Eight PRs across the phases from the 05-31 analysis. Phase mapping and **what combines**:


| PR       | Phase(s)            | Issue / part                            | Depends on    | Size          |
| -------- | ------------------- | --------------------------------------- | ------------- | ------------- |
| **PR-1** | 1 (policy)          | #2537 whole                             | —             | S             |
| **PR-2** | 0 (foundation)      | #2534 + #2553 prereq                    | —             | S–M           |
| **PR-3** | 1 (rail L1)         | #2534 safety rail (core gate)           | PR-2          | M             |
| **PR-4** | 1 (rail L2)         | #2553 whole                             | PR-2          | M             |
| **PR-5** | 3.1 (relief)        | #2539 step 1                            | —             | S–M           |
| **PR-6** | 3.2–3.5 (framework) | #2539 core                              | PR-5          | XL (multi-PR) |
| **PR-7** | 2 (capability)      | #2534 owner-driven + C3 replay          | PR-6 scaffold | L             |
| **PR-8** | 4 (ergonomics)      | #2550 `#[derive(Migrate)]` (epic #2561) | PR-6/7        | L             |


**Recommended combinations**

- **Ship-now bundle:** PR-1 (independent) + PR-2 → PR-3 + PR-4. PR-3 and PR-4 can be **one PR** ("migration safety rail") since they share the `collection_category` classifier and the downgrade-finding logic; keep PR-2 separate because it changes `state-schema.json` shape and regenerates golden files.
- **Relief:** PR-5 ships any time, independent of the rail.
- **Framework:** PR-6 is itself a sub-train (6a→6d); PR-7 and PR-8 land on top.

---

### PR-1 — UpgradePolicy guard + deprecate Coordinated

- **Phases combined:** Phase 1 (policy guardrail). Roadmap **C1**.
- **Issue / part:** **#2537** in full.
- **Solution (plain):** Today, migrating under `Automatic` or `Coordinated` silently corrupts: the receiver swaps its app pointer to v2 but never runs the migrate (the only receiver-side trigger, `maybe_lazy_upgrade`, early-returns for any non-`LazyOnAccess` policy), so v2 wasm reads v1 bytes → a silent borsh "Not all bytes read" panic. Make that combination **fail loudly** at emit time with a clear error. And **deprecate `Coordinated`** — it does nothing `Automatic` doesn't, and its only distinguishing field (`deadline`) is inert (read by the meroctl CLI, never enforced in core).
- **Verified anchors:**
  - `maybe_lazy_upgrade` at `execute/mod.rs:2136`; the exclusive gate `if !matches!(meta.upgrade_policy, UpgradePolicy::LazyOnAccess) { return None }` at `**execute/mod.rs:2168`**.
  - `UpgradePolicy` enum `crates/primitives/src/context.rs:198` (`#[default]` `LazyOnAccess` at :202, `Coordinated { deadline: Option<Duration> }` at :205, manual borsh tags 0/1/2 at :216-241).
  - CLI: `UpgradePolicyArg` `crates/meroctl/src/cli/upgrade_policy.rs:7`, `to_upgrade_policy` :13; `deadline_secs` read only in `cli/group/update.rs` + `cli/namespace/create.rs`; no core enforcement (only a borsh round-trip test in `store/src/key/group/mod.rs`).
- **Impl plan:**
  1. **Guard at op emission.** In the `upgrade_group` / `update_application` emission path, before emitting the group op, where both `migration: Option<...>` and `policy` are known:
    ```rust
     // crates/context/src/handlers/upgrade_group.rs (or update_application/mod.rs emit path)
     if migration.is_some()
         && !matches!(policy, UpgradePolicy::LazyOnAccess)
     {
         return Err(UpgradeError::UnsupportedUpgradePolicyForMigration { policy });
     }
    ```
     Add the structured variant to the upgrade error enum. (Code-only upgrades — `migration.is_none()` — stay allowed under `Automatic`.)
  2. **Deprecate `Coordinated`.** `#[deprecated(note = "does nothing Automatic doesn't; deadline is inert. Use LazyOnAccess for migrate, Automatic for code-only.")]` on the variant at `context.rs:205`. **Keep borsh deserialize** (tag 2 stays, for stored/in-flight ops). Reject it on *new* upgrades (either in `to_upgrade_policy` or alongside the guard). Drop/hide it from the meroctl `UpgradePolicyArg` surface (`upgrade_policy.rs:7`) — `#[clap(hide = true)]` or remove the arm.
  3. **Tests:** (a) migrate-carrying upgrade under `Automatic` → `UnsupportedUpgradePolicyForMigration` (not a downstream borsh panic); (b) code-only upgrade under `Automatic` still works; (c) borsh round-trip of a stored `Coordinated` value still decodes.
- **Combines with:** nothing required; smallest, highest value-to-risk. Good first PR.

---

### PR-2 — Safety-rail foundation: shared classifier + `SharedStorage` schema variant

- **Phases combined:** Phase 0 (foundation). Prerequisite for PR-3 **and** PR-4.
- **Issue / part:** the shared prerequisite called out in **#2534** and **#2553**.
- **Solution (plain):** Two foundations. (a) The logic that answers "is this collection identity-gated?" is copy-pasted in **three** places that already disagree — unify it into one `collection_category()`. (b) The schema can't even *see* `SharedStorage` (it's flattened to its inner type during normalization), so add it as a first-class CRDT collection type. Together these unblock both the authoritative core gate (PR-3) and the CI lint (PR-4).
- **Verified anchors (the scatter):**
  - `CrdtCollectionType` enum `crates/wasm-abi/src/schema.rs:181` (`#[non_exhaustive]` at :180); 8 variants, **no `SharedStorage`**. Per-field `crdt_type: Option<CrdtCollectionType>` at `schema.rs:133`, preserved through normalization.
  - `SharedStorage` flattened (unwrapped to inner `T`, no `Collection`, no `crdt_type`) at `normalize.rs:427`.
  - `is_collection_type` substring matcher at `crates/sdk/macros/src/state.rs:555` (matches UnorderedMap/Vector/UnorderedSet/Counter/ReplicatedGrowableArray/UserStorage/FrozenStorage/SharedStorage — but **not** AuthoredMap/AuthoredVector explicitly; "Vector" substring catches AuthoredVector).
  - `EXCLUDED_ACCESS_CONTROL_TYPES` at `crates/sdk/macros/src/private.rs:462`.
  - CrdtCollectionType-based classification scattered in `normalize.rs` (lines ~234, 315, 324, 354, 365, 375, 386).
- **Impl plan:**
  1. **One classifier.** Introduce
    ```rust
     pub enum CollectionCategory { Convergent, Replayable, IdentityGated }
     pub fn collection_category(crdt: &CrdtCollectionType) -> CollectionCategory { /* ... */ }
     // plus a name-based shim for the macro/string sites:
     pub fn collection_category_by_name(type_name: &str) -> Option<CollectionCategory>;
    ```
     Place in a low-dep crate both `wasm-abi` and `sdk/macros` can use (candidate: `wasm-abi` for the enum-keyed fn; a tiny shared const table for the name-keyed shim). Re-point `private.rs:462`, the relevant `state.rs:555` consumers, and the `normalize.rs` classification at this one source.
  2. `**SharedStorage` as a CRDT collection type.** Add the variant to `CrdtCollectionType` (`schema.rs`, additive). Change the `normalize.rs:427` `"SharedStorage" =>` arm to emit a `Collection` carrying `crdt_type: Some(CrdtCollectionType::SharedStorage)` + inner type (mirroring the `UserStorage`/`FrozenStorage` arms at :414-422) **instead of** unwrapping to inner `T`.
  3. **Golden regen.** This changes `state-schema.json` shape for any app with a `SharedStorage` field → regenerate `apps/state-schema-conformance/state-schema.expected.json` (+ `apps/abi_conformance`) and note the schema-version bump in the PR. This is *why PR-2 is its own PR* — the diff is mechanical but touches goldens.
  4. **Drift-guard test** (the existing `private.rs` pattern): a test that goes red if category membership drifts between the classifier and the enum.
- **Combines with:** foundation only; merge before PR-3/PR-4.

---



#### PR-4 — L2 `calimero abi`PR-3 + PR-4 — The no-silent-downgrade safety rail (L1 core gate + L2 CI lint)

> Recommended as **one PR** ("migration safety rail") since L1 and L2 share PR-2's classifier and the downgrade-finding logic. Split only if review prefers.

- **Phases combined:** Phase 1 (safety). **#2534** rail (L1) + **#2553** (L2).
- **Depends on:** PR-2.

**The part it solves (plain):** A developer can change `AuthoredMap → UnorderedMap` (or `SharedStorage → UnorderedMap`, or `AuthoredVector → Vector`, or drop the field) in a migration. It compiles, runs, and **silently strips per-entry authorship / the writer ACL across the whole network — no error today.** The rail makes that a loud, explicit failure: caught authoritatively in core before the op is emitted (L1), and caught early in CI for fast feedback (L2).

#### PR-3 — L1 core upgrade gate (authoritative)

- **Verified anchors:** `verify_appkey_continuity` at `update_application/mod.rs:299` (the natural neighbour for a continuity check); `execute_migration` at :502.
- **Impl plan:**
  1. **Where:** add an `verify_no_identity_downgrade(old_schema, new_schema, migration_meta)` check next to `verify_appkey_continuity` (`update_application/mod.rs:299`), run **before** the op is emitted.
  2. **Logic:** diff the **old context's** `state-schema.json` against the **new app's** `state-schema.json`, per top-level field. Using PR-2's `collection_category`, reject when a field whose old type is `IdentityGated` maps to a non-`IdentityGated` type (or disappears) — `Err(UpgradeError::IdentityDowngradeForbidden { field, from, to })` — **unless** the migration carries an explicit `#[migrate(unsafe_strip_identity = "<reason>")]` allowance in its metadata.
  3. **Integration question to resolve in this PR:** *where does the gate read old vs new schema?* `state-schema.json` is emitted into each app's ABI; confirm how an `application_id` resolves to its stored schema (app package/blob metadata) so both sides are available at emit time. This is the one real unknown — scope a short spike at the top of the PR.
  4. **Tests:** `AuthoredMap→UnorderedMap` rejected; carry-through (same type) allowed; cat3→cat3 *value* rewrite allowed; field annotated `unsafe_strip_identity` allowed; `SharedStorage→UnorderedMap` rejected (proves the PR-2 variant made it visible). `diff` + `UNSAFE_IDENTITY_DOWNGRADE`

- **Verified state of the tool:** there is **no** `abi diff` today. ABI tooling is the separate `mero-abi` binary (`tools/calimero-abi`, clap name `calimero-abi`) with only `Extract` / `Types` / `State` subcommands. The conformance harness (`apps/state-schema-conformance/verify-state-schema.sh`) does **byte-level** `diff -u` of golden vs built schema — no semantic diff.
- **Impl plan:**
  1. **New subcommand** `Commands::Diff { old: PathBuf, new: PathBuf }` in `tools/calimero-abi/src/main.rs`. Parse both `state-schema.json`, walk top-level fields.
  2. **Classify** each field change:
    - type unchanged → ignore;
    - new field with default-fillable type → `ADDITIVE`;
    - type change (incl. `crdt_type` change, e.g. `LwwRegister<u64> → LwwRegister<String>`) → `BREAKING — migration required`;
    - `IdentityGated → non-IdentityGated` or removed (via PR-2's `collection_category`, now seeing `SharedStorage`) → `**UNSAFE_IDENTITY_DOWNGRADE`**, override = `#[migrate(unsafe_strip_identity="..."]` + governance allowance.
  3. **Known limitation to document:** field **rename** looks like remove+add → can false-positive a downgrade. Ship with a rename heuristic or an explicit `--rename old=new` map; note it.
  4. **CI wiring:** extend the app-migration CI / `verify-state-schema.sh` to run `calimero-abi diff <prev> <curr>` and fail on `BREAKING`-without-migration or `UNSAFE_IDENTITY_DOWNGRADE`-without-override.
- **L3 (compile-time, advisory):** extend the derive macro's type classifier with `is_identity_gated()` so `#[derive(Migrate)]` warns early. Fold into **PR-8** (the macro), since it needs the macro to exist. Advisory only — hand-written `#[app::migrate]` is arbitrary Rust the macro can't introspect; **L1 is the guarantee.**

---

### PR-5 — Reads-available during upgrade (zero-downtime, step 1)

- **Phases combined:** Phase 3.1 (the quick relief from #2539). Independent.
- **Issue / part:** **#2539** suggested-landing step 1.
- **Solution (plain):** Today an in-progress upgrade freezes the whole app — **both reads and writes** are refused until the cascade completes. Let **reads keep working** (served from the pinned pre-migration root) so only *writes* pause during the `InProgress` window. Big UX win, small change.
- **Verified anchors:** the `InProgress` gate refusing all writes at `execute/mod.rs:129-205` (returns `ExecuteError::UpgradeInProgress` at :192); `upgrade_blocks_write(status)` at `execute/mod.rs:2123` (true only for `GroupUpgradeStatus::InProgress`); `GroupUpgradeStatus` at `store/src/key/group/mod.rs:1457`.
- **Impl plan:**
  1. Thread a read-vs-write **intent** into the gate at `execute/mod.rs:129-205`. The gate already keys off `upgrade_blocks_write`; today the call path "doesn't carry an intent flag" (per the in-code comment at :169) — add one.
  2. For **read/view** calls (`__calimero_*` query/view entrypoints), bypass the write-gate and serve from the currently-committed root (which is the pre-migration state until migrate commits its single-root rewrite at `update_application/mod.rs:443/453`).
  3. Keep **writes** blocked during `InProgress` (unchanged correctness).
  4. **Tests:** during `InProgress`, a view call returns pre-migration state; a mutate call returns `UpgradeInProgress`.
- **C2 note:** this is the right moment to settle roadmap **C2** (does the per-delta HLC fence replace #2507's coarse sync-gate, or do they layer?) since both touch this gate.

---

### PR-6 — Per-entity expand-contract framework (zero-downtime core)

- **Phases combined:** Phase 3.2–3.5. **Research-gated** (answer #2539 Q1–Q7 first). Itself a sub-train.
- **Issue / part:** **#2539** core direction.
- **Solution (plain):** Stop rewriting the entire root in one frozen shot. Instead: v2 **dual-reads** old+new layouts and writes new; each entity migrates itself on first access; a v3 later drops the old read path. The app stays read+write available, and v1/v2 ops stay mergeable so no global freeze is needed. Safety is **local + deterministic, no quorum** (the explicit non-goal).
- **Key reframe (from the 05-31 analysis):** separate three things currently fused into "contract":
  1. *stop writing old layout* — safe, local, immediate;
  2. *keep the migrate function* — a tiny pure fn, near-free to retain;
  3. *delete old bytes* — the only irreversible step.
  You get ~95% of the payoff at step 1 with no coordination; only step 3 needs a "is everyone done?" signal — and that's made safe (not perfect) below.
- **Sub-PRs / impl direction:**
  - **6a — per-entity version tags + lazy migrate-on-access.** Each entity carries a schema version; reading an old-tagged entity runs the migrate fn and writes the new version as a normal convergent delta. Replaces the single-root rewrite at `update_application/mod.rs:443-453`.
  - **6b — absorb-don't-drop.** Replace the HLC fence's **silent drop** of stale-schema deltas with **buffer-then-migrate**. Touch points: `crates/context/src/hlc_fence.rs` (`should_fence:20`, `delta_is_fenced:31`) and the state-delta receive path that currently drops+meters (node `state_delta` handler). A node offline across the whole window is then *absorbed* on reconnect, not lost — directly addressing #2539 Q2/Q3.
  - **6c — reversible reclamation.** A shrink-only **residue CRDT** (count of old-layout entities) any node can observe; reclaim old bytes only when `residue==0` in synced view **+** soak window **+** snapshot taken **+** scoped to **current governance membership**. Wrong guess self-heals (6b) or rolls back (snapshot). This is the **vote-free completion** answer (#2539 Q4): bound "everyone" to current membership, not all history; observe convergent state, don't poll a vote.
  - **6d — canary soak + `migration_check`.** Upgrade a canary subgroup first; run an app-exported `migration_check(old_root,new_root)->bool` during a soak; gate the full cascade on its health (#2539 Q5/Q6). Snapshot gives clean rollback (Q7).
- **Stable-fleet note:** on an all-online TEE fleet where nodes are the membership, 6c's `residue==0` signal is reliable and reclamation becomes routine; 6b stays mandatory anyway (rolling deploys always create brief version skew).

---

### PR-7 — Owner-driven cat-3 rewrite + Counter/RGA/NestedMap replay coverage

- **Phases combined:** Phase 2 (capability) + roadmap **C3**. Depends on PR-6's dual-read scaffolding.
- **Issue / part:** **#2534** owner-driven rewrite (replacing the gate-relaxation) + the deferred replay scenarios.
- **Solution (plain):** For the genuinely-needed case of rewriting identity-gated content: each **owner** rewrites only *their own* entries via a **normal signed write** after the upgrade (the gate is satisfied because the rightful owner does the write and re-signs with their own key — no bypass). `SharedStorage` is easier: identity is collection-level, so any single writer copies the whole blob. Plus: add the missing plain-collection replay scenarios (Counter/RGA/NestedMap) deferred from #2533.
- **Verified anchors:** `authored_map.rs` — `insert` open at :118, `owner_of` at :205, `update`/`remove` owner-gated at :141/:171; `make_owner_stamp` (`authored_common.rs:34`) reads `current_executor()` (= `env::executor_id()`); the re-sign path (`execute/mod.rs:1750`/`:1886`).
- **Impl direction:**
  1. **Expand:** v2 dual-reads old+new (PR-6 scaffold).
  2. **Self-migration method:** a normal method that, for entries where `owner_of(k) == executor_id()`, copies them into the new layout via a regular `insert`/`push` (which re-stamps + re-signs as the owner). For `SharedStorage`, any single writer copies the blob (collection-level identity).
  3. **On-the-fly:** editing an old-layout entry migrates it inline (edits are already owner-gated → correct re-stamp).
  4. **Contract:** drop the old read path in v3 once residue (PR-6c) hits zero.
  5. **Replay scenarios (C3):** Counter via `increment_for(executor_id, …)` (not `increment`), RGA via `insert_str_at_timestamp(…)`, NestedMap inner-collection id-determinism investigation — as `apps/migrations/scenario-*` crate pairs + 2-node workflows, matching the #2533 recipe.
- **Parked sub-case:** rewriting an **absent owner's** authored content — cryptographically hard (§2.3). Deferred; see Parked items.

---

### PR-8 — Versioned-state / `#[derive(Migrate)]` macro (ergonomics)

- **Phases combined:** Phase 4. Depends on PR-6/PR-7.
- **Issue / part:** **#2550** (`#[derive(Migrate)]`, under SDK-UX epic **#2561**) = the concrete form of **#2539** Q8. *(The comments' "#9" is an analysis-list item number that became #2550, not GitHub issue #9.)*
- **Solution (plain):** A `#[derive(Migrate)]` / versioned-state macro that generates the dual-read + lazy-migration hook so app authors write only `migrate(old) -> new` per type. One wrapper; the per-category strategy (carry / replay / owner-driven) plugs in underneath. Makes expand-contract the ergonomic default and is where the **L3** compile-time downgrade check lives.
- **Impl direction:** generate per-field dual-read accessors keyed off PR-6a version tags; dispatch to the category strategy via PR-2's `collection_category`; emit the `is_identity_gated()` compile-time warning (L3). API surface to be designed with the macro's existing `#[app::state]` / `#[app::migrate]` infrastructure (`crates/sdk/macros/src/{state,migration}.rs`).

---

## Part 4b — Loose ends from the 05-28 roadmap (small, not their own PR)

Carried over from `2026-05-28-migration-remaining-work.md` after this week's merges. None is large enough for a dedicated PR, but each is still real:

- **E — close/trim #2494 (ready now, ~triage).** #2494 ("re-introduce cascade e2e workflows + v3 fixture once merobox#255 publishes") is **still OPEN**. merobox steps shipped (#272) and #2530 consumed them, so most of #2494's scope is satisfied. Action: audit #2494 vs what #2507/#2530 shipped, then either **close as subsumed** or trim to the two genuinely-missing cascade workflows (`04-cascade-skip-heterogeneous`, `05-cascade-chain-v1-to-v3`). One comment + close/relabel. *Do this regardless of the 8-PR plan.*
- **Fence-rejection e2e gap (verification debt of A/#2524).** The HLC fence shipped with unit + integration coverage but **no e2e** — `workflows/app-migration/06-fence-rejects-straggler.yml` does not exist, and nothing in `workflows/app-migration/` exercises `UpgradeFenced`. #2530 documented why: no merobox **partition-with-RPC-alive** primitive exists to keep a straggler offline-to-sync but online-to-RPC. Blocked on the **same merobox primitive** as C5/C6 → bundle the primitive request once, then this e2e + concurrent (C5) + failed-recovery (C6) all unblock together. Stretch step `set_node_clock_offset` (roadmap §D) belongs here too.
- **C3 is decided, not open.** The RGA/Counter "populated-during-migrate" determinism question resolved to a **documented replay-in-body contract** (category 2: rebuild via `increment_for` / `insert_str_at_timestamp`), captured in `crates/sdk/AGENTS.md` and the #2533 audit. So C3 needs no design phase — only the **scenario coverage** in **PR-7** and the developer-guide writeup in **#2536**. (Listed here so it isn't mistaken for still-open design work.)
- **#2536 — migration developer guide.** Not in the original roadmap's lettered workstreams but filed this week; it's the natural home for the convergence rules + the category model. Small docs PR; can ride alongside PR-7 or stand alone.

## Part 5 — Parked (off the critical path)

- **Departed-owner authored *content* rewrite** — cryptographically blocked (§2.3: entries signed over data+nonce; only the owner's key re-signs). Direction when picked up: a **distinct verifiable migration authority** that re-signs deterministically while recording the original author separately (never forging "signed by X"). On a TEE fleet, the natural signer is the **attested enclave** under policy, with **shared attested migration key + deterministic Ed25519 + epoch-derived nonce** (or sign-after-converge). Tracked in the #2534 deferral comment. **Do not block** PR-2/3/4/7 on it.
- **True eager `Automatic` migration** — the long-term replacement for PR-1's interim guard. Builds on PR-6 (6b absorb, 6c residue/membership) + the all-online TEE assumption. Needs an eager receiver-side trigger + sync barrier + real deadline/quorum. #2537 longer-term.
- `**force_complete_cascade --evict-peer`** + multi-version-coexistence workflow — roadmap **B**; defer until a real operator need appears.
- **Concurrent-migration (C5) / failed-migration-recovery (C6) e2e** — design-first; blocked on new merobox primitives (overlapping-upgrade driver; failing-migrate injector; partition-with-RPC-alive).

---

## Part 6 — Dependency graph & suggested order

```
PR-1 (#2537 guard) ───────────────── independent ─────────────┐
                                                              │ SHIP-NOW
PR-2 (classifier + SharedStorage variant) ──┬──► PR-3 (L1 gate)┤  (trust)
                                            └──► PR-4 (L2 lint)─┘
PR-5 (#2539 reads-available) ───────────── independent quick win
        │
        ▼
PR-6 (#2539 expand-contract: 6a→6b→6c→6d) ──► PR-7 (#2534 owner-driven + C3 replay)
        │                                              │
        └──────────────────────────────► PR-8 (derive macro, L3)
                                                       │
                              parked: departed-owner TEE-signer, eager Automatic, B/C5/C6
```

**Suggested sequence**

1. **PR-1** — smallest, independent, turns silent corruption into a clear error. *(C2 reconcilation rides along.)*
2. **PR-2 → PR-3+PR-4** — the safety rail; trustworthy migrations before more capability. Highest value-to-risk after PR-1.
3. **PR-5** — biggest visible relief (no more stop-the-world reads) for little code.
4. **PR-6** (sub-train) — the real zero-downtime fix; research-gate the open #2539 questions first.
5. **PR-7**, then **PR-8** — capability + ergonomics on top of the framework.

Parked items stay parked until a concrete need (or, for the departed-owner case, until the attested-signer infra from PR-6 exists).