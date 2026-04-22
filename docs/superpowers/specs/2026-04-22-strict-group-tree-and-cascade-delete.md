# Strict group-tree invariant + cascade delete

**Status**: spec, pending implementation plan
**Supersedes**: closed issue #2174, closed PR #2175
**Author**: driven by admin@calimero.network; spec drafted in collaboration with Claude
**Date**: 2026-04-22

## 1. Motivation

`delete_group` fails on orphaned groups (groups whose parent edge has been removed via `unnest_group`) because implicit-requester resolution walks the parent chain. PR #2175 tried to patch this with a fallback chain (group-level signing key lookup → namespace-identity public-key scan → governance-preflight signing-key capture). Each layer exposed another gap. The root cause is upstream: **orphan state exists at all** — it's producible via `unnest_group`, and even `create_group_in_namespace` transiently produces it between its `GroupCreated` and `GroupNested` ops.

Fix the state machine, not the identity resolver: make orphan state structurally impossible.

## 2. Invariant

> Every group in a namespace forms a tree rooted at the namespace root. Every group except the root has exactly one parent, at all times, in every persisted state, on every peer, across all governance-op applications.

This is enforced at the governance-op layer. No application code, RPC path, or handler can observe or persist a parentless group (other than a namespace root). There is no valid intermediate state where this is not true.

## 3. Scope

### In scope

- Replace `RootOp::GroupNested` / `RootOp::GroupUnnested` with `RootOp::GroupReparented` (atomic edge swap).
- Make `RootOp::GroupCreated` carry a required `parent_id` field (no create-without-parent path).
- Extend `RootOp::GroupDeleted` to carry a full cascade payload (descendant groups + contained contexts), applied atomically.
- Drop `nest_group` / `unnest_group` REST endpoints, store functions, CLI subcommands, merobox step types.
- Update `create_group` and `create_group_in_namespace` handlers to emit a single `GroupCreated` op with `parent_id` set.
- Update `delete_group` handler to pre-compute and apply a cascade.
- Adjust all callers (calimero-client-py, meroctl, mero-drive via follow-up) to use `reparent_group` in place of the old pair.
- Remove PR #2175's dead-code helpers (`node_group_admin_identity`, `find_namespace_identity_by_public_key`, preflight signing-key capture).

### Out of scope

- Cross-namespace reparenting (forbidden by construction; ops are namespace-scoped).
- Retroactive migration of existing orphans in deployed networks. Pre-release, not supported.
- Group-tree moves that span multiple namespaces (future work; semantics unclear).
- Recovery of groups after their namespace is deleted (never was in scope).

## 4. Governance ops

File: `crates/context/primitives/src/local_governance/mod.rs`.

### 4.1. Modified variant

```rust
GroupCreated {
    group_id: [u8; 32],
    parent_id: [u8; 32],  // NEW — required, must exist in this namespace
}
```

Pre-fix form (`GroupCreated { group_id }`) is removed. No deserialization compatibility with older DAG logs — this project is pre-release, we discard.

### 4.2. New variant

```rust
GroupReparented {
    child_group_id: [u8; 32],
    new_parent_id: [u8; 32],
}
```

Atomic edge swap: removes the existing `GroupParentRef(child)` edge and writes the new one, plus updates both parents' child-indices, within a single RocksDB write batch inside the op-apply transaction. No window where `GroupParentRef(child)` is absent.

### 4.3. Modified variant

```rust
GroupDeleted {
    root_group_id: [u8; 32],
    cascade_group_ids: Vec<[u8; 32]>,     // NEW — descendants, children-first
    cascade_context_ids: Vec<[u8; 32]>,   // NEW — all contexts in subtree
}
```

Payload enumerated at the admin node via `collect_subtree_for_cascade`. Every peer applies and independently re-enumerates; if the local enumeration disagrees with the payload, the op is rejected (determinism check — prevents silent divergence).

### 4.4. Removed variants

```rust
GroupNested { parent_group_id, child_group_id }       // DELETED
GroupUnnested { parent_group_id, child_group_id }     // DELETED
```

### 4.5. Unchanged variants

`AdminChanged`, `PolicyUpdated`, `MemberJoined`, `MemberLeft`, and all other group-scoped ops are untouched.

## 5. Application semantics

File: `crates/context/src/group_store/namespace_governance.rs`.

### 5.1. `execute_group_created(op, group_id, parent_id)`

1. `require_namespace_admin(&op.signer)`
2. If group already exists: debug-log and return `Ok(())` (idempotency preserved).
3. Verify `parent_id` exists in this namespace (either the namespace root or a previously-`GroupCreated`'d group). Reject if not.
4. Write `GroupMetaValue { admin_identity: op.signer, target_application_id: inherited_from_parent, ... }` for `group_id`.
5. Write `GroupParentRef(group_id) = parent_id`.
6. Write `GroupChildIndex(parent_id, group_id)`.
7. Add `op.signer` as `GroupMemberRole::Admin` for the new group.

All writes in one RocksDB batch. If any step errors, the whole transaction is dropped.

### 5.2. `execute_group_reparented(op, child, new_parent)`

1. `require_namespace_admin(&op.signer)`.
2. Load `old_parent = GroupParentRef(child)`; reject with `"cannot reparent the namespace root"` if missing. (Namespace roots have no `GroupParentRef`; any other missing entry means the child doesn't exist, same rejection.)
3. If `new_parent == old_parent`: no-op, return `Ok(())` (idempotent).
4. Verify `new_parent` exists in this namespace.
5. **Cycle check**: call `is_descendant_of(store, new_parent, child)`. If true, reject with `"cycle: new_parent is a descendant of child"`.
6. Delete `GroupChildIndex(old_parent, child)`.
7. Write `GroupParentRef(child) = new_parent`.
8. Write `GroupChildIndex(new_parent, child)`.

### 5.3. `execute_group_deleted(op, root_group_id, cascade_group_ids, cascade_context_ids)`

1. `require_namespace_admin(&op.signer)`.
2. Reject if `root_group_id == namespace_id` with `"cannot delete the namespace root (use delete_namespace)"`.
3. **Determinism check**: run `collect_subtree_for_cascade(store, root_group_id)` locally; if the result's `(descendant_groups, contexts)` differs from the payload, reject with `"cascade payload mismatch — tree state diverged"`. Order-sensitive comparison for groups (children-first); context list compared as a set.
4. For each `group_id` in `cascade_group_ids` followed by `root_group_id` (children-first):
   - `delete_all_group_signing_keys(store, group_id)`
   - `delete_all_group_members(store, group_id)`
   - Delete all `GroupAlias` entries for `group_id`
   - `delete_group_meta(store, group_id)`
   - Delete `GroupParentRef(group_id)`
   - Delete `GroupChildIndex(parent, group_id)` for the known parent
5. For each `context_id` in `cascade_context_ids`:
   - Delete `GroupContext(context_id)` record
   - Notify `node_client` to unsubscribe and tear down per-context state
6. Emit one audit log entry listing all deleted group_ids and context_ids.

All of 4–5 in one RocksDB batch.

## 6. Store-layer API

File: `crates/context/src/group_store/namespace.rs` (unless noted).

### 6.1. Added

```rust
/// Atomic edge swap. Enforces: child is not the namespace root,
/// new_parent exists, no cycle. Idempotent on new_parent == old_parent.
pub fn reparent_group(
    store: &Store,
    child: &ContextGroupId,
    new_parent: &ContextGroupId,
) -> EyreResult<()>;

/// Walk the subtree rooted at `root` in children-first order.
/// Returns every descendant group_id (excluding root itself) plus every
/// context_id registered on any group in the subtree (including root).
pub fn collect_subtree_for_cascade(
    store: &Store,
    root: &ContextGroupId,
) -> EyreResult<CascadePayload>;

pub struct CascadePayload {
    pub descendant_groups: Vec<ContextGroupId>,
    pub contexts: Vec<ContextId>,
}

/// Bounded walk; reject cycles. O(depth), MAX_NAMESPACE_DEPTH cap applies.
pub fn is_descendant_of(
    store: &Store,
    candidate: &ContextGroupId,
    potential_ancestor: &ContextGroupId,
) -> EyreResult<bool>;
```

In `crates/context/src/group_store/membership.rs`:

```rust
/// Bulk delete used by cascade-delete. Mirrors delete_all_group_signing_keys.
pub fn delete_all_group_members(store: &Store, group_id: &ContextGroupId) -> EyreResult<()>;
```

### 6.2. Removed

```rust
pub fn nest_group(store, parent, child) -> EyreResult<()>;    // DELETED
pub fn unnest_group(store, parent, child) -> EyreResult<()>;  // DELETED
```

### 6.3. Re-exports

`crates/context/src/group_store/mod.rs`:

- Remove: `nest_group`, `unnest_group`
- Add: `reparent_group`, `collect_subtree_for_cascade`, `is_descendant_of`, `CascadePayload`

## 7. Handlers & REST endpoints

Directory: `crates/server/src/admin/handlers/groups/` (except where noted).

### 7.1. Added

- `reparent_group.rs` — `POST /admin-api/groups/:group_id/reparent` with body `{ newParentId: hex }`. Calls `sign_apply_and_publish_namespace_op` with `RootOp::GroupReparented`. Authorization via namespace admin.

### 7.2. Removed

- `nest_group.rs` — endpoint and handler deleted.
- `unnest_group.rs` — endpoint and handler deleted.
- Route registrations for both are dropped from `admin/service.rs`.

### 7.3. Modified

- `crates/server/src/admin/handlers/namespaces/create_group_in_namespace.rs` — emit a single `RootOp::GroupCreated { group_id, parent_id: namespace_id }`. Remove the second `GroupNested` op emission. Also applies to any path that creates a subgroup under a non-root parent.
- `crates/server/src/admin/handlers/groups/create_group.rs` — now requires `parent_id` in the request body (breaking API change). Emits single `GroupCreated` op.
- `crates/server/src/admin/handlers/groups/delete_group.rs` — build `CascadePayload` via `collect_subtree_for_cascade`, emit single `GroupDeleted` op with full cascade.

### 7.4. From PR #2175 — remove entirely

- `ContextManager::node_group_admin_identity` in `crates/context/src/lib.rs` — delete.
- `group_store::find_namespace_identity_by_public_key` — delete.
- Preflight signing-key capture logic in `ContextManager::governance_preflight` — revert to pre-#2175 shape.
- Error messages in the 6 inline handlers — revert to pre-#2175 text (or improve independently — orthogonal).

## 8. Client APIs

### 8.1. `crates/client/src/client/group.rs`

- Remove `nest_group`, `unnest_group` methods.
- Add `reparent_group(group_id, ReparentGroupApiRequest { new_parent_id, requester })`.
- `create_group` signature gains `parent_id` parameter.

### 8.2. `crates/meroctl/src/cli/group/`

- Delete `nest.rs` and `unnest.rs` subcommand files.
- Add `reparent.rs` — `meroctl group reparent --child <id> --new-parent <id>`.
- `meroctl group create` gains required `--parent <id>` flag.

### 8.3. calimero-client-py

Tracked in a separate follow-up PR in the calimero-client-py repo. Breaking: `nest_group()` and `unnest_group()` removed; `reparent_group()` added; `create_group()` requires `parent_id`. mero-drive's e2e workflows will need the matching client update to run.

### 8.4. merobox

Tracked in a separate follow-up PR in the merobox repo (`calimero-network/merobox`):

- Remove `nest_group` and `unnest_group` step types from `group_management.py`.
- Add `reparent_group` step type with fields `node`, `child_group_id`, `new_parent_id`.

## 9. Authorization

All three ops (`GroupCreated`, `GroupReparented`, `GroupDeleted`) require namespace admin signature. No per-descendant check on cascade delete — admin over the target subtree is implied by admin over the namespace.

Rationale: we already treat namespace admin as authoritative over every group in the namespace (`AdminChanged`, `PolicyUpdated`, `GroupCreated`, old `GroupNested`/`GroupUnnested` all check it). Keeping that model keeps this PR's authorization footprint zero — no new privilege surfaces.

Group-level admins (who are admin of a specific group but not the namespace) cannot reparent or cascade-delete their group. They must coordinate with the namespace admin. This mirrors the existing constraint for all other `RootOp` variants.

## 10. Testing strategy

### 10.1. Store-level unit tests

In `crates/context/src/group_store/tests.rs`:

- `reparent_group_swaps_edge_atomically` — verify `GroupParentRef` and both `GroupChildIndex` entries are correct after reparent.
- `reparent_group_is_idempotent_on_same_parent` — calling with `new_parent == old_parent` is a no-op that succeeds.
- `reparent_group_rejects_cycle` — `reparent(A, descendant_of_A)` returns `Err`.
- `reparent_group_rejects_namespace_root` — `reparent(namespace_id, anything)` returns `Err`.
- `reparent_group_rejects_nonexistent_new_parent` — returns `Err`.
- `collect_subtree_for_cascade_enumerates_children_first` — deterministic order.
- `collect_subtree_for_cascade_includes_contexts_from_all_descendants`.
- `is_descendant_of_bounded_walk` — terminates at `MAX_NAMESPACE_DEPTH`.

### 10.2. Governance-op application tests

In `crates/context/src/group_store/tests.rs`:

- `execute_group_created_requires_parent_exists` — applying `GroupCreated` with an unknown parent returns `Err`, no state written.
- `execute_group_reparented_swaps_edge` — op application produces the expected post-state.
- `execute_group_deleted_cascade_determinism_mismatch_rejected` — applying with a payload that disagrees with local enumeration returns `Err`.
- `execute_group_deleted_cleans_up_everything` — after cascade, no `GroupMeta`, `GroupMember`, `GroupSigningKey`, `GroupAlias`, `GroupParentRef`, `GroupChildIndex`, or `GroupContext` entries remain for any group in the subtree.

### 10.3. Merobox E2E workflows

New file `apps/e2e-kv-store/workflows/group-reparent-and-cascade-delete.yml`:

1. Create namespace, create three subgroups A, B, C under the namespace, with C also nested under B (so namespace → B → C, and namespace → A).
2. Create contexts in each subgroup.
3. `reparent_group(C, A)` — move C from B to A. Assert C is now listed as subgroup of A and not of B.
4. `delete_group(A)` — cascade-delete A including its subtree (C) and all contexts in A and C.
5. Assert: A, C, and all their contexts are gone; B and its contexts still exist; namespace is intact.

Wire into the PR-gating CI matrix (`e2e-rust-apps.yml`).

Delete the existing `group-nesting.yml` (which exercises the removed `nest_group`/`unnest_group` primitives). Replace with a `group-reparent.yml` that covers reparent-only scenarios.

### 10.4. Regression coverage

Every existing `group-*.yml` workflow is re-run to confirm the change doesn't break non-reparent flows. Any workflow currently using `nest_group` or `unnest_group` step types must be migrated to `reparent_group` in the same PR.

## 11. Implementation sequence

Suggested order — each step builds on the previous and can be verified independently:

1. **Primitives**: update `RootOp` enum in `context/primitives`. Compile errors will light up every callsite that needs updating — use this as a work-list.
2. **Store layer**: add `reparent_group`, `collect_subtree_for_cascade`, `is_descendant_of`, `delete_all_group_members`. Delete `nest_group`, `unnest_group`. Add unit tests.
3. **Governance execution**: update `execute_group_created`, add `execute_group_reparented`, update `execute_group_deleted`. Add tests.
4. **Handlers**: update `create_group`, `create_group_in_namespace`, `delete_group`. Add `reparent_group`. Delete `nest_group`, `unnest_group`. Update route registrations.
5. **Client & meroctl**: mirror the handler changes in the client/CLI surface.
6. **Cleanup**: remove PR #2175's residual helpers.
7. **E2E**: new and updated merobox workflows.
8. **Follow-up PRs** (separate repos, not blocking): merobox step-type updates, calimero-client-py API update, mero-drive migration.

## 12. Success criteria

- [ ] `RootOp` has no `GroupNested` / `GroupUnnested` variants; `GroupCreated` has required `parent_id`; new `GroupReparented` variant exists; `GroupDeleted` carries cascade payload.
- [ ] No code path (handler, RPC, store fn) writes or produces a group without a `GroupParentRef`, except for namespace roots.
- [ ] `cargo test -p calimero-context --lib` passes including all new tests.
- [ ] `cargo fmt --check` and `cargo clippy -- -A warnings` pass.
- [ ] New `group-reparent-and-cascade-delete` workflow passes in CI; other `group-*` workflows pass unchanged (or with `nest`/`unnest` step type calls migrated to `reparent`).
- [ ] PR #2175's fallback helpers (`node_group_admin_identity`, `find_namespace_identity_by_public_key`, preflight signing-key capture) are removed — no dead code.
- [ ] No mention of "orphan" state in any handler, error message, or doc comment except as a historical reference in the spec/changelog.

## 13. Out-of-scope follow-ups

- merobox step-type PR (`calimero-network/merobox`): remove `nest_group`/`unnest_group`, add `reparent_group`.
- calimero-client-py PR: mirror the API change.
- mero-drive migration PR: replace the `unnest → delete` workaround with a direct `delete_group` call (cascade handles it).
- CHANGELOG entry in core noting the breaking governance-op change.

---

## Appendix A: rejected alternatives

- **Transactional tree (option B from brainstorm)**: allow multi-op governance transactions with briefly-orphan intermediate states, shielded from handlers. Rejected — we don't have a transaction concept, and adding one to support a state we don't want is backwards.
- **Unlink-then-link move (option C from brainstorm)**: `reparent` implemented as two sequential ops. Rejected — has the same crash-recovery failure mode as today's `unnest`/`nest`.
- **Keep `nest_group` as a convenience**: rejected — in the new model there's no orphan to nest, so the primitive has no valid domain.
- **Optional `parent_id` on `GroupCreated` (null = nest under namespace root)**: rejected — explicitness is better; callers always know where they're putting a group.

## Appendix B: why this isn't fix #4 from the original issue

Issue #2174 suggested "forbid orphan-creation in unnest_group for non-namespace parents" as a possible fix. This spec goes further:

- Removes `unnest_group` entirely, not just its orphan-producing paths.
- Also removes the orphan produced by `GroupCreated` today (pre-`GroupNested`).
- Replaces two primitives with one (`reparent`) that has strictly broader semantics.

The original fix #4 would have kept `unnest` and made it reject some inputs. The current spec makes the broken state unreachable regardless of input.
