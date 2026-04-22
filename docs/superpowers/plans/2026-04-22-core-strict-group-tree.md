# Strict group-tree invariant + cascade delete (core)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make orphan-group state structurally impossible by replacing `nest_group`/`unnest_group` with a single atomic `reparent_group` primitive, requiring `parent_id` on `GroupCreated`, and making `delete_group` cascade over its full subtree (groups + contexts) in one governance op.

**Spec:** `docs/superpowers/specs/2026-04-22-strict-group-tree-and-cascade-delete.md`

**Architecture:** Three governance-op shape changes (`GroupCreated` gains `parent_id`, `GroupReparented` is added, `GroupDeleted` carries cascade payload), `GroupNested` and `GroupUnnested` are removed. Identity-resolution code from closed PR #2175 becomes dead code and is deleted. The invariant — "every group except the namespace root has exactly one parent at all times" — is enforced at the governance-op application layer; no handler can observe a partially-orphaned tree.

**Tech Stack:** Rust 2021, cargo workspace, actix actors, RocksDB store via `calimero_store::Store`, borsh for op serialization, axum for HTTP, eyre for error handling.

**Landing order coordination (across 3 PRs):**

```
1. core           ← THIS PLAN. Lands first. Breaks calimero-client-py master build until step 2.
2. calimero-client-py  ← MUST land within minutes of core. Plan: 2026-04-22-client-py-reparent-bindings.md
3. merobox        ← Lands after step 2. Plan: 2026-04-22-merobox-reparent-step-type.md
```

To minimize the broken-master window, all three PRs should be reviewed and approved in parallel; merge in the order above with no delay between merges.

---

## File structure

### Files modified

- `crates/context/primitives/src/local_governance/mod.rs` — `RootOp` enum reshape (3 variants change, 2 removed, 1 added)
- `crates/context/src/group_store/namespace.rs` — drop `nest_group`/`unnest_group`, add `reparent_group`/`collect_subtree_for_cascade`/`is_descendant_of`
- `crates/context/src/group_store/membership.rs` — add `delete_all_group_members`
- `crates/context/src/group_store/namespace_governance.rs` — update `execute_group_created`/`_deleted`, add `execute_group_reparented`, drop `execute_group_nested`/`_unnested`
- `crates/context/src/group_store/mod.rs` — re-exports
- `crates/context/src/group_store/tests.rs` — new tests for all changes
- `crates/context/src/handlers/delete_group.rs` — build cascade payload, emit single op
- `crates/context/src/handlers/create_group.rs` — accept `parent_id`, emit single op
- `crates/server/src/admin/handlers/namespaces/create_group_in_namespace.rs` — emit single `GroupCreated { parent_id: namespace_id }` op (drop the `GroupNested` second op)
- `crates/server/src/admin/service.rs` — route registrations: drop `nest`/`unnest`, add `reparent`
- `crates/client/src/client/group.rs` — drop `nest_group`/`unnest_group` methods, add `reparent_group`
- `crates/server/primitives/src/admin.rs` (or wherever the API request types live) — drop `NestGroupApiRequest`/`UnnestGroupApiRequest`, add `ReparentGroupApiRequest`
- `crates/meroctl/src/cli/group.rs` — drop `nest`/`unnest` subcommand declarations, add `reparent`

### Files created

- `crates/server/src/admin/handlers/groups/reparent_group.rs` — new HTTP handler
- `crates/meroctl/src/cli/group/reparent.rs` — new CLI subcommand
- `apps/e2e-kv-store/workflows/group-reparent.yml` — replaces `group-nesting.yml` with reparent-only scenarios
- `apps/e2e-kv-store/workflows/group-reparent-and-cascade-delete.yml` — full cascade-delete E2E test

### Files deleted

- `crates/server/src/admin/handlers/groups/nest_group.rs`
- `crates/server/src/admin/handlers/groups/unnest_group.rs`
- `crates/meroctl/src/cli/group/nest.rs`
- `crates/meroctl/src/cli/group/unnest.rs`
- `apps/e2e-kv-store/workflows/group-nesting.yml`

### Dead code from closed PR #2175 — confirm absent on master before starting

The current `master` does NOT contain PR #2175's changes (the PR was closed without merging). Before starting, confirm by grepping:

```bash
grep -r "node_group_admin_identity\|find_namespace_identity_by_public_key" crates/context/src/
```

Expected: no results. If any results appear, those are a residual from PR #2175 that was somehow merged — surface to the user before continuing.

---

## Tasks

### Task 1: Update `RootOp` enum

**Files:**
- Modify: `crates/context/primitives/src/local_governance/mod.rs`

- [ ] **Step 1: Read the current `RootOp` enum**

Run: `sed -n '180,230p' crates/context/primitives/src/local_governance/mod.rs`

Note current variants for reference. The current enum has 7 variants including `GroupCreated`, `GroupDeleted`, `AdminChanged`, `PolicyUpdated`, `GroupNested`, `GroupUnnested`, `MemberJoined`.

- [ ] **Step 2: Modify the enum**

Replace the `GroupCreated`, `GroupDeleted`, `GroupNested`, `GroupUnnested` variants with:

```rust
/// A new group was created AND atomically nested under `parent_id`.
/// `parent_id` MUST reference a group that exists in this namespace
/// (the namespace root itself or a previously-created subgroup).
GroupCreated {
    group_id: [u8; 32],
    parent_id: [u8; 32],
},
/// Atomically move `child_group_id` from its current parent to `new_parent_id`.
/// Both groups MUST exist in the same namespace. Must not create a cycle.
/// `child_group_id` MUST NOT be the namespace root.
GroupReparented {
    child_group_id: [u8; 32],
    new_parent_id: [u8; 32],
},
/// Delete `root_group_id` AND its entire subtree AND all contained contexts
/// in one op. The signer pre-computes `cascade_group_ids` (descendants in
/// children-first order) and `cascade_context_ids`. Every peer re-enumerates
/// locally and rejects the op if the payload disagrees with their state
/// (deterministic application check — catches silent divergence).
GroupDeleted {
    root_group_id: [u8; 32],
    cascade_group_ids: Vec<[u8; 32]>,
    cascade_context_ids: Vec<[u8; 32]>,
},
```

Delete the `GroupNested` and `GroupUnnested` variants entirely.

- [ ] **Step 3: Verify it compiles in isolation**

Run: `cargo check -p calimero-context-client`
Expected: PASS (this crate doesn't use the variants by name).

- [ ] **Step 4: Verify the workspace shows the expected breakage**

Run: `cargo check --workspace 2>&1 | grep -E "no variant|expected.*found.*Group" | head -20`
Expected: errors in `crates/context/src/group_store/namespace_governance.rs`, `crates/context/src/group_store/tests.rs`, and possibly any consumer that pattern-matches on these variants. Use this output as the work-list for subsequent tasks.

- [ ] **Step 5: Commit**

```bash
git add crates/context/primitives/src/local_governance/mod.rs
git commit -m "feat(context-client): reshape RootOp for strict group-tree invariant

GroupCreated now requires parent_id (atomic create+nest).
GroupNested and GroupUnnested removed.
GroupReparented added (atomic edge swap).
GroupDeleted carries cascade payload (descendants + contexts).

Spec: docs/superpowers/specs/2026-04-22-strict-group-tree-and-cascade-delete.md"
```

---

### Task 2: Add `delete_all_group_members` helper

**Files:**
- Modify: `crates/context/src/group_store/membership.rs`
- Modify: `crates/context/src/group_store/mod.rs` (re-export)
- Test: `crates/context/src/group_store/tests.rs`

- [ ] **Step 1: Write the failing test**

Append to `crates/context/src/group_store/tests.rs`:

```rust
#[test]
fn delete_all_group_members_removes_every_member() {
    let store = test_store();
    let gid = ContextGroupId::from([0xC0; 32]);
    let admin = PublicKey::from([0x01; 32]);
    let m1 = PublicKey::from([0x02; 32]);
    let m2 = PublicKey::from([0x03; 32]);

    add_group_member(&store, &gid, &admin, GroupMemberRole::Admin).unwrap();
    add_group_member(&store, &gid, &m1, GroupMemberRole::Member).unwrap();
    add_group_member(&store, &gid, &m2, GroupMemberRole::Member).unwrap();
    assert!(check_group_membership(&store, &gid, &admin).unwrap());
    assert!(check_group_membership(&store, &gid, &m1).unwrap());
    assert!(check_group_membership(&store, &gid, &m2).unwrap());

    delete_all_group_members(&store, &gid).unwrap();

    assert!(!check_group_membership(&store, &gid, &admin).unwrap());
    assert!(!check_group_membership(&store, &gid, &m1).unwrap());
    assert!(!check_group_membership(&store, &gid, &m2).unwrap());
}

#[test]
fn delete_all_group_members_does_not_touch_other_groups() {
    let store = test_store();
    let gid_a = ContextGroupId::from([0xC0; 32]);
    let gid_b = ContextGroupId::from([0xC1; 32]);
    let pk = PublicKey::from([0x01; 32]);

    add_group_member(&store, &gid_a, &pk, GroupMemberRole::Member).unwrap();
    add_group_member(&store, &gid_b, &pk, GroupMemberRole::Member).unwrap();

    delete_all_group_members(&store, &gid_a).unwrap();

    assert!(!check_group_membership(&store, &gid_a, &pk).unwrap());
    assert!(check_group_membership(&store, &gid_b, &pk).unwrap());
}
```

- [ ] **Step 2: Run the failing test**

Run: `cargo test -p calimero-context --lib delete_all_group_members 2>&1 | tail -10`
Expected: COMPILATION ERROR `cannot find function delete_all_group_members in this scope`.

- [ ] **Step 3: Implement the function**

Find `delete_all_group_signing_keys` in `crates/context/src/group_store/signing_keys.rs` for the iteration pattern (uses `collect_keys_with_prefix` filtered by `group_id == gid`). Mirror that pattern in `membership.rs`.

Append to `crates/context/src/group_store/membership.rs`:

```rust
/// Bulk-delete every GroupMember record for `group_id`.
/// Used by cascade-delete; mirrors `delete_all_group_signing_keys`.
pub fn delete_all_group_members(
    store: &Store,
    group_id: &ContextGroupId,
) -> EyreResult<()> {
    let gid = group_id.to_bytes();
    let keys = super::collect_keys_with_prefix(
        store,
        calimero_store::key::GroupMember::new(gid, PublicKey::from([0u8; 32])),
        calimero_store::key::GROUP_MEMBER_PREFIX,
        |k| k.group_id() == gid,
    )?;
    let mut handle = store.handle();
    for key in keys {
        handle.delete(&key)?;
    }
    Ok(())
}
```

If `GROUP_MEMBER_PREFIX` is not the right constant name, grep for it in `crates/store/src/key/group/mod.rs` (search for `GroupMember` neighbors). The grep pattern: `grep -n "GROUP_MEMBER" crates/store/src/key/group/mod.rs`.

- [ ] **Step 4: Re-export from mod.rs**

In `crates/context/src/group_store/mod.rs`, locate `pub use self::membership::{...}` and add `delete_all_group_members` to the list.

- [ ] **Step 5: Run the test**

Run: `cargo test -p calimero-context --lib delete_all_group_members 2>&1 | tail -10`
Expected: PASS, 2 tests.

- [ ] **Step 6: Commit**

```bash
git add crates/context/src/group_store/membership.rs crates/context/src/group_store/mod.rs crates/context/src/group_store/tests.rs
git commit -m "feat(context/group_store): add delete_all_group_members for cascade

Mirrors delete_all_group_signing_keys. Used by execute_group_deleted
in cascade-delete to bulk-clear membership records for every group
in the deleted subtree."
```

---

### Task 3: Add `is_descendant_of` helper

**Files:**
- Modify: `crates/context/src/group_store/namespace.rs`
- Modify: `crates/context/src/group_store/mod.rs`
- Test: `crates/context/src/group_store/tests.rs`

- [ ] **Step 1: Write the failing tests**

Append to `crates/context/src/group_store/tests.rs`:

```rust
#[test]
fn is_descendant_of_direct_child() {
    let store = test_store();
    let parent = ContextGroupId::from([0xD0; 32]);
    let child = ContextGroupId::from([0xD1; 32]);
    nest_under(&store, &parent, &child);

    assert!(is_descendant_of(&store, &child, &parent).unwrap());
    assert!(!is_descendant_of(&store, &parent, &child).unwrap());
}

#[test]
fn is_descendant_of_grandchild() {
    let store = test_store();
    let root = ContextGroupId::from([0xD0; 32]);
    let mid = ContextGroupId::from([0xD1; 32]);
    let leaf = ContextGroupId::from([0xD2; 32]);
    nest_under(&store, &root, &mid);
    nest_under(&store, &mid, &leaf);

    assert!(is_descendant_of(&store, &leaf, &root).unwrap());
    assert!(is_descendant_of(&store, &leaf, &mid).unwrap());
    assert!(!is_descendant_of(&store, &root, &leaf).unwrap());
}

#[test]
fn is_descendant_of_unrelated() {
    let store = test_store();
    let a = ContextGroupId::from([0xD0; 32]);
    let b = ContextGroupId::from([0xD1; 32]);
    // No edge between them.
    assert!(!is_descendant_of(&store, &a, &b).unwrap());
    assert!(!is_descendant_of(&store, &b, &a).unwrap());
}

#[test]
fn is_descendant_of_self_is_false() {
    let store = test_store();
    let a = ContextGroupId::from([0xD0; 32]);
    assert!(!is_descendant_of(&store, &a, &a).unwrap());
}

// Helper for tests in this section: write a parent edge directly,
// bypassing any nest_group fn (which won't exist after Task 5).
fn nest_under(store: &Store, parent: &ContextGroupId, child: &ContextGroupId) {
    use calimero_store::key::{GroupChildIndex, GroupParentRef, GroupParentRefValue};
    let mut handle = store.handle();
    handle
        .put(
            &GroupParentRef::new(child.to_bytes()),
            &GroupParentRefValue { parent_group_id: parent.to_bytes() },
        )
        .unwrap();
    handle
        .put(&GroupChildIndex::new(parent.to_bytes(), child.to_bytes()), &())
        .unwrap();
}
```

If the `GroupParentRefValue` struct field name differs from `parent_group_id`, check `crates/store/src/key/group/mod.rs` and adjust. Same for `GroupChildIndex` value type.

- [ ] **Step 2: Run the failing tests**

Run: `cargo test -p calimero-context --lib is_descendant_of 2>&1 | tail -10`
Expected: COMPILATION ERROR.

- [ ] **Step 3: Implement `is_descendant_of`**

Add to `crates/context/src/group_store/namespace.rs`, near `resolve_namespace`:

```rust
/// Returns true iff `candidate` is a descendant of `potential_ancestor`
/// (transitively, via parent chain). Returns false for `candidate == potential_ancestor`.
/// Bounded walk; returns Err if MAX_NAMESPACE_DEPTH is exceeded (cycle detection).
pub fn is_descendant_of(
    store: &Store,
    candidate: &ContextGroupId,
    potential_ancestor: &ContextGroupId,
) -> EyreResult<bool> {
    if candidate == potential_ancestor {
        return Ok(false);
    }
    let mut current = *candidate;
    for _ in 0..MAX_NAMESPACE_DEPTH {
        match get_parent_group(store, &current)? {
            Some(parent) => {
                if parent == *potential_ancestor {
                    return Ok(true);
                }
                current = parent;
            }
            None => return Ok(false),
        }
    }
    eyre::bail!(
        "is_descendant_of exceeded MAX_NAMESPACE_DEPTH ({MAX_NAMESPACE_DEPTH}); possible cycle"
    )
}
```

- [ ] **Step 4: Re-export**

In `crates/context/src/group_store/mod.rs`, add `is_descendant_of` to the `pub use self::namespace::{...}` block.

- [ ] **Step 5: Run the tests**

Run: `cargo test -p calimero-context --lib is_descendant_of 2>&1 | tail -10`
Expected: PASS, 4 tests.

- [ ] **Step 6: Commit**

```bash
git add crates/context/src/group_store/namespace.rs crates/context/src/group_store/mod.rs crates/context/src/group_store/tests.rs
git commit -m "feat(context/group_store): add is_descendant_of for cycle detection

Used by reparent_group to reject moves that would create a cycle
(reparenting a node under one of its own descendants)."
```

---

### Task 4: Add `reparent_group` store function

**Files:**
- Modify: `crates/context/src/group_store/namespace.rs`
- Modify: `crates/context/src/group_store/mod.rs`
- Test: `crates/context/src/group_store/tests.rs`

- [ ] **Step 1: Write the failing tests**

Append to `crates/context/src/group_store/tests.rs`:

```rust
#[test]
fn reparent_group_swaps_parent_edge() {
    let store = test_store();
    let old_parent = ContextGroupId::from([0xE0; 32]);
    let new_parent = ContextGroupId::from([0xE1; 32]);
    let child = ContextGroupId::from([0xE2; 32]);
    save_group_meta(&store, &old_parent, &test_meta()).unwrap();
    save_group_meta(&store, &new_parent, &test_meta()).unwrap();
    save_group_meta(&store, &child, &test_meta()).unwrap();
    nest_under(&store, &old_parent, &child);

    reparent_group(&store, &child, &new_parent).unwrap();

    assert_eq!(get_parent_group(&store, &child).unwrap(), Some(new_parent));
    let old_children = list_child_groups(&store, &old_parent).unwrap();
    assert!(!old_children.contains(&child));
    let new_children = list_child_groups(&store, &new_parent).unwrap();
    assert!(new_children.contains(&child));
}

#[test]
fn reparent_group_idempotent_on_same_parent() {
    let store = test_store();
    let parent = ContextGroupId::from([0xE0; 32]);
    let child = ContextGroupId::from([0xE2; 32]);
    save_group_meta(&store, &parent, &test_meta()).unwrap();
    save_group_meta(&store, &child, &test_meta()).unwrap();
    nest_under(&store, &parent, &child);

    // Should not fail, should not double-write child index.
    reparent_group(&store, &child, &parent).unwrap();
    assert_eq!(get_parent_group(&store, &child).unwrap(), Some(parent));
    assert_eq!(list_child_groups(&store, &parent).unwrap().len(), 1);
}

#[test]
fn reparent_group_rejects_cycle() {
    let store = test_store();
    let a = ContextGroupId::from([0xE0; 32]);
    let b = ContextGroupId::from([0xE1; 32]);
    save_group_meta(&store, &a, &test_meta()).unwrap();
    save_group_meta(&store, &b, &test_meta()).unwrap();
    // a is the root; b nested under a.
    nest_under(&store, &a, &b);

    // Trying to reparent a under b would create a cycle.
    let err = reparent_group(&store, &a, &b).unwrap_err();
    assert!(format!("{err}").contains("cycle"), "expected cycle error, got: {err}");
}

#[test]
fn reparent_group_rejects_root() {
    let store = test_store();
    let root = ContextGroupId::from([0xE0; 32]);
    let other = ContextGroupId::from([0xE1; 32]);
    save_group_meta(&store, &root, &test_meta()).unwrap();
    save_group_meta(&store, &other, &test_meta()).unwrap();
    // root has no parent edge.

    let err = reparent_group(&store, &root, &other).unwrap_err();
    assert!(
        format!("{err}").contains("namespace root") || format!("{err}").contains("no parent"),
        "expected root rejection, got: {err}"
    );
}

#[test]
fn reparent_group_rejects_nonexistent_new_parent() {
    let store = test_store();
    let parent = ContextGroupId::from([0xE0; 32]);
    let child = ContextGroupId::from([0xE2; 32]);
    let phantom = ContextGroupId::from([0xFF; 32]);
    save_group_meta(&store, &parent, &test_meta()).unwrap();
    save_group_meta(&store, &child, &test_meta()).unwrap();
    nest_under(&store, &parent, &child);

    let err = reparent_group(&store, &child, &phantom).unwrap_err();
    assert!(
        format!("{err}").contains("not found") || format!("{err}").contains("does not exist"),
        "expected new-parent-not-found, got: {err}"
    );
}
```

- [ ] **Step 2: Run failing tests**

Run: `cargo test -p calimero-context --lib reparent_group 2>&1 | tail -15`
Expected: COMPILATION ERROR `cannot find function reparent_group`.

- [ ] **Step 3: Implement `reparent_group`**

Add to `crates/context/src/group_store/namespace.rs`:

```rust
/// Atomically swap the parent of `child` to `new_parent`. Enforces:
/// - `child` must currently have a parent (cannot reparent the namespace root).
/// - `new_parent` must exist in the store (have a `GroupMeta` entry).
/// - The swap must not create a cycle (`new_parent` must not be a descendant of `child`).
/// Idempotent on `new_parent == old_parent`.
pub fn reparent_group(
    store: &Store,
    child: &ContextGroupId,
    new_parent: &ContextGroupId,
) -> EyreResult<()> {
    use calimero_store::key::{GroupChildIndex, GroupParentRef, GroupParentRefValue};

    let old_parent = get_parent_group(store, child)?
        .ok_or_else(|| eyre::eyre!("cannot reparent the namespace root: '{child:?}' has no parent"))?;

    if old_parent == *new_parent {
        return Ok(());
    }

    if super::load_group_meta(store, new_parent)?.is_none() {
        eyre::bail!("new parent group '{new_parent:?}' not found in this namespace");
    }

    if is_descendant_of(store, new_parent, child)? {
        eyre::bail!(
            "cycle: new_parent '{new_parent:?}' is a descendant of child '{child:?}'"
        );
    }

    let mut handle = store.handle();
    handle.delete(&GroupChildIndex::new(old_parent.to_bytes(), child.to_bytes()))?;
    handle.put(
        &GroupParentRef::new(child.to_bytes()),
        &GroupParentRefValue { parent_group_id: new_parent.to_bytes() },
    )?;
    handle.put(
        &GroupChildIndex::new(new_parent.to_bytes(), child.to_bytes()),
        &(),
    )?;
    Ok(())
}
```

If field/value names differ, check `crates/store/src/key/group/mod.rs` for the actual types.

- [ ] **Step 4: Re-export**

Add `reparent_group` to the `pub use self::namespace::{...}` block in `mod.rs`.

- [ ] **Step 5: Run the tests**

Run: `cargo test -p calimero-context --lib reparent_group 2>&1 | tail -15`
Expected: PASS, 5 tests.

- [ ] **Step 6: Commit**

```bash
git add crates/context/src/group_store/namespace.rs crates/context/src/group_store/mod.rs crates/context/src/group_store/tests.rs
git commit -m "feat(context/group_store): add reparent_group atomic edge swap

Replaces nest_group + unnest_group with a single primitive that
atomically deletes the old parent edge and writes the new one.
Rejects: namespace root, cycles, nonexistent new parent.
Idempotent on same-parent calls."
```

---

### Task 5: Remove `nest_group` and `unnest_group` store functions

**Files:**
- Modify: `crates/context/src/group_store/namespace.rs`
- Modify: `crates/context/src/group_store/mod.rs`

- [ ] **Step 1: Find current callers**

Run: `grep -rn "nest_group\|unnest_group" crates/context/src/ | grep -v test`
Note all callers — they will all need updating in subsequent tasks. Expect callers in `namespace_governance.rs`, possibly `lib.rs`, possibly handler files.

- [ ] **Step 2: Delete the functions**

In `crates/context/src/group_store/namespace.rs`, locate and delete:
- `pub fn nest_group(...)` and its body
- `pub fn unnest_group(...)` and its body

Leave `is_descendant_of` (Task 3), `reparent_group` (Task 4), and all other functions intact.

- [ ] **Step 3: Update mod.rs re-exports**

In `crates/context/src/group_store/mod.rs`, remove `nest_group` and `unnest_group` from the `pub use self::namespace::{...}` block.

- [ ] **Step 4: Verify expected breakage**

Run: `cargo check -p calimero-context 2>&1 | grep -E "cannot find function.*(nest|unnest)_group" | head -10`
Expected: errors at every previous caller. These will be fixed in Task 6 and beyond.

- [ ] **Step 5: Commit (intentionally broken; subsequent tasks fix)**

```bash
git add crates/context/src/group_store/namespace.rs crates/context/src/group_store/mod.rs
git commit -m "refactor(context/group_store): drop nest_group and unnest_group

Both are subsumed by reparent_group (atomic edge swap).
Callers will be updated in subsequent commits; build is intentionally
broken between this commit and the governance-execution update."
```

---

### Task 6: Add `collect_subtree_for_cascade`

**Files:**
- Modify: `crates/context/src/group_store/namespace.rs`
- Modify: `crates/context/src/group_store/mod.rs`
- Test: `crates/context/src/group_store/tests.rs`

- [ ] **Step 1: Write the failing tests**

Append to `crates/context/src/group_store/tests.rs`:

```rust
#[test]
fn collect_subtree_for_cascade_empty_subtree() {
    let store = test_store();
    let root = ContextGroupId::from([0xF0; 32]);
    save_group_meta(&store, &root, &test_meta()).unwrap();

    let payload = collect_subtree_for_cascade(&store, &root).unwrap();
    assert!(payload.descendant_groups.is_empty());
    assert!(payload.contexts.is_empty());
}

#[test]
fn collect_subtree_for_cascade_two_level_tree() {
    let store = test_store();
    let root = ContextGroupId::from([0xF0; 32]);
    let mid = ContextGroupId::from([0xF1; 32]);
    let leaf = ContextGroupId::from([0xF2; 32]);
    save_group_meta(&store, &root, &test_meta()).unwrap();
    save_group_meta(&store, &mid, &test_meta()).unwrap();
    save_group_meta(&store, &leaf, &test_meta()).unwrap();
    nest_under(&store, &root, &mid);
    nest_under(&store, &mid, &leaf);

    let payload = collect_subtree_for_cascade(&store, &root).unwrap();
    // Children-first traversal: leaf must come before mid.
    assert_eq!(payload.descendant_groups.len(), 2);
    let leaf_pos = payload.descendant_groups.iter().position(|g| g == &leaf).unwrap();
    let mid_pos = payload.descendant_groups.iter().position(|g| g == &mid).unwrap();
    assert!(leaf_pos < mid_pos, "expected children-first; leaf={leaf_pos} mid={mid_pos}");
}

#[test]
fn collect_subtree_for_cascade_includes_contexts_from_all_groups() {
    let store = test_store();
    let root = ContextGroupId::from([0xF0; 32]);
    let child = ContextGroupId::from([0xF1; 32]);
    save_group_meta(&store, &root, &test_meta()).unwrap();
    save_group_meta(&store, &child, &test_meta()).unwrap();
    nest_under(&store, &root, &child);

    let ctx_root = ContextId::from([0x10; 32]);
    let ctx_child = ContextId::from([0x11; 32]);
    register_context_to_group(&store, &root, &ctx_root).unwrap();
    register_context_to_group(&store, &child, &ctx_child).unwrap();

    let payload = collect_subtree_for_cascade(&store, &root).unwrap();
    assert!(payload.contexts.contains(&ctx_root));
    assert!(payload.contexts.contains(&ctx_child));
    assert_eq!(payload.contexts.len(), 2);
}
```

If `register_context_to_group` is not the existing fn name, grep `crates/context/src/group_store/` for the function that adds a context to a group and use that.

- [ ] **Step 2: Run failing tests**

Run: `cargo test -p calimero-context --lib collect_subtree_for_cascade 2>&1 | tail -15`
Expected: COMPILATION ERROR.

- [ ] **Step 3: Implement `collect_subtree_for_cascade`**

Add to `crates/context/src/group_store/namespace.rs`:

```rust
/// Result of subtree enumeration. `descendant_groups` does NOT include the
/// root itself. Order is children-first (deepest descendants come first),
/// which matches the order required by `execute_group_deleted` for safe
/// child-index cleanup.
pub struct CascadePayload {
    pub descendant_groups: Vec<ContextGroupId>,
    pub contexts: Vec<ContextId>,
}

/// Walk the subtree rooted at `root` and return:
/// - every descendant group_id (children-first traversal)
/// - every context_id registered on `root` or any descendant
pub fn collect_subtree_for_cascade(
    store: &Store,
    root: &ContextGroupId,
) -> EyreResult<CascadePayload> {
    let mut descendants = Vec::new();
    let mut contexts = Vec::new();

    // Collect contexts of the root itself.
    contexts.extend(super::list_group_contexts(store, root)?);

    // BFS to enumerate descendants, then reverse to get children-first.
    let mut frontier = vec![*root];
    let mut bfs_order = Vec::new();
    while let Some(g) = frontier.pop() {
        let children = list_child_groups(store, &g)?;
        for child in children {
            bfs_order.push(child);
            frontier.push(child);
            contexts.extend(super::list_group_contexts(store, &child)?);
        }
    }
    // Reverse so deepest descendants come first.
    descendants = bfs_order.into_iter().rev().collect();
    Ok(CascadePayload { descendant_groups: descendants, contexts })
}
```

If `list_group_contexts` is not the right fn name (it might be `enumerate_group_contexts` based on what I saw earlier), grep and adjust.

- [ ] **Step 4: Re-export**

Add `collect_subtree_for_cascade` and `CascadePayload` to `mod.rs` re-exports.

- [ ] **Step 5: Run the tests**

Run: `cargo test -p calimero-context --lib collect_subtree_for_cascade 2>&1 | tail -10`
Expected: PASS, 3 tests.

- [ ] **Step 6: Commit**

```bash
git add crates/context/src/group_store/namespace.rs crates/context/src/group_store/mod.rs crates/context/src/group_store/tests.rs
git commit -m "feat(context/group_store): add collect_subtree_for_cascade

Returns the children-first list of descendant group_ids plus all
context_ids registered on root or any descendant. Used by
delete_group handler to build the GroupDeleted op payload, and
by execute_group_deleted to verify deterministic application."
```

---

### Task 7: Update `execute_group_created` for required parent_id

**Files:**
- Modify: `crates/context/src/group_store/namespace_governance.rs`
- Test: `crates/context/src/group_store/tests.rs`

- [ ] **Step 1: Write the failing tests**

Append to `crates/context/src/group_store/tests.rs`:

```rust
#[test]
fn execute_group_created_writes_parent_edge() {
    let (store, ns_id, admin_pk, admin_sk) = setup_namespace_governance();
    let new_group_id = [0xAB; 32];

    let op = SignedNamespaceOp::sign(
        &admin_sk,
        ns_id,
        next_op_seq(&store, &ns_id),
        prev_op_hash(&store, &ns_id),
        NamespaceOp::Root(RootOp::GroupCreated {
            group_id: new_group_id,
            parent_id: ns_id,
        }),
    ).unwrap();

    let result = apply_signed_namespace_op(&store, ns_id, &op);
    assert!(result.is_ok(), "apply failed: {result:?}");

    let new_gid = ContextGroupId::from(new_group_id);
    assert_eq!(get_parent_group(&store, &new_gid).unwrap(), Some(ContextGroupId::from(ns_id)));
}

#[test]
fn execute_group_created_rejects_unknown_parent() {
    let (store, ns_id, _admin_pk, admin_sk) = setup_namespace_governance();
    let new_group_id = [0xAB; 32];
    let phantom_parent = [0xCD; 32];

    let op = SignedNamespaceOp::sign(
        &admin_sk,
        ns_id,
        next_op_seq(&store, &ns_id),
        prev_op_hash(&store, &ns_id),
        NamespaceOp::Root(RootOp::GroupCreated {
            group_id: new_group_id,
            parent_id: phantom_parent,
        }),
    ).unwrap();

    let result = apply_signed_namespace_op(&store, ns_id, &op);
    assert!(result.is_err(), "expected rejection, got: {result:?}");
    // No state should have been written.
    let new_gid = ContextGroupId::from(new_group_id);
    assert!(load_group_meta(&store, &new_gid).unwrap().is_none());
}
```

`setup_namespace_governance`, `next_op_seq`, `prev_op_hash` — check existing test helpers in `tests.rs`. There are likely existing patterns from the current `GroupNested` tests around line 2722. If helpers don't exist, define them inline based on those existing tests.

- [ ] **Step 2: Run failing tests**

Run: `cargo test -p calimero-context --lib execute_group_created 2>&1 | tail -15`
Expected: COMPILATION ERROR (because `RootOp::GroupCreated` no longer accepts the old single-field form, the EXISTING tests that use `RootOp::GroupCreated { group_id }` should also be failing).

Note: this means we need to fix BOTH the new tests AND the existing tests at lines 2801, 2841, 2857 that use `GroupCreated { group_id }`. Add `parent_id: ns_id` to each.

- [ ] **Step 3: Update existing test usages**

Find all existing usages: `grep -n "RootOp::GroupCreated" crates/context/src/group_store/tests.rs`

For each, add `parent_id: <appropriate_id>` field. The appropriate id is whatever namespace the test is operating in.

- [ ] **Step 4: Update `execute_group_created` signature and body**

In `crates/context/src/group_store/namespace_governance.rs`, find `fn execute_group_created`. Update signature to accept `parent_id: [u8; 32]` and update body:

```rust
fn execute_group_created(
    &self,
    op: &SignedNamespaceOp,
    group_id: [u8; 32],
    parent_id: [u8; 32],
) -> EyreResult<()> {
    self.require_namespace_admin(&op.signer)?;
    let gid = ContextGroupId::from(group_id);
    if load_group_meta(self.store, &gid)?.is_some() {
        tracing::debug!(
            group_id = %hex::encode(group_id),
            "group already exists, ignoring GroupCreated"
        );
        return Ok(());
    }

    // Verify parent exists in this namespace.
    let parent_gid = ContextGroupId::from(parent_id);
    if load_group_meta(self.store, &parent_gid)?.is_none() {
        eyre::bail!(
            "GroupCreated rejected: parent_id '{parent_gid:?}' not found in namespace"
        );
    }

    // Inherit application ID from the parent (which is reachable now;
    // before this change, the namespace root was hard-coded — but parent_id
    // generalizes to subgroup nesting at create time).
    let parent_app_id = load_group_meta(self.store, &parent_gid)?
        .map(|m| m.target_application_id)
        .unwrap_or_else(|| calimero_primitives::application::ApplicationId::from([0u8; 32]));

    let meta = calimero_store::key::GroupMetaValue {
        admin_identity: op.signer,
        target_application_id: parent_app_id,
        app_key: [0u8; 32],
        upgrade_policy: calimero_primitives::context::UpgradePolicy::default(),
        migration: None,
        created_at: 0,
        auto_join: false,
    };

    // Atomic batch: meta + parent edge + child index + admin membership.
    save_group_meta(self.store, &gid, &meta)?;
    {
        use calimero_store::key::{GroupChildIndex, GroupParentRef, GroupParentRefValue};
        let mut handle = self.store.handle();
        handle.put(
            &GroupParentRef::new(group_id),
            &GroupParentRefValue { parent_group_id: parent_id },
        )?;
        handle.put(&GroupChildIndex::new(parent_id, group_id), &())?;
    }
    add_group_member(self.store, &gid, &op.signer, GroupMemberRole::Admin)?;
    Ok(())
}
```

- [ ] **Step 5: Update the dispatch in the apply loop**

Find the match arm in `apply_signed_namespace_op` (or wherever ops are dispatched) that handles `RootOp::GroupCreated`. Update it to pass the new `parent_id` field:

```rust
RootOp::GroupCreated { group_id, parent_id } => {
    self.execute_group_created(op, *group_id, *parent_id)?;
}
```

- [ ] **Step 6: Run the tests**

Run: `cargo test -p calimero-context --lib execute_group_created 2>&1 | tail -10`
Expected: PASS, 2 new tests + existing tests still passing.

- [ ] **Step 7: Commit**

```bash
git add crates/context/src/group_store/namespace_governance.rs crates/context/src/group_store/tests.rs
git commit -m "feat(context/group_store): GroupCreated requires parent_id

execute_group_created now atomically writes group meta, parent edge,
child index, and admin membership in one transaction. Rejects ops
whose parent_id doesn't exist in the namespace."
```

---

### Task 8: Add `execute_group_reparented` and remove `_nested`/`_unnested`

**Files:**
- Modify: `crates/context/src/group_store/namespace_governance.rs`
- Test: `crates/context/src/group_store/tests.rs`

- [ ] **Step 1: Write the failing tests**

Append to `crates/context/src/group_store/tests.rs`:

```rust
#[test]
fn execute_group_reparented_swaps_edge() {
    let (store, ns_id, admin_pk, admin_sk) = setup_namespace_governance();
    let mid_id = [0xAA; 32];
    let new_parent_id = [0xBB; 32];
    let leaf_id = [0xCC; 32];
    // Create three subgroups: mid under namespace, new_parent under namespace,
    // leaf under mid. Then reparent leaf from mid to new_parent.
    apply_create(&store, ns_id, &admin_sk, mid_id, ns_id);
    apply_create(&store, ns_id, &admin_sk, new_parent_id, ns_id);
    apply_create(&store, ns_id, &admin_sk, leaf_id, mid_id);

    let op = SignedNamespaceOp::sign(
        &admin_sk,
        ns_id,
        next_op_seq(&store, &ns_id),
        prev_op_hash(&store, &ns_id),
        NamespaceOp::Root(RootOp::GroupReparented {
            child_group_id: leaf_id,
            new_parent_id,
        }),
    ).unwrap();

    let result = apply_signed_namespace_op(&store, ns_id, &op);
    assert!(result.is_ok(), "apply failed: {result:?}");

    let leaf_gid = ContextGroupId::from(leaf_id);
    assert_eq!(get_parent_group(&store, &leaf_gid).unwrap(), Some(ContextGroupId::from(new_parent_id)));
    let old_children = list_child_groups(&store, &ContextGroupId::from(mid_id)).unwrap();
    assert!(!old_children.contains(&leaf_gid));
    let new_children = list_child_groups(&store, &ContextGroupId::from(new_parent_id)).unwrap();
    assert!(new_children.contains(&leaf_gid));
}

#[test]
fn execute_group_reparented_rejects_cycle() {
    let (store, ns_id, _admin_pk, admin_sk) = setup_namespace_governance();
    let parent_id = [0xAA; 32];
    let child_id = [0xBB; 32];
    apply_create(&store, ns_id, &admin_sk, parent_id, ns_id);
    apply_create(&store, ns_id, &admin_sk, child_id, parent_id);

    // Try to reparent parent under child — cycle.
    let op = SignedNamespaceOp::sign(
        &admin_sk,
        ns_id,
        next_op_seq(&store, &ns_id),
        prev_op_hash(&store, &ns_id),
        NamespaceOp::Root(RootOp::GroupReparented {
            child_group_id: parent_id,
            new_parent_id: child_id,
        }),
    ).unwrap();

    let result = apply_signed_namespace_op(&store, ns_id, &op);
    assert!(result.is_err());
    assert!(format!("{result:?}").contains("cycle"));
}

// Helper to apply a single create op (used by the reparent tests).
fn apply_create(
    store: &Store,
    ns_id: [u8; 32],
    admin_sk: &PrivateKey,
    new_id: [u8; 32],
    parent_id: [u8; 32],
) {
    let op = SignedNamespaceOp::sign(
        admin_sk,
        ns_id,
        next_op_seq(store, &ns_id),
        prev_op_hash(store, &ns_id),
        NamespaceOp::Root(RootOp::GroupCreated { group_id: new_id, parent_id }),
    ).unwrap();
    apply_signed_namespace_op(store, ns_id, &op).unwrap();
}
```

- [ ] **Step 2: Run failing tests**

Run: `cargo test -p calimero-context --lib execute_group_reparented 2>&1 | tail -10`
Expected: COMPILATION ERROR — `RootOp::GroupReparented` exists in primitives (Task 1) but isn't dispatched yet, OR `execute_group_reparented` not defined.

- [ ] **Step 3: Add `execute_group_reparented`**

In `crates/context/src/group_store/namespace_governance.rs`:

```rust
fn execute_group_reparented(
    &self,
    op: &SignedNamespaceOp,
    child_group_id: [u8; 32],
    new_parent_id: [u8; 32],
) -> EyreResult<()> {
    self.require_namespace_admin(&op.signer)?;
    let child = ContextGroupId::from(child_group_id);
    let new_parent = ContextGroupId::from(new_parent_id);
    super::reparent_group(self.store, &child, &new_parent)
}
```

The store-level `reparent_group` (from Task 4) already validates everything — root rejection, cycle, nonexistent new_parent. The execute fn is a thin wrapper that adds the namespace-admin auth check.

- [ ] **Step 4: Update the dispatch**

In `apply_signed_namespace_op`, add a match arm for `RootOp::GroupReparented`:

```rust
RootOp::GroupReparented { child_group_id, new_parent_id } => {
    self.execute_group_reparented(op, *child_group_id, *new_parent_id)?;
}
```

Remove the existing arms for `RootOp::GroupNested` and `RootOp::GroupUnnested`. Also delete the `execute_group_nested` and `execute_group_unnested` fn definitions.

- [ ] **Step 5: Run the tests**

Run: `cargo test -p calimero-context --lib execute_group_reparented 2>&1 | tail -10`
Expected: PASS, 2 tests.

- [ ] **Step 6: Find and update old test usages of GroupNested/GroupUnnested**

Run: `grep -n "GroupNested\|GroupUnnested" crates/context/src/group_store/tests.rs`

For tests that exercise the old nest/unnest flow (around line 2722, 2748), either:
- Rewrite them to use `GroupReparented`, OR
- If they were specifically about nest/unnest semantics that no longer exist, delete them with a brief commit-message rationale.

The test `unnest_group_orphans_child_does_not_cascade` (or similar) is no longer relevant — orphaning is forbidden. Delete it.

- [ ] **Step 7: Verify all governance-execution tests pass**

Run: `cargo test -p calimero-context --lib namespace_governance 2>&1 | tail -10`
Expected: all pass.

- [ ] **Step 8: Commit**

```bash
git add crates/context/src/group_store/namespace_governance.rs crates/context/src/group_store/tests.rs
git commit -m "feat(context/group_store): add execute_group_reparented, drop nest/unnest

Adds atomic edge-swap governance op handler. Removes
execute_group_nested / execute_group_unnested and their dispatch
arms. Old tests that exercised orphan-producing flows are deleted
(orphan state is now forbidden by construction)."
```

---

### Task 9: Update `execute_group_deleted` for cascade

**Files:**
- Modify: `crates/context/src/group_store/namespace_governance.rs`
- Test: `crates/context/src/group_store/tests.rs`

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn execute_group_deleted_cascades_subtree_and_contexts() {
    let (store, ns_id, _admin_pk, admin_sk) = setup_namespace_governance();
    let mid_id = [0xAA; 32];
    let leaf_id = [0xBB; 32];
    apply_create(&store, ns_id, &admin_sk, mid_id, ns_id);
    apply_create(&store, ns_id, &admin_sk, leaf_id, mid_id);
    let ctx_mid = ContextId::from([0x11; 32]);
    let ctx_leaf = ContextId::from([0x22; 32]);
    register_context_to_group(&store, &ContextGroupId::from(mid_id), &ctx_mid).unwrap();
    register_context_to_group(&store, &ContextGroupId::from(leaf_id), &ctx_leaf).unwrap();

    let payload = collect_subtree_for_cascade(&store, &ContextGroupId::from(mid_id)).unwrap();
    let cascade_groups: Vec<[u8; 32]> = payload.descendant_groups.iter().map(|g| g.to_bytes()).collect();
    let cascade_contexts: Vec<[u8; 32]> = payload.contexts.iter().map(|c| **c).collect();

    let op = SignedNamespaceOp::sign(
        &admin_sk,
        ns_id,
        next_op_seq(&store, &ns_id),
        prev_op_hash(&store, &ns_id),
        NamespaceOp::Root(RootOp::GroupDeleted {
            root_group_id: mid_id,
            cascade_group_ids: cascade_groups,
            cascade_context_ids: cascade_contexts,
        }),
    ).unwrap();

    let result = apply_signed_namespace_op(&store, ns_id, &op);
    assert!(result.is_ok(), "apply failed: {result:?}");

    // mid + leaf + their contexts gone; ns_id intact.
    assert!(load_group_meta(&store, &ContextGroupId::from(mid_id)).unwrap().is_none());
    assert!(load_group_meta(&store, &ContextGroupId::from(leaf_id)).unwrap().is_none());
    assert!(load_group_meta(&store, &ContextGroupId::from(ns_id)).unwrap().is_some());
    assert!(get_parent_group(&store, &ContextGroupId::from(mid_id)).unwrap().is_none());
    assert!(get_parent_group(&store, &ContextGroupId::from(leaf_id)).unwrap().is_none());
}

#[test]
fn execute_group_deleted_rejects_payload_mismatch() {
    let (store, ns_id, _admin_pk, admin_sk) = setup_namespace_governance();
    let target_id = [0xAA; 32];
    let leaf_id = [0xBB; 32];
    apply_create(&store, ns_id, &admin_sk, target_id, ns_id);
    apply_create(&store, ns_id, &admin_sk, leaf_id, target_id);

    // Build a payload that LIES — claims no descendants.
    let op = SignedNamespaceOp::sign(
        &admin_sk,
        ns_id,
        next_op_seq(&store, &ns_id),
        prev_op_hash(&store, &ns_id),
        NamespaceOp::Root(RootOp::GroupDeleted {
            root_group_id: target_id,
            cascade_group_ids: vec![],  // wrong: leaf is a descendant
            cascade_context_ids: vec![],
        }),
    ).unwrap();

    let result = apply_signed_namespace_op(&store, ns_id, &op);
    assert!(result.is_err());
    assert!(format!("{result:?}").contains("mismatch") || format!("{result:?}").contains("payload"));
    // No state should have been mutated.
    assert!(load_group_meta(&store, &ContextGroupId::from(target_id)).unwrap().is_some());
    assert!(load_group_meta(&store, &ContextGroupId::from(leaf_id)).unwrap().is_some());
}

#[test]
fn execute_group_deleted_rejects_namespace_root() {
    let (store, ns_id, _admin_pk, admin_sk) = setup_namespace_governance();

    let op = SignedNamespaceOp::sign(
        &admin_sk,
        ns_id,
        next_op_seq(&store, &ns_id),
        prev_op_hash(&store, &ns_id),
        NamespaceOp::Root(RootOp::GroupDeleted {
            root_group_id: ns_id,
            cascade_group_ids: vec![],
            cascade_context_ids: vec![],
        }),
    ).unwrap();

    let result = apply_signed_namespace_op(&store, ns_id, &op);
    assert!(result.is_err());
    assert!(format!("{result:?}").contains("namespace root") || format!("{result:?}").contains("delete_namespace"));
}
```

- [ ] **Step 2: Run failing tests**

Run: `cargo test -p calimero-context --lib execute_group_deleted 2>&1 | tail -10`
Expected: failures, possibly compile errors.

- [ ] **Step 3: Update `execute_group_deleted`**

In `crates/context/src/group_store/namespace_governance.rs`, replace the existing fn:

```rust
fn execute_group_deleted(
    &self,
    op: &SignedNamespaceOp,
    root_group_id: [u8; 32],
    cascade_group_ids: &[[u8; 32]],
    cascade_context_ids: &[[u8; 32]],
) -> EyreResult<()> {
    self.require_namespace_admin(&op.signer)?;

    let root_gid = ContextGroupId::from(root_group_id);
    if root_group_id == self.namespace_id {
        eyre::bail!(
            "cannot delete the namespace root '{root_gid:?}' (use delete_namespace instead)"
        );
    }

    // Determinism check: re-enumerate locally and compare.
    let local_payload = super::collect_subtree_for_cascade(self.store, &root_gid)?;
    let local_groups: Vec<[u8; 32]> =
        local_payload.descendant_groups.iter().map(|g| g.to_bytes()).collect();
    let local_contexts: std::collections::BTreeSet<[u8; 32]> =
        local_payload.contexts.iter().map(|c| **c).collect();
    let payload_contexts: std::collections::BTreeSet<[u8; 32]> =
        cascade_context_ids.iter().copied().collect();
    if local_groups != cascade_group_ids {
        eyre::bail!(
            "GroupDeleted cascade payload mismatch (groups): local={local_groups:?} payload={cascade_group_ids:?}"
        );
    }
    if local_contexts != payload_contexts {
        eyre::bail!(
            "GroupDeleted cascade payload mismatch (contexts): local={local_contexts:?} payload={payload_contexts:?}"
        );
    }

    // Children-first: delete descendants before root.
    for gid_bytes in cascade_group_ids.iter().chain(std::iter::once(&root_group_id)) {
        let gid = ContextGroupId::from(*gid_bytes);
        super::delete_all_group_signing_keys(self.store, &gid)?;
        super::delete_all_group_members(self.store, &gid)?;
        super::delete_group_meta(self.store, &gid)?;
        // Delete parent edge and child-index entry.
        if let Some(parent) = super::get_parent_group(self.store, &gid)? {
            let mut handle = self.store.handle();
            handle.delete(&calimero_store::key::GroupParentRef::new(*gid_bytes))?;
            handle.delete(&calimero_store::key::GroupChildIndex::new(parent.to_bytes(), *gid_bytes))?;
        }
    }

    // Clean up contexts.
    for ctx_bytes in cascade_context_ids {
        let ctx = ContextId::from(*ctx_bytes);
        super::unregister_context_from_group(self.store, &ctx)?;
    }

    tracing::info!(
        ?root_gid,
        deleted_groups = cascade_group_ids.len() + 1,
        deleted_contexts = cascade_context_ids.len(),
        "cascade-deleted group subtree"
    );
    Ok(())
}
```

If `unregister_context_from_group` doesn't exist by that name, grep for the existing fn that removes a context from a group.

- [ ] **Step 4: Update the dispatch**

```rust
RootOp::GroupDeleted { root_group_id, cascade_group_ids, cascade_context_ids } => {
    self.execute_group_deleted(op, *root_group_id, cascade_group_ids, cascade_context_ids)?;
}
```

- [ ] **Step 5: Run the tests**

Run: `cargo test -p calimero-context --lib execute_group_deleted 2>&1 | tail -10`
Expected: PASS, 3 tests.

- [ ] **Step 6: Commit**

```bash
git add crates/context/src/group_store/namespace_governance.rs crates/context/src/group_store/tests.rs
git commit -m "feat(context/group_store): cascade delete with determinism check

execute_group_deleted now expects a payload listing descendants and
contained contexts, re-enumerates locally, and rejects mismatches.
Children-first deletion order. Rejects deletion of the namespace root."
```

---

### Task 10: Update `delete_group` and `create_group` handlers

**Files:**
- Modify: `crates/context/src/handlers/delete_group.rs`
- Modify: `crates/context/src/handlers/create_group.rs` (if it exists; otherwise the equivalent path)
- Modify: `crates/server/src/admin/handlers/namespaces/create_group_in_namespace.rs`
- Modify: `crates/server/primitives/src/admin.rs` (or wherever the API request types live — confirm with grep)

- [ ] **Step 1: Locate the API request types**

Run: `grep -rn "CreateGroupApiRequest\|DeleteGroupApiRequest" crates/server/primitives/ crates/context-primitives/ 2>&1 | head -10`

- [ ] **Step 2: Update `CreateGroupApiRequest` to require parent_id**

In the located file, add a required `parent_id` field to `CreateGroupApiRequest`. The existing struct will have fields like `app_id`, `protocol`, etc. — add `parent_id: ContextGroupId` (or `[u8; 32]`).

- [ ] **Step 3: Update `delete_group.rs` handler**

Open `crates/context/src/handlers/delete_group.rs`. Replace the body of the `handle` fn to:
1. Pre-flight: load group meta (still required), build cascade payload via `collect_subtree_for_cascade`.
2. Build the `RootOp::GroupDeleted { root_group_id, cascade_group_ids, cascade_context_ids }` op.
3. Sign-and-publish via existing path.

Show the full updated `handle` body in the diff. Remove the existing pre-cascade "still has N contexts" check (now handled by cascade — contexts are deleted, not blocked).

- [ ] **Step 4: Update `create_group_in_namespace` handler**

In `crates/server/src/admin/handlers/namespaces/create_group_in_namespace.rs`, change the op emission from two separate ops (`GroupCreated` then `GroupNested`) to a single `GroupCreated { group_id, parent_id: namespace_id }` op. Delete the second op emission entirely.

- [ ] **Step 5: Update any other group-creation paths**

Run: `grep -rn "RootOp::GroupCreated" crates/`

For each callsite, ensure it now passes a `parent_id`. For paths that previously created a "freestanding" group (orphan), they must now specify a parent — typically the namespace root for top-level subgroups.

- [ ] **Step 6: Run handler tests + build**

Run: `cargo build -p calimero-context -p calimero-server 2>&1 | tail -20`
Expected: clean build (no errors related to our changes).

Run: `cargo test -p calimero-context --lib 2>&1 | tail -10`
Expected: all passing.

- [ ] **Step 7: Commit**

```bash
git add -u
git commit -m "feat(context/handlers): cascade delete + atomic create+nest

delete_group handler builds the cascade payload and emits a single
GroupDeleted op. create_group / create_group_in_namespace emit a
single GroupCreated { parent_id } op (dropped the separate GroupNested
follow-up op). CreateGroupApiRequest gains a required parent_id."
```

---

### Task 11: Add reparent REST handler + remove nest/unnest endpoints

**Files:**
- Create: `crates/server/src/admin/handlers/groups/reparent_group.rs`
- Delete: `crates/server/src/admin/handlers/groups/nest_group.rs`
- Delete: `crates/server/src/admin/handlers/groups/unnest_group.rs`
- Modify: `crates/server/src/admin/handlers/groups/mod.rs` (or wherever route module declarations live)
- Modify: `crates/server/src/admin/service.rs` (route registrations)
- Modify: `crates/server/primitives/src/admin.rs` (request/response types)

- [ ] **Step 1: Read an existing handler as template**

Run: `cat crates/server/src/admin/handlers/groups/nest_group.rs`

Use this as a template — the new `reparent_group.rs` will be very similar in shape (HTTP body parse, build op, sign+publish, return response).

- [ ] **Step 2: Create `ReparentGroupApiRequest` / `ReparentGroupApiResponse`**

In `crates/server/primitives/src/admin.rs`:

```rust
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReparentGroupApiRequest {
    pub new_parent_id: ContextGroupId,
    pub requester: Option<PublicKey>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReparentGroupApiResponse {
    pub reparented: bool,
}
```

Delete `NestGroupApiRequest`, `NestGroupApiResponse`, `UnnestGroupApiRequest`, `UnnestGroupApiResponse`.

- [ ] **Step 3: Create the handler file**

Create `crates/server/src/admin/handlers/groups/reparent_group.rs`. Mirror the structure of the deleted `nest_group.rs` but emit a `RootOp::GroupReparented` op via `sign_apply_and_publish_namespace_op`. The handler:
1. Parses path: `/admin-api/groups/:group_id/reparent`.
2. Parses body: `ReparentGroupApiRequest { new_parent_id, requester }`.
3. Resolves namespace identity for the (orphan-free) group via the existing `node_namespace_identity` (this works because reparent is only valid on already-nested groups).
4. Emits a single `NamespaceOp::Root(RootOp::GroupReparented { ... })`.
5. Returns `ReparentGroupApiResponse { reparented: true }` on success.

- [ ] **Step 4: Delete `nest_group.rs` and `unnest_group.rs`**

```bash
git rm crates/server/src/admin/handlers/groups/nest_group.rs
git rm crates/server/src/admin/handlers/groups/unnest_group.rs
```

- [ ] **Step 5: Update module declarations**

In `crates/server/src/admin/handlers/groups.rs` (or `.../groups/mod.rs`), remove `mod nest_group;` and `mod unnest_group;`. Add `mod reparent_group;`.

- [ ] **Step 6: Update route registrations**

In `crates/server/src/admin/service.rs`, find the route table for groups. Remove routes for `/groups/:id/nest` and `/groups/:id/unnest`. Add a route for `POST /groups/:id/reparent`.

- [ ] **Step 7: Build the server**

Run: `cargo build -p calimero-server 2>&1 | tail -10`
Expected: clean build.

- [ ] **Step 8: Commit**

```bash
git add -A
git commit -m "feat(server/admin): replace /nest and /unnest with /reparent endpoint

Drops POST /admin-api/groups/:id/nest and /unnest endpoints and their
handler files. Adds POST /admin-api/groups/:id/reparent handler that
emits a RootOp::GroupReparented governance op."
```

---

### Task 12: Update Rust client + meroctl CLI

**Files:**
- Modify: `crates/client/src/client/group.rs`
- Modify: `crates/meroctl/src/cli/group.rs`
- Create: `crates/meroctl/src/cli/group/reparent.rs`
- Delete: `crates/meroctl/src/cli/group/nest.rs`
- Delete: `crates/meroctl/src/cli/group/unnest.rs`

- [ ] **Step 1: Update the Rust client**

In `crates/client/src/client/group.rs`:
1. Delete `pub async fn nest_group(&self, ...)` and `pub async fn unnest_group(&self, ...)`.
2. Add:

```rust
pub async fn reparent_group(
    &self,
    group_id: &str,
    request: ReparentGroupApiRequest,
) -> Result<ReparentGroupApiResponse> {
    let response = self
        .connection
        .post(&format!("admin-api/groups/{group_id}/reparent"), request)
        .await?;
    Ok(response)
}
```

3. Update `delete_group` if its `DeleteGroupApiRequest` shape changed (it likely didn't, but verify).
4. Update `create_group` if its `CreateGroupApiRequest` shape changed (it did — `parent_id` is required now).

- [ ] **Step 2: Update meroctl CLI surface**

In `crates/meroctl/src/cli/group.rs`:
- Remove `mod nest;` and `mod unnest;` declarations.
- Remove their subcommand variants from the `enum GroupCommands { ... }` derive.
- Add `mod reparent;` and a `Reparent(reparent::Args)` variant.

Create `crates/meroctl/src/cli/group/reparent.rs`:

```rust
use clap::Args;
use eyre::Result as EyreResult;

use crate::cli::Environment;

#[derive(Debug, Args)]
pub struct ReparentArgs {
    /// Group ID (hex) to reparent.
    #[arg(long)]
    pub child: String,
    /// New parent group ID (hex).
    #[arg(long)]
    pub new_parent: String,
}

impl ReparentArgs {
    pub async fn run(self, env: &Environment) -> EyreResult<()> {
        let client = env.client()?;
        let resp = client
            .reparent_group(
                &self.child,
                calimero_server_primitives::admin::ReparentGroupApiRequest {
                    new_parent_id: parse_group_id(&self.new_parent)?,
                    requester: None,
                },
            )
            .await?;
        println!("{resp:?}");
        Ok(())
    }
}
```

If `parse_group_id` or `Environment` import paths differ, check existing `crates/meroctl/src/cli/group/*.rs` files.

- [ ] **Step 3: Delete nest/unnest subcommand files**

```bash
git rm crates/meroctl/src/cli/group/nest.rs crates/meroctl/src/cli/group/unnest.rs
```

- [ ] **Step 4: Update `create` subcommand to require --parent**

In `crates/meroctl/src/cli/group/create.rs`, add a required `#[arg(long)] parent: String` field. Pass it through to the `CreateGroupApiRequest`.

- [ ] **Step 5: Build meroctl**

Run: `cargo build -p meroctl 2>&1 | tail -10`
Expected: clean build.

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "feat(client, meroctl): replace nest/unnest with reparent

Rust client: drop nest_group/unnest_group methods, add reparent_group.
meroctl: drop \`group nest\` and \`group unnest\` subcommands, add
\`group reparent --child <id> --new-parent <id>\`. \`group create\` now
requires --parent."
```

---

### Task 13: Run full test suite + lint

- [ ] **Step 1: Format check**

Run: `cargo fmt --check 2>&1 | tail -10`
Expected: clean.

- [ ] **Step 2: Clippy on changed crates**

Run: `cargo clippy -p calimero-context -p calimero-server -p calimero-client -p meroctl -- -A warnings 2>&1 | tail -10`
Expected: clean.

- [ ] **Step 3: Full workspace test**

Run: `cargo test --workspace 2>&1 | tail -20`
Expected: all passing.

- [ ] **Step 4: Commit any stray fmt/clippy fixes**

If steps 1-2 produced changes, commit them: `git add -u && git commit -m "style: cargo fmt + clippy"`.

---

### Task 14: Replace E2E workflows

**Files:**
- Delete: `apps/e2e-kv-store/workflows/group-nesting.yml`
- Create: `apps/e2e-kv-store/workflows/group-reparent.yml`
- Create: `apps/e2e-kv-store/workflows/group-reparent-and-cascade-delete.yml`
- Modify: `.github/workflows/e2e-rust-apps.yml` (matrix entries)

> **Note:** Tasks 14's workflows depend on the merobox `reparent_group` step type from the merobox PR (Plan: `2026-04-22-merobox-reparent-step-type.md`). They will fail in CI until that PR lands. This is the expected coordination point.

- [ ] **Step 1: Delete the old workflow**

```bash
git rm apps/e2e-kv-store/workflows/group-nesting.yml
```

- [ ] **Step 2: Create `group-reparent.yml`**

Mirror the structure of the deleted `group-nesting.yml`, but use `type: reparent_group` to move a child between two parents and assert child-index updates on both.

- [ ] **Step 3: Create `group-reparent-and-cascade-delete.yml`**

Build a 3-level tree (namespace → A, B; B → C), put contexts in each, reparent C to A, cascade-delete A, assert: A and C and their contexts gone; B intact; namespace intact.

- [ ] **Step 4: Update CI matrix**

In `.github/workflows/e2e-rust-apps.yml`, in the matrix `include:` block:
- Remove the `group-nesting` entry.
- Add `group-reparent` and `group-reparent-and-cascade-delete` entries.

- [ ] **Step 5: Validate YAML parses**

Run: `python3 -c "import yaml; [yaml.safe_load(open(f)) for f in ['apps/e2e-kv-store/workflows/group-reparent.yml', 'apps/e2e-kv-store/workflows/group-reparent-and-cascade-delete.yml', '.github/workflows/e2e-rust-apps.yml']]; print('OK')"`
Expected: `OK`.

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "test(e2e): replace group-nesting workflow with reparent + cascade

Adds group-reparent.yml (atomic edge-swap test) and
group-reparent-and-cascade-delete.yml (3-level tree + cascade test).
Wired into e2e-rust-apps.yml matrix. Will fail in CI until the merobox
reparent_group step type lands (see merobox plan)."
```

---

### Task 15: Open the PR

- [ ] **Step 1: Push branch**

```bash
git push -u origin feat/strict-group-tree-cascade-delete
```

- [ ] **Step 2: Open the PR**

```bash
gh pr create --repo calimero-network/core --base master \
  --title "feat(context): strict group-tree invariant + cascade delete" \
  --body "$(cat <<'EOF'
## Summary

Replaces orphan-group state with a structural invariant: every group
except the namespace root has exactly one parent at all times. Drops
\`nest_group\` / \`unnest_group\`, adds atomic \`reparent_group\`. Makes
\`delete_group\` cascade over its full subtree (descendants + contexts)
in a single governance op with determinism check.

Spec: \`docs/superpowers/specs/2026-04-22-strict-group-tree-and-cascade-delete.md\`

Supersedes closed issue #2174 and closed PR #2175.

## Coordination

This PR coordinates with two satellite PRs:
- calimero-network/calimero-client-py (mirror Rust client surface in pyo3 wrappers)
- calimero-network/merobox (replace nest/unnest step types with reparent)

Landing order: this PR first, then calimero-client-py, then merobox.
The E2E workflow added here will fail in CI until the merobox PR lands.

## Test plan

- [x] cargo fmt --check
- [x] cargo clippy passes
- [x] cargo test --workspace passes
- [ ] group-reparent E2E workflow passes (after merobox PR lands)
- [ ] group-reparent-and-cascade-delete E2E workflow passes (after merobox PR lands)
EOF
)"
```

---

## Self-review checklist

- Spec § 4 (governance ops) covered by Tasks 1, 7, 8, 9.
- Spec § 5 (application semantics) covered by Tasks 7, 8, 9.
- Spec § 6 (store API) covered by Tasks 2, 3, 4, 5, 6.
- Spec § 7 (handlers + REST) covered by Tasks 10, 11.
- Spec § 8 (clients) covered by Task 12.
- Spec § 10 (testing) — store unit tests in Tasks 2-6, governance-execution tests in Tasks 7-9, E2E workflows in Task 14.
- Spec § 11 implementation sequence followed in Tasks 1 → 14.
- Spec § 12 success criteria — all items have a corresponding task.
- No "TBD" / "TODO" placeholders.
- Function/method names consistent: `reparent_group`, `collect_subtree_for_cascade`, `is_descendant_of`, `delete_all_group_members`, `execute_group_reparented`.

---

**Done.** Plan covers all spec sections; satellite plans (`2026-04-22-client-py-reparent-bindings.md` and `2026-04-22-merobox-reparent-step-type.md`) handle the cross-repo coordination.
