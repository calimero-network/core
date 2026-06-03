# Embed ABI in wasm + L1 identity-downgrade gate — Design

- **Date:** 2026-06-03
- **Issue:** #2587 (Embed app ABI in the wasm `calimero_abi_v1` + L1 identity-downgrade upgrade gate)
- **Status:** approved design, ready for implementation plan
- **Builds on:** #2582 (PR-2, `collection_category` + `SharedStorage` variant, MERGED), #2586 (PR-4, L2 `calimero-abi diff` lint, MERGED), #2585 (PR-1, emit-time upgrade-policy guard, MERGED)
- **Companion to:** `docs/superpowers/plans/2026-06-01-migration-execution-plan.md` (this is plan PR-3, which absorbed the embed prerequisite the spike uncovered)
- **Standing practice:** this spec stays out of the feature PR (like the other `docs/superpowers/**` plans).

---

## 1. Problem

A developer can change an identity-gated CRDT to a plain one in a migration —
`AuthoredMap → UnorderedMap`, `AuthoredVector → Vector`, `SharedStorage → UnorderedMap`,
or simply drop the field. It compiles, runs, and **silently strips per-entry
authorship / the writer-ACL across the whole network. No error today.**

The L2 CI lint (`calimero-abi diff`, #2586) catches this *in CI* — but CI is
bypassable (a developer can ship without it). The authoritative defense must live
**in core**, refusing the upgrade before it is emitted. That is the **L1 gate**.

**Why it is blocked today (verified):** the node has no access to an app's state
schema at runtime. `state-schema.json` is emitted only into the build tree
(`build.rs` → `res/state-schema.json`); neither `ApplicationMeta`
(`crates/store/src/types/application.rs`) nor the `Application` primitive
(`crates/primitives/src/application.rs`) carries it; the `calimero_abi_v1` wasm
custom section is *referenced* by `tools/calimero-abi/src/inspect.rs` but **never
written** (absent from every built wasm). So the gate has nothing to read. This
spec fixes that prerequisite (embed the schema in the wasm) **and** adds the gate.

## 2. Goals / non-goals

**Goals**
- The app's state schema travels *inside* its wasm, tamper-evident (bound to `blob_id`).
- At upgrade emit time, core reads old + new schema and **refuses an identity downgrade**.
- Back-compat: apps built before embedding (no section) do not break — the gate
  fails *open* with a warning.

**Non-goals (explicit follow-ups)**
- `#[migrate(unsafe_strip_identity = "…")]` override (macro attr + wire field + governance).
- Rolling the embed step out to *every* real app (this PR wires only the downgrade-scenario apps).
- Receiver-side re-validation of the op.
- Macro-based auto-embed (Option B below).
- Nested identity-gated CRDTs inside `Record`/`Variant` fields (top-level scope only, as in #2586).

## 3. Locked decisions

| Decision | Choice | Rationale |
|---|---|---|
| Back-compat when old app has no embedded schema | **Fail-open + warn** (skip gate, `warn!` + metric, allow) | Existing apps keep upgrading; gate protects apps built after embedding; can tighten to fail-closed in a later release once fleets re-deploy. |
| Override / escape hatch | **Hard-refuse; defer `unsafe_strip_identity`** | Smallest correct guarantee; a blocked dev's legitimate path today is owner-driven rewrite (#2534), not stripping. Override is a focused follow-up. |
| Enforcement point | **Emitter-only** (`validate_upgrade` + `dispatch_cascade`) | Catches the real threat (a dev *accidentally* shipping a downgrade). Receiver re-validation would let a refusing receiver fork from peers — a large correctness surface for an out-of-scope (malicious-emitter) threat. |
| Embed mechanism | **Option A** — `mero-abi embed` tool in each app's `build.sh`, **after `wasm-opt`** | Lands the schema in the *deployed* artifact; embedding after `wasm-opt` dodges the custom-section-strip risk; consistent with fail-open (partial coverage grows over time). |

**Why not the alternatives (recorded so they are not re-litigated):**
- *Embed in `build-wasms.sh` (shared wrapper):* that script only builds the
  `apps/migrations/**` test fixtures by calling each one's `build.sh`. It is **not**
  the path a real deployed app is built through, so it would sticker test fixtures
  and ship real apps unstickered — backwards.
- *Macro `#[link_section]` auto-embed (Option B):* the only approach that covers
  every app automatically, but it embeds *before* `wasm-opt` (strip risk) and the
  `#[app::state]` macro would have to reproduce the whole-crate schema emit at
  macro-expansion time. Deferred as a future "make it automatic" hardening step.

## 4. Architecture

### 4.1 Components

1. **`mero-abi embed <wasm> <state-schema.json>`** (`tools/calimero-abi`)
   Reads the schema JSON, appends it to the wasm as a `calimero_abi_v1` **custom
   section**. A wasm custom section is `0x00 <payload-len> <name-len><name><bytes>`
   and is valid to concatenate onto a complete module, so this needs no wasm-rewrite
   dependency. **Idempotent:** if a `calimero_abi_v1` section already exists, replace
   it (parse sections, drop the old one, append the new) so re-runs are safe.

2. **`read_embedded_state_schema(wasm: &[u8]) -> Option<Manifest>`** (`calimero-wasm-abi`, lib)
   Walks the wasm's custom sections with `wasmparser` (the same technique
   `tools/calimero-abi/src/inspect.rs` uses — but implemented in the lib so core can
   call it without depending on the `mero-abi` binary; add `wasmparser` to
   `calimero-wasm-abi` deps if not already present), finds `calimero_abi_v1`, parses the
   JSON into a `Manifest`. Returns `None` if the section is absent or malformed — this
   drives fail-open at the gate.

3. **`identity_downgrades(old: &Manifest, new: &Manifest) -> Vec<IdentityDowngrade>`** (`calimero-wasm-abi`, lib)
   The top-level-field identity-downgrade check from #2586, lifted next to
   `collection_category` so **both** the `mero-abi diff` tool and the core gate call
   one implementation (DRY). For each top-level state field whose **old** type is
   `IdentityGated` and **new** type is not (or is gone), emit
   `IdentityDowngrade { field, from, to }`. `IdentityDowngrade` carries display
   strings for the field name and the from/to type labels.

4. **Node blob→schema resolver** (context handlers helper)
   Given an `application_id`: resolve to its `Application { blob: ApplicationBlob {
   bytecode: BlobId } }`, fetch the wasm bytes from the node's blob store, run
   `read_embedded_state_schema`. The **old** app = the context's current
   `application_id`; the **new** app = the incoming `application_id`. Read-on-demand,
   no caching — the gate runs only at upgrade emit, so it is rare and cheap.

5. **The gate** (`crates/context/src/handlers/upgrade_group.rs`)
   ```
   fn verify_no_identity_downgrade(old: Option<&Manifest>, new: Option<&Manifest>)
       -> Result<(), UpgradeError>
   ```
   - Either schema `None` → `warn!("cannot verify identity downgrade: …(no embedded ABI)")`
     + bump a metric, **return Ok** (fail-open).
   - Else `identity_downgrades(old, new)`; if non-empty → `Err(eyre::eyre!("identity
     downgrade forbidden: field '{field}' {from} → {to} strips authorship/writer-ACL
     network-wide"))` (first finding). The two emit sites already return
     `eyre::Result` and error via `eyre::eyre!` — the gate matches that style; there is
     **no `UpgradeError` enum** in this code (verified).
   Called in **`validate_upgrade`** (single-group, `upgrade_group.rs:481`) and
   **`dispatch_cascade`** (cascade, `:835`), **only when `migration.is_some()`** —
   the same two emit sites where PR-1 (#2585) placed its policy guard.

### 4.2 Data flow

```
BUILD    build.rs → res/state-schema.json
         cargo build → app.wasm
         wasm-opt → app.wasm (optimised)
         mero-abi embed app.wasm res/state-schema.json → app.wasm + [calimero_abi_v1]

DEPLOY   app.wasm installed as a blob; blob_id = hash(bytes) covers the section
         ⇒ schema is tamper-evident, bound to app identity

UPGRADE  emitter, in validate_upgrade / dispatch_cascade, migration.is_some():
           old = read_embedded_state_schema( blob_of(context.application_id) )
           new = read_embedded_state_schema( blob_of(incoming application_id) )
           verify_no_identity_downgrade(old, new):
             either None    → warn! + metric + ALLOW
             downgrade found → Err(IdentityDowngradeForbidden) — op never emitted
             else            → ALLOW
```

### 4.3 Tamper-evidence

The `calimero_abi_v1` section is part of the wasm bytes, so it is covered by the
`blob_id` content hash. You cannot alter the declared schema without changing
`blob_id` → a different application identity. This is why the schema is **embedded**,
not stored in a node-side DB (which would be spoofable: ship benign wasm, register an
honest-looking schema, then swap behaviour).

## 5. File structure

**Create**
- `tools/calimero-abi/src/embed.rs` — the `embed` subcommand (append/replace section).
- `crates/context/tests/identity_downgrade_gate.rs` — core integration test.
- `apps/migrations/scenario-identity-downgrade-v1/` + `-v2/` — **already exist** (from #2586); this PR adds the `mero-abi embed` line to their `build.sh`.
- `workflows/app-migration/21-scenario-identity-downgrade.yml` — merobox negative-path workflow.

**Modify**
- `tools/calimero-abi/src/main.rs` — register the `Embed { wasm, schema }` subcommand.
- `crates/wasm-abi/src/lib.rs` (or a focused module) — `read_embedded_state_schema`, `identity_downgrades`, `IdentityDowngrade`.
- `tools/calimero-abi/src/diff.rs` — delegate its identity-downgrade detection to `identity_downgrades` (DRY; behaviour unchanged, all #2586 tests stay green).
- `crates/context/src/handlers/upgrade_group.rs` — the resolver helper + `verify_no_identity_downgrade`, called in `validate_upgrade` and `dispatch_cascade`.
- (no error-enum change — `validate_upgrade`/`dispatch_cascade` use `eyre::Result`; the gate returns `eyre::eyre!(...)`.)
- `apps/migrations/scenario-identity-downgrade-v1/build.sh` + `-v2/build.sh` — add `mero-abi embed` after `wasm-opt`.
- `.github/workflows/app-migration-e2e.yml` — add `21-scenario-identity-downgrade` to the merobox matrix.

## 6. Testing

| Level | What | Where |
|---|---|---|
| **Unit** | `embed` → `read_embedded_state_schema` round-trip (incl. idempotent re-embed); section survives a representative wasm | `tools/calimero-abi` |
| **Unit** | `identity_downgrades` table: AuthoredMap→UnorderedMap = downgrade; carry-through (same type) = none; SharedStorage dropped = downgrade; AuthoredMap→AuthoredVector (both gated) = none; plain→plain change = none | `crates/wasm-abi` |
| **Unit** | gate: old=gated/new=plain → `IdentityDowngradeForbidden`; either schema `None` → Ok (fail-open); `migration.is_none()` → gate not run | `upgrade_group.rs` tests mod |
| **Core integration** | build + `embed` the real downgrade-scenario v1/v2 wasms, load their bytes as the blob, drive the upgrade handler: v1→v2-with-migration **refused**; same-type carry-through **allowed** | `crates/context/tests/identity_downgrade_gate.rs` |
| **Static (exists)** | `calimero-abi diff` on the scenario schemas | `schema-downgrade-guard` CI job (#2586) |
| **Merobox e2e** | deploy v1, attempt v2-with-migration with `expected_failure: true`; `assert_log_present "IdentityDowngradeForbidden"` + `assert_log_absent "Executing migration"` | `workflows/app-migration/21-scenario-identity-downgrade.yml` |

**Merobox capability confirmed:** `group_upgrade.py` already supports
`expected_failure: true` (`_is_expected_failure()` → on errored RPC reports
"✓ Expected failure" and the step passes; `_report_unexpected_success()` warns if it
unexpectedly succeeds). No merobox change required.

## 7. Open implementation details (low-risk, resolve during build)

- **Blob-bytes fetch API:** confirm the exact call to read a blob's bytes from the
  node's blob store given a `BlobId` (the existing `update_application` path already
  loads the module from blob storage — reuse that access). Small spike at the top of
  the blob-resolver task.
- **Metric name:** pick a counter (e.g. `identity_downgrade_gate_skipped_total`) for
  the fail-open path, consistent with existing metric naming.
- **`UpgradeError` location:** add the variant where the upgrade error enum lives and
  ensure it surfaces as a clear RPC error string (the merobox `assert_log_present`
  matches on it).

## 8. Why team-review-gated

1. **Artifact/wire-format change** — embedding a section changes the wasm bytes and
   therefore `blob_id`; the section format should be blessed.
2. **Consensus-relevant upgrade path** — adds a new way for an upgrade to be rejected
   (`IdentityDowngradeForbidden`) in `update_application`'s emit path.
3. **Policy surface** — the eventual `unsafe_strip_identity` + governance allowance is
   a policy decision (deferred here, but the gate's shape anticipates it).

Land an approach comment on #2587 to get format sign-off before the large node-side
work, then implement as one PR with layered commits (emit/embed → read → gate).
