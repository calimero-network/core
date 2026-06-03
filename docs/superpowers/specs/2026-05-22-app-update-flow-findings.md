# Application Update Flow — Findings (Current State)

**Date:** 2026-05-22
**Scope:** End-to-end audit of how application (WASM) version updates work today across `calimero-network/core` and `calimero-network/app-registry`.
**Status:** Findings only. Proposed design is a separate document.

---

## 1. Summary

Calimero has a real, well-modelled upgrade pipeline in **core** (governance ops → DAG replication → per-context swap, with optional WASM-level state migration), but it is enforced and tested almost entirely at the governance-op layer. There is no end-to-end proof the full WASM-swap + state-migration path works, no recovery from stuck propagators, no signer rotation, and `cascade_target_application` is a deliberate no-op stub.

The **app-registry** is a passive, pull-based, currently-unauthenticated package store. It does not notify, does not coordinate group upgrades, and does not carry migration metadata that anyone consumes.

End users are notified of new releases **nowhere**. Admins must poll `get_group_upgrade_status`.

---

## 2. Data model: what a "version" is

In core:

- `ApplicationMeta.version: Box<str>` — opaque semver string (`crates/store/src/types/application.rs:26`).
- `Application.version`, `Application.package`, `Application.signer_id` (did:key) — the publisher's cryptographic identity (`crates/primitives/src/application.rs:430`).
- **Application identity = blob content hash** (`ApplicationId = Hash([u8;32])`). The version string is metadata; the hash is the primary key. There is no `schema_version`, no compatibility range, no `min_compatible_with` field.
- `AppKey { package, signer_id }` is the stable identity across versions (`primitives/src/application.rs:224-261`). Updates are only accepted if the new signer matches the installed one — see `verify_appkey_continuity` (`context/src/handlers/update_application/mod.rs:275`).

---

## 3. How an upgrade actually flows

There are **two layers** that must not be confused.

### 3.1 Layer A — Install (admin RPC)

`/install-application` (`crates/server/src/admin/handlers/applications/install_application.rs:13`) takes a URL+hash, calls `node_client.install_application_from_url`, returns an `ApplicationId`. Gated by `ApplicationPermission::Install` (`crates/auth/src/auth/permissions/types.rs:67`).

This only puts the WASM blob in the node's blob store. It does **not** switch any running context to it.

### 3.2 Layer B — Group upgrade (governance)

`UpgradeGroupRequest { group_id, target_application_id, requester, migration }` lands in `upgrade_group` handler (`context/src/handlers/upgrade_group.rs:19`). This writes two DAG-replicated ops:

- `GroupOp::TargetApplicationSet { app_key, target_application_id }` — "the new target for this group is X"
- `GroupOp::GroupMigrationSet { migration: Option<Vec<u8>> }` — optional name of a WASM export to run as the migration function

Three policies are encoded:

| Policy | Behaviour |
|---|---|
| `LazyOnAccess` (default) | Just publishes the governance ops; each context picks up the new app on its **next execution** — zero coordination, zero races. |
| `Automatic` | Spawns a propagator that walks every context in the group and calls `update_application` per-context with retry (`MAX_AUTO_RETRIES`, exponential backoff at `upgrade_group.rs:684`). |
| `Coordinated { deadline }` | Same as Automatic with a deadline. |

The actual swap happens in `update_application` (`context/src/handlers/update_application/mod.rs:34`):

- If `migration.is_none()` and target equals current → skip.
- Else invalidate cached WASM module, reload, then either `update_application_id` (no migration) or `update_application_with_migration` (run the named export → returns new Borsh-encoded state → `write_migration_state` → bump root hash and DAG heads).

---

## 4. Propagation across peers

It is **DAG-replicated governance ops, not push notifications**. Peers see `TargetApplicationSet` / `GroupMigrationSet` come in over the same sync mechanism as every other governance op. Convergence is tested in `context/tests/local_group_governance_convergence.rs:150-210` — two nodes with separate stores converge on the same `target_application_id`, `app_key`, `migration` after gossip.

There is **no consensus vote** — it is admin-signed and asymmetric. There is **no "new version available" gossip from the registry** — the registry is passive (see §6).

---

## 5. Access control

- **Install** (node-local): `ApplicationPermission::Install` admin capability.
- **Group upgrade**: `ContextApplicationPermission::Update` plus a group-admin role signing the `TargetApplicationSet` op. No per-member vote; this is a **unilateral admin action** propagated to all peers via governance.
- **Signer continuity**: only the original publisher (matching `signer_id`) can ship an upgrade. **No key rotation yet** — the code comment at `update_application/mod.rs:296` lists it as a future extension requiring explicit context-governance authorisation.

---

## 6. Notifications / user-facing visibility

- **No dedicated `ApplicationUpdated` / `VersionAvailable` event.** State changes ride on `NodeEvent::Context(StateMutation)` (`update_application/mod.rs:470-475`).
- The admin/UI **polls** `get_group_upgrade_status` (`context/src/handlers/get_group_upgrade_status.rs:1`) to see `GroupUpgradeInfo { from_version, to_version, migration, initiated_at, initiated_by, status: InProgress { total, completed, failed } | Completed }`.
- No WebSocket-level "you should upgrade" signal to end users.

---

## 7. App-registry: what it does and does not do

**Short answer:** distribution only, not upgrades.

- **What it is:** A Vercel-hosted Fastify backend + React frontend + CLI (`packages/backend`, `packages/frontend`, `packages/cli`). Stores bundles in Vercel KV, WASM blobs in `/artifacts`. Marked "Production Ready" in README.
- **Versioning:** Multi-version per package, immutable, semver-sorted descending (`BundleStorageKV.js:138-158`). First-write-wins via Redis `setNX`. `ALLOW_BUNDLE_OVERWRITE=true` is a server-side env override only "for migrations".
- **Publish auth:** **Currently unauthenticated.** Ed25519 `signature` field is accepted but not enforced (`server.js:376-389`). `DEVELOPER_ENROLLMENT_PLAN.md` describes a full enrollment + namespace ownership scheme that is **not yet implemented** (marked "Planning").
- **Consumer reads:** `GET /api/v2/bundles[?package=&version=&developer=]`, `GET /api/v2/bundles/:package/:version`, `GET /artifacts/*` — **pull only**.
- **Notifications / push:** **None.** Grep for `webhook|subscribe|notify|observer|pubsub` returns zero hits. No SSE, no WebSocket, no email, no webhook.
- **Group-update orchestration:** **None.** Registry has no awareness of which contexts run which version, and no endpoint to trigger or schedule an upgrade for a context or group.
- **Migration metadata:** A `migrations: []` field exists in the V2 manifest but is **never validated or used**. `min_runtime_version` is stored but enforcement is the runtime's job. No "breaking change" / "compatible-with" / "rollback" metadata.
- **Tests:** publish + storage well-covered (`v2-e2e-*.test.js`, `version-sorting.test.js`, `push-validation.test.js`); **no core ↔ registry e2e**.

---

## 8. Test coverage — honest accounting

| Surface | Coverage |
|---|---|
| Governance op replication (`TargetApplicationSet`, `GroupMigrationSet` converging) | ✅ `local_group_governance_convergence.rs:150-210` |
| End-to-end install → group announce → per-context apply → execute on new WASM | ❌ Not found |
| Migration function actually runs and rewrites state | ❌ No isolated test |
| `LazyOnAccess` vs `Automatic` vs `Coordinated` policy behaviour | ❌ Not found |
| Signer-mismatch rejection (`verify_appkey_continuity`) | ❌ Not found |
| Propagator retry exhaustion / stuck `InProgress` recovery | ❌ Not found |
| Registry ↔ node fetch + version selection | ❌ Registry tests are backend-isolated |
| Merobox workflow that bumps an app version mid-context | ❌ Not found |

`workflows/fuzzy-tests/` has group-governance / kv-store fuzzers but none of them exercise an in-place upgrade.

---

## 9. Concrete issues / known gaps

1. **`cascade_target_application` is a no-op stub** (`group_store/mod.rs:~1941`, prior session finding obs #6885) — child groups in a hierarchy do not automatically inherit a parent's `TargetApplicationSet`. The comment claims namespace-level governance handles it; this assumption needs verification.
2. **No e2e proof the migration path works.** The code is clean and modular, but every claim about "migration runs, state survives, peers converge" rests on unit tests of the governance layer, not on a real WASM swap.
3. **Stuck upgrades have no escape hatch.** After `MAX_AUTO_RETRIES`, failed contexts sit in `InProgress` forever. No admin-cancel, no timeout-to-failed, no manual force.
4. **`LazyOnAccess` ↔ `Automatic` race** is called out in a code comment (`upgrade_group.rs:111-114`) — "could invoke migration functions twice" — but not tested.
5. **No signer key rotation.** A compromised or lost publisher key bricks future upgrades for that app's namespace. The code comment at `update_application/mod.rs:296` acknowledges this as TODO.
6. **No schema/compat metadata.** The runtime cannot refuse an incompatible migration; the WASM author has to encode all compatibility logic themselves.
7. **Registry has no notion of consumers.** No "who's running 0.1.0?" query, no "notify admins of context X that 0.2.0 shipped" hook. The admin UI would have to poll `GET /api/v2/bundles/:package` and diff locally.
8. **Registry publish is unauthenticated** in production code today — anyone can publish under any namespace not already taken. The enrollment plan is paper-only.
9. **`min_runtime_version` and `migrations` fields in the manifest are dead metadata** — declared in the V2 schema but neither the registry nor (as far as observed) the node enforces them.

---

## 10. Improvement opportunities, ranked

### High-leverage

- **End-to-end upgrade test in merobox**: install v1 → create context → write state → publish v2 with a `migrate_v1_to_v2` export → `upgrade_group` with migration → assert state correctly transformed on all peers. This single workflow would catch most of the silent gaps in §9.
- **Registry → node "check for updates" endpoint** plus a node-side periodic poller that emits a `NodeEvent::ApplicationUpdateAvailable { context_id, current, available, signer_match }` so admin UIs can surface it without business logic in the registry.
- **Implement registry auth** (Ed25519-signed publish + namespace ownership) per the existing enrollment plan; until then `app-registry` cannot be trusted as the source of truth for "what is the canonical v0.2.0".

### Medium

- **Stuck-upgrade recovery** in `upgrade_group`: admin RPC to mark contexts `Failed` and re-trigger, plus a status timeout.
- **Schema-version and compat metadata** in `ApplicationMeta` and registry manifest, with the node refusing to apply a `target_application_id` whose `min_runtime_version` exceeds what it speaks.
- **Implement `cascade_target_application`** for hierarchical groups, or delete the stub and document the namespace-only path explicitly.

### Lower / nice-to-have

- Signer key rotation governance op.
- `min_runtime_version` enforcement on `install_application`.
- Webhook / SSE in the registry so admin UIs don't have to poll.

---

## 11. Pointers (file:line)

Core:

- `crates/store/src/types/application.rs:26` — `ApplicationMeta.version`
- `crates/primitives/src/application.rs:224-261, 430` — `AppKey`, `Application`
- `crates/server/src/admin/handlers/applications/install_application.rs:13` — install RPC
- `crates/context/src/handlers/upgrade_group.rs:19, 111-114, 684` — group upgrade entry, lazy/auto race comment, retry backoff
- `crates/context/src/handlers/update_application/mod.rs:34, 275, 296, 470-475` — per-context apply, signer continuity, key-rotation TODO, mutation event
- `crates/context/src/handlers/get_group_upgrade_status.rs:1` — admin polling endpoint
- `crates/context/src/group_store/mod.rs:~1941` — `cascade_target_application` no-op
- `crates/context/primitives/src/group.rs:15-35` — `GroupUpgradeInfo`
- `crates/auth/src/auth/permissions/types.rs:63-69, 120-124` — install/update capabilities
- `crates/context/tests/local_group_governance_convergence.rs:150-210` — convergence test
- `crates/context/primitives/src/local_governance/mod.rs` — `GroupOp::TargetApplicationSet` / `GroupOp::GroupMigrationSet`

App-registry:

- `packages/backend/server.js:206-209, 259-324, 327-355, 376-389, 413-464` — read APIs, push, normalisation, artifacts
- `packages/backend/.../BundleStorageKV.js:57, 64-73, 138-158` — KV layout, immutability, semver sort
- `DEVELOPER_ENROLLMENT_PLAN.md` — enrollment (Planning, unimplemented)
- `MULTI_VERSION_WASM.md` — versioning rationale
- `API_FORMAT_STANDARD.md`, `api.yml` — V2 manifest schema (`migrations: []`, `min_runtime_version`)
- `__tests__/v2-e2e-*.test.js`, `push-validation.test.js`, `version-sorting.test.js`, `bundle-storage-validation.test.js` — coverage
