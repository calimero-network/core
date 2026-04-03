# PR #2096 Split Plan

PR #2096 (`feat/namespace-identity`) has grown to **199 files changed, 9,296 insertions, 26,134 deletions** across 60+ commits. It mixes multiple unrelated concerns. This document proposes splitting it into **7 focused PRs**, each doing one thing, with the required changes to external repos (`merobox`, `mero-js`, `calimero-client-py`) called out per-PR.

---

## PR 1: Namespace Identity Model (per-root-group keypairs)

**Branch:** `feat-namespace-identity-model`

**What it does:** Replaces the single global `GroupIdentityConfig` with per-namespace (root group) Ed25519 keypairs stored in the datastore.

### Core changes

| Area | Files | Description |
|------|-------|-------------|
| Store keys | `crates/store/src/key/group.rs` | Add `NamespaceIdentity` store key (prefix `0x36`) |
| Store types | `crates/store/src/types/group.rs` | Add `NamespaceIdentityValue` type |
| Context/group_store | `crates/context/src/group_store.rs` | Add `resolve_namespace()`, `get_or_create_namespace_identity()`, `store_namespace_identity()` |
| Context handlers | `create_group.rs`, `join_group.rs`, `create_context.rs` | Use `get_or_create_namespace_identity()` instead of `new_identity()` |
| All 30+ mutation handlers | `add_group_members.rs`, `remove_group_members.rs`, etc. | Replace `node_group_identity()` with `node_namespace_identity(&group_id)` |
| Node primitives | `crates/node/primitives/src/lib.rs` | Remove `GroupIdentityConfig` struct |
| Config | `crates/config/src/lib.rs` | Remove `group_identity` from `NodeConfig`, `IdentityConfig`, `ConfigFile` |
| merod init | `crates/merod/src/cli/init.rs` | Remove group keypair generation |
| meroctl | `crates/meroctl/src/cli/node.rs` | Update `node identity` to show `peer_id` instead |
| Primitives | `crates/primitives/src/common.rs`, `hash.rs` | Add `Hash::zero()`, `ZERO_HASH` constants |
| Fleet-join | `crates/server/src/admin/handlers/tee/fleet_join.rs` | Use namespace identity for TEE attestation + auto-join contexts |

### Commits from PR #2096 that belong here

- `6a28ae53` feat: namespace identity model (per-root-group keypairs)
- `db0c4544` feat: auto-generate namespace identity in create_group, join_group, create_context
- `7b1a432e` feat: fleet-join uses namespace identity + auto-joins all group contexts
- `2a0800a0` chore: remove GroupIdentityConfig from config, init, and node
- `4305a516` fix: resolve CI errors -- add store key re-exports
- `8e47c110` fix: deref PrivateKey to [u8; 32]
- `f256b499` feat: application inheritance -- subgroups inherit target_application_id
- `194bd71e` fix: add PredefinedEntry impl for NamespaceIdentity
- `6af0c389` fix: address PR review comments and CI build error
- `21bef061` refactor: add named constants for zero application ID and zero hash

### External repo changes

| Repo | Change needed |
|------|---------------|
| **merobox** | None -- merobox doesn't interact with `GroupIdentityConfig` or namespace identity directly |
| **mero-js** | None -- no admin API routes change in this PR |
| **calimero-client-py** | None -- no client API surface changes |

---

## PR 2: Namespace Admin API + CLI

**Branch:** `feat-namespace-admin-api`  
**Depends on:** PR 1

**What it does:** Adds `/admin-api/namespaces` endpoints and `meroctl namespace` CLI commands.

### Core changes

| Area | Files | Description |
|------|-------|-------------|
| Server handlers | `crates/server/src/admin/handlers/namespaces/` (new dir: `list.rs`, `get_identity.rs`, `list_for_application.rs`, `mod.rs`) | 3 new endpoints: `GET /admin-api/namespaces`, `GET /admin-api/namespaces/:id/identity`, `GET /admin-api/namespaces/for-application/:app_id` |
| Server admin types | `crates/server/primitives/src/admin.rs` | Add `ListNamespacesApiResponse`, `NamespaceSummary`, `NamespaceIdentityApiResponse` |
| Server routes | `crates/server/src/admin/service.rs` | Wire new namespace routes |
| Context messages | `crates/context/primitives/src/messages.rs` | Add `ListNamespaces`, `GetNamespaceIdentity`, `ListNamespacesForApplication` variants |
| Context client | `crates/context/primitives/src/client.rs` | Add corresponding client methods |
| Context handlers | `crates/context/src/handlers/list_namespaces.rs`, `list_namespaces_for_application.rs`, `get_namespace_identity.rs` | New handler files |
| Client lib | `crates/client/src/client/namespace.rs` (new) | `list_namespaces()`, `get_namespace_identity()`, `list_namespaces_for_application()` |
| meroctl | `crates/meroctl/src/cli/namespace.rs` (new), `namespace/list.rs`, `namespace/identity.rs` | `meroctl namespace ls`, `meroctl namespace identity <id>` |

### Commits from PR #2096

- `64448a24` feat: add /admin-api/namespaces endpoints
- `8ea684b5` feat: add meroctl namespace subcommand + calimero-client namespace methods
- `fa9af4d9` fix: update meroctl grant/revoke to error with group API hint, fix namespace output

### External repo changes

| Repo | Change needed |
|------|---------------|
| **merobox** | Add `merobox namespace` CLI commands (optional, can be deferred) |
| **mero-js** | Add `listNamespaces()`, `getNamespaceIdentity()`, `listNamespacesForApplication()` to `AdminApiClient`. Add corresponding types to `admin-types.ts` |
| **calimero-client-py** | Add `list_namespaces()`, `get_namespace_identity()`, `list_namespaces_for_application()` to `PyClient` in `src/client.rs` |

---

## PR 3: Remove Per-Context Authorization (visibility/allowlist/invite/join)

**Branch:** `refactor-remove-context-authorization`  
**Depends on:** PR 1

**What it does:** Removes the legacy per-context authorization model (visibility, allowlists, context-level invitations, context-level join) in favor of groups-based access control.

### Core changes — Deleted endpoints

- `POST /contexts/join` (join_context)
- `POST /contexts/invite` (invite_to_context)
- `GET/PUT /groups/:id/contexts/:cid/visibility`
- `GET/POST /groups/:id/contexts/:cid/allowlist`

### Core changes — Deleted concepts

- Per-context visibility (Open/Restricted)
- Per-context allowlists
- Context-level invitations (`SignedOpenInvitation`)
- Ghost member hack in `join_context`

### Core changes — Files affected

| Area | Files | Description |
|------|-------|-------------|
| Server handlers (deleted) | `invite_to_context.rs`, `grant_capabilities.rs`, `revoke_capabilities.rs`, `get_context_visibility.rs`, `set_context_visibility.rs`, `get_context_allowlist.rs`, `manage_context_allowlist.rs`, `join_group_context.rs` | Delete all these handler files |
| Server routes | `service.rs` | Remove deleted routes |
| Server admin types | `admin.rs` | Remove request/response types for deleted endpoints |
| Context handlers (deleted) | `get_context_allowlist.rs`, `get_context_visibility.rs`, `grant_context_capabilities.rs`, `revoke_context_capabilities.rs`, `set_context_visibility.rs`, `store_context_allowlist.rs`, `store_context_visibility.rs`, `manage_context_allowlist.rs`, `join_group_context.rs` | Delete handler files |
| Context handlers (modified) | `join_context.rs` | Rewrite: becomes `POST /contexts/:id/join` (group membership check only) |
| Context messages | `messages.rs` | Remove deleted variants |
| Context client | `client.rs` | Remove deleted methods |
| Client lib | `client.rs`, `client/group.rs` | Remove `invite_to_context()`, `join_context()`, `grant_permissions()`, `revoke_permissions()`, visibility/allowlist methods. Add `join_context()` (new simplified version). Rename `join_group_context` -> `join_context` |
| meroctl | `cli/context/invite.rs` (deleted), `cli/context/join.rs` (deleted), `cli/group/join_group_context.rs` -> `join_context.rs` | Remove old CLI commands, add new simplified ones |
| Store keys | `key/group.rs` | Remove `GroupContextVisibility`, `GroupContextAllowlist` keys |
| Context primitives | `group.rs` | Remove visibility/allowlist related types |

### Commits from PR #2096

- `9968f4f1` fix: move capabilities grant/revoke from context-scoped to group-scoped
- `b11f0e16` refactor: remove per-context capabilities (keep group-level only)
- `59177af0` chore: remove grant/revoke_permissions from calimero-client
- `40639c13` refactor: remove per-context join/invite/visibility/allowlist (groups-only model)
- `ccb62a97` refactor: rename join_group_context to join_context, simplify API

### External repo changes

| Repo | Change needed |
|------|---------------|
| **merobox** | **`commands/join.py`**: Remove `join_context_via_admin_api` or update it to use `POST /contexts/:id/join` (no body). **`commands/group.py`**: `join-context` subcommand already calls `join_context(context_id)` correctly. **`commands/identity.py`**: Remove `invite_to_context` / `join_context` references if any. Remove `grant_permissions`/`revoke_permissions` usage if any. |
| **mero-js** | **`admin-types.ts`**: Remove `InviteToContextRequest`, `JoinContextRequest`, `JoinContextResponseData` (old shapes). **`admin-client.ts`**: Remove `inviteToContext()` and the old `joinContext()`. Add new `joinContext(contextId: string)` that calls `POST /admin-api/contexts/:id/join` with no body. |
| **calimero-client-py** | **`src/client.rs`**: Remove `invite_to_context()`, `join_context()` (old), `grant_permissions()`, `revoke_permissions()`. Add new `join_context(context_id)` that calls `POST /admin-api/contexts/:id/join`. Update README accordingly. |

---

## PR 4: Multi-Service Bundle Support

**Branch:** `feat-multi-service-bundles`  
**Depends on:** None (independent of PRs 1-3)

**What it does:** Adds multi-WASM-service bundle support to the manifest, store model, install pipeline, and context creation.

### Core changes

| Area | Files | Description |
|------|-------|-------------|
| Bundle primitives | `crates/node/primitives/src/bundle/mod.rs` | Add `BundleService`, `services` field to `BundleManifest`, `wasm_artifacts()`, `to_metadata_json()` |
| Store types | `crates/store/src/types/application.rs` | Add `ServiceMeta`, `services` field to `ApplicationMeta`, `resolve_service()` |
| Store types | `crates/store/src/types/context.rs` | Add `service_name` to `ContextMeta` |
| Context primitives | `crates/primitives/src/context.rs` | Add `service_name` to `Context` |
| Context messages | `messages.rs` | Add `service_name` to `CreateContextRequest` |
| Application client | `crates/node/primitives/src/client/application.rs` | Split `install_application` into `install_raw_wasm()` and `install_bundle_application()`. Add `services` param. Extract `install_verified_bundle()`. |
| Context handlers | `create_context.rs` | Pass `service_name` through to context metadata |
| Execute handler | `execute.rs` | Pass `context.service_name` when loading module |
| Update application | `update_application.rs` | Use `context.service_name` in `get_module` |
| Sync | `state_delta.rs` | Add missing `services` arg to `install_application` call |
| Server admin types | `admin.rs` | Add `serviceName` to `CreateContextRequest` |
| Server handler | `create_context.rs` | Pass `serviceName` through |
| Client | `client.rs` | Add `service_name` to `create_context()` |
| meroctl | `cli/context/create.rs` | Add `--service` flag |
| merodb | `migration/test_utils.rs` | Add `service_name` to test `ContextMeta` |

### Commits from PR #2096

- `151b909d` feat: multi-service bundle support -- manifest and store model
- `9993242a` feat: add service_name to Context model and CreateContextRequest
- `4e953375` feat: multi-service install logic (WIP)
- `8418668d` feat: wire service_name through get_module, create_context, and execute
- `a498deaa` feat: add --service flag to meroctl context create
- `9340ffad` fix: re-export ServiceMeta from store types module
- `886c093d` fix: add missing services arg to install_application call in sync.rs
- `4586a37c` fix: set service_name on context meta at handler level
- `30f2503e` fix: add services: None to BundleManifest test initializations
- `93a27e80` refactor: apply DRY/SOLID principles to multi-service bundle pipeline
- `bdb89291` feat: implement per-service WASM blob resolution in get_module

### External repo changes

| Repo | Change needed |
|------|---------------|
| **merobox** | **`commands/context.py`** (if it exists): Add optional `--service` / `service_name` parameter when creating contexts. Workflows that create contexts may add `serviceName` field. |
| **mero-js** | **`admin-types.ts`**: Add `serviceName?: string` to `CreateContextRequest`. Add `services?: ApplicationService[]` to `Application` type. **`admin-client.ts`**: No method changes needed (passes request object through). |
| **calimero-client-py** | **`src/client.rs`**: Add `service_name: Option<&str>` parameter to `create_context()`. Update `CreateContextRequest::new()` call to pass it. |

---

## PR 5: ECDH Group Key Envelopes (replace P2P key-share)

**Branch:** `feat-ecdh-group-key-envelopes`  
**Depends on:** PR 1

**What it does:** Replaces the ~1100-line challenge-response P2P key-share protocol with ECDH-wrapped key envelopes on the namespace governance DAG.

### Core changes

| Area | Files | Description |
|------|-------|-------------|
| Group store | `group_store.rs` | Add `KeyEnvelope`, `KeyRotation`, `GroupKeyEntry` types. Add `key_id` field. Key delivery/rotation logic. |
| Store keys | `key/group.rs` | Add `GroupKey` store key type |
| Sync/key (deleted) | `crates/node/src/sync/key.rs` (469 lines) | Delete entire challenge-response key-share protocol |
| State delta | `state_delta.rs` | Remove `request_key_share_with_peer()` (~400 lines), `ensure_author_sender_key()` (~75 lines) |
| Network event | `network_event.rs` | Remove `KeyShare/Challenge/ChallengeResponse` wire type handling |
| Wire types | `crates/node/primitives/src/sync/wire.rs` | Remove key-share related variants |
| Local governance | `local_governance.rs` | Add `KeyDelivery` handling, retry encrypted ops |
| Context group | `crates/context/primitives/src/group.rs` | Add `key_id` to `NamespaceOp::Group`, `ContextRegistered` carries `application_id`, `blob_id`, `source` |
| Delta buffer | `delta_buffer.rs` | Add `key_id` to `BroadcastMessage::StateDelta` |
| Key delivery | `crates/node/src/key_delivery.rs` (new) | Reactive key delivery after `MemberJoined` |

### Commits from PR #2096

- `91757b10` feat: replace P2P key-share with ECDH group key envelopes

### External repo changes

| Repo | Change needed |
|------|---------------|
| **merobox** | None -- wire protocol / internal sync, no API changes |
| **mero-js** | None |
| **calimero-client-py** | None |

---

## PR 6: Namespace Governance Sync + Join Flow Fixes

**Branch:** `fix-namespace-governance-sync`  
**Depends on:** PRs 1, 5

**What it does:** Fixes the join_group flow end-to-end: proper namespace governance sync via stream protocol, sender_key ordering, heartbeat-based catch-up, auto-join context subscription.

### Core changes

| Area | Files | Description |
|------|-------|-------------|
| Join group handler | `join_group.rs` | Reorder phases: 1) store local state + sender_key, 2) subscribe + sync namespace ops, 3) publish MemberJoined + auto-join |
| Sync manager | `sync/manager.rs` | Add `ns_sync_tx/rx` channel, `sync_namespace_from_peer()` via stream protocol |
| Governance DAG | `governance_dag.rs` | Add ancestry hash, `compute_governance_ancestry_hash()`, `collect_governance_ancestry_heads()` |
| State delta | `state_delta.rs` | Delta-triggered governance catch-up, ancestry hash comparison |
| Execute | `execute.rs` | Produces ancestry hash instead of single-group heads |
| Broadcast group | Various | Fix gossipsub mesh timing, add broadcast_group_local_state changes |
| Context lib | `crates/context/src/lib.rs` | Add `sync_namespace()` method, `ns_sync_tx` to `NodeClient` |
| Node run | `run.rs` | Wire `ns_sync_tx/rx` channel |

### Commits from PR #2096

- `0ca194eb` fix: call sync_context_config in join_group auto-join loop
- `f17d1172` fix: wait for gossipsub mesh before publishing governance ops
- `fc580403` feat: delta-triggered governance catch-up
- `08aed881` feat: governance ancestry hash for full parent-chain divergence detection
- `5b816f82` feat: commit-reveal group join flow with merobox relay
- `f449fa5d` fix: joiner signs+publishes MemberJoined without local apply
- `fba4e218` fix: audit fixes — admin escalation, double writes, namespace boundary
- `2b61b70c` fix: remove metadata poll from join_group
- `2578fca0` fix: store stub group metadata on joiner + trigger sync after join
- `9325e928` fix: active namespace catchup on join via heartbeat push
- `8ac83e38` feat: proper namespace governance sync via stream protocol
- `13662f7a` fix: store sender_key BEFORE namespace sync so encrypted ops decrypt

### External repo changes

| Repo | Change needed |
|------|---------------|
| **merobox** | None -- internal sync protocol, no API surface changes |
| **mero-js** | None |
| **calimero-client-py** | None |

---

## PR 7: Workflow Migration + Refactoring + Docs + Cleanup

**Branch:** `refactor-workflow-migration-cleanup`  
**Depends on:** PRs 1, 3

**What it does:** Migrates E2E workflows from invite+join to group flow. Applies DRY/SOLID refactoring to group_store. Converts large files to directory modules. Removes stale readme/ doc directories (replaced by architecture/ docs site). Updates architecture docs.

### Core changes — Workflow migration

| File | Description |
|------|-------------|
| `apps/kv-store/workflows/workflow-example.yml` | Replace invite+join steps with create_group_invitation + join_group |
| `apps/sync-test/workflows/three-node-sync.yml` | Same migration |
| `apps/sync-test/workflows/six-node-sync.yml` | Same migration |
| `apps/e2e-kv-store/workflows/e2e.yml` | Same migration |
| `apps/e2e-kv-store/workflows/groups.yml` | Remove redundant join_context after join_group |
| `apps/xcall-example/workflows/xcall.yml` | Same migration |
| `apps/xcall-example/workflows/xcall-example.yml` | Same migration |

### Core changes — Refactoring

| Area | Files | Description |
|------|-------|-------------|
| group_store | `group_store.rs` -> `group_store/mod.rs` + `tests.rs` | Convert to directory module, extract tests. Add `GroupHandle`, `NamespaceHandle`, `GovernancePublisher`, `GroupStoreIndex`. DRY helpers. `GroupStoreError` typed error enum. |
| GovernancePreflight | `crates/context/src/lib.rs`, 10 handler files | Add `GovernancePreflight` helper, migrate 10 governance handlers to reduce boilerplate |
| Directory modules | `sync/manager.rs`, `server/primitives/admin.rs`, `store/key/group.rs`, `local_governance.rs`, `execute.rs`, `update_application.rs`, `state_delta.rs`, `kms.rs`, `client.rs` | Convert large files to directory modules |

### Core changes — Documentation

| Area | Description |
|------|-------------|
| `architecture/*.html` | Update concepts, glossary, storage-schema, config-reference, local-governance, system-overview, sequence-diagrams, wire-protocol, crates/* docs |
| Deleted `readme/` dirs | `crates/dag/readme/`, `crates/node/readme/`, `crates/storage/readme/`, `crates/network/ARCHITECTURE.md` (moved to docs site) |

### Core changes — CI

| File | Description |
|------|-------------|
| `.github/actions/setup-merobox/action.yml` | Point to correct merobox branch (temporary, revert after merobox merges) |
| `Cargo.toml` | Version bump to `0.10.1-rc.18` |

### Commits from PR #2096

- `355465e3` refactor: migrate core/apps workflows from invite+join to group flow
- `59e642e7` refactor: migrate remaining app workflows to group flow
- `5ad75eb7` refactor: DRY cleanup of group_store.rs
- `6611eb94` refactor: introduce encapsulation types for group store API boundaries
- `ccee31e3` refactor: convert group_store.rs to directory module, extract tests
- `bdac7ea1` refactor: add GovernancePreflight helper, migrate add_group_members handler
- `394cf82f` refactor: convert 4 large files to directory modules, extract tests
- `99f91856` refactor: convert 5 more large files to directory modules
- `e51c3b30` refactor: introduce GroupStoreError typed error enum
- `bc613044` refactor: migrate 9 governance handlers to GovernancePreflight
- `00741f9d` docs: update architecture docs for namespace identity model
- `6a04e39e` docs: update architecture for multi-service bundles
- `7b41961b` docs: update architecture for groups-only authorization model
- `8c4de2ac` chore: bump version to 0.10.1-rc.18
- `5c1b913e` ci: point setup-merobox to feat/namespace-governance-rewrite branch
- Plus various `cargo fmt` and CI fix commits

### External repo changes

| Repo | Change needed |
|------|---------------|
| **merobox** | **Workflow YAML files** in `workflow-examples/` and `example-project/` may need updating to use `create_group_invitation` + `join_group` instead of old `invite_identity` + `join_context` steps. |
| **mero-js** | None |
| **calimero-client-py** | None |

---

## Dependency Graph

```
PR 1: Namespace Identity Model
 ├── PR 2: Namespace Admin API + CLI
 ├── PR 3: Remove Per-Context Authorization ──→ PR 7: Workflows + Refactoring + Docs
 ├── PR 5: ECDH Group Key Envelopes ──→ PR 6: Namespace Governance Sync
 └── (PR 7 also depends on PR 1)

PR 4: Multi-Service Bundles (independent, can merge anytime)
```

### Recommended merge order

1. **PR 4** (Multi-Service Bundles) — fully independent, can merge first
2. **PR 1** (Namespace Identity Model) — foundation for all others
3. **PR 2** (Namespace Admin API) — depends on PR 1
4. **PR 3** (Remove Per-Context Auth) — depends on PR 1
5. **PR 5** (ECDH Group Keys) — depends on PR 1
6. **PR 6** (Governance Sync Fixes) — depends on PR 1 + PR 5
7. **PR 7** (Workflows + Refactoring + Docs) — depends on PR 1 + PR 3

---

## External Repository Summary

### `calimero-network/merobox` (Python CLI + workflows)

| PR | Impact |
|----|--------|
| PR 2 | Optional: add `merobox namespace` commands |
| PR 3 | Update `commands/join.py` to use new `POST /contexts/:id/join` (no body). Remove old invite/join/visibility/allowlist references |
| PR 7 | Update workflow YAML examples to use group invitation flow |

### `calimero-network/mero-js` (TypeScript SDK)

| PR | Impact |
|----|--------|
| PR 2 | Add namespace API methods + types |
| PR 3 | Remove `inviteToContext()`, old `joinContext()`. Add new `joinContext(contextId)`. Remove old invite/join types |
| PR 4 | Add `serviceName?` to `CreateContextRequest` type, `services?` to `Application` type |

### `calimero-network/calimero-client-py` (Python/Rust client)

| PR | Impact |
|----|--------|
| PR 2 | Add `list_namespaces()`, `get_namespace_identity()`, `list_namespaces_for_application()` |
| PR 3 | Remove `invite_to_context()`, `grant_permissions()`, `revoke_permissions()`, old `join_context()`. Add new `join_context(context_id)` |
| PR 4 | Add `service_name` param to `create_context()` |

---

## Notes

- Each PR should be buildable and pass CI independently (after its dependencies are merged).
- The version bump (`0.10.1-rc.18`) should go in the last PR to merge (PR 7).
- The `setup-merobox` CI change pointing to a branch should be reverted once all merobox changes are merged to master.
- `cargo fmt` and `cargo clippy` commits are not listed separately; each PR should be formatted before submission.
