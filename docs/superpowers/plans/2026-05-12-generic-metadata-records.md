# Generic Metadata Records Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the group-scoped *alias* (a bare `id → String` label) with a generic, app-extensible `MetadataRecord` carried by every group / group-member / group-registered context, add a `CAN_MANAGE_METADATA` capability bit, and delete the alias store keys, ops, and HTTP/CLI surface entirely.

**Architecture:** A new `MetadataRecord { name: Option<String>, data: BTreeMap<String,String>, updated_at: u64, updated_by: PublicKey }` value type lives beside `MemberCapabilities` in `calimero-context-config`. Three new store-key types (`GroupMetadata`, `GroupMemberMetadata`, `GroupContextMetadata`) reuse the byte layout of the three alias keys they replace (only the prefix byte and the value type change). Three new `GroupOp` variants (`GroupMetadataSet`, `MemberMetadataSet`, `ContextMetadataSet`) wholly replace the target record on apply. Authorization reuses the existing `PermissionChecker` (admin-or-capability, plus `signer == member` for member metadata). Metadata is deliberately excluded from `compute_group_state_hash` (exactly as aliases were). No migration of existing alias rows — they become dead keys. The node-local `calimero_primitives::alias::Alias<T>` resolver is untouched.

**Tech Stack:** Rust workspace (cargo), borsh-derived store values, actix actors (`ContextManager`), axum admin HTTP server, `calimero-client` HTTP client, `meroctl` clap CLI, merobox YAML e2e workflows, GitHub Actions matrix.

**Design reference:** `docs/superpowers/specs/2026-05-11-generic-metadata-records-design.md` (PR #2332). This plan implements that spec; §7 (per-namespace name uniqueness) is explicitly out of scope.

**Branch/PR:** Implement on a fresh branch `feat/metadata-records` off `origin/master` (currently `007f1f06`), opened as its own PR that references #2332 for the design. PR #2332 (spec-only) is NOT merged.

---

## File Structure

**New files:**
- `crates/context/config/src/metadata.rs` — `MetadataRecord` type (or inline in `lib.rs` next to `MemberCapabilities` if a separate module is awkward; pick whichever matches the crate's layout — `lib.rs` already holds `MemberCapabilities`, so inline is fine).
- `crates/context/src/group_store/metadata.rs` — renamed-and-rewritten `aliases.rs`: `get/set/delete_*_metadata`, `enumerate_member_metadata`, `enumerate_group_contexts_with_names`, `build_namespace_summary`, `count_group_contexts`, `delete_all_member_metadata`.
- `crates/context/src/handlers/store_group_metadata.rs`, `store_member_metadata.rs`, `store_context_metadata.rs` — renamed from the `store_*_alias.rs` handlers (direct-persist path).
- `crates/context/src/handlers/set_group_metadata.rs`, `set_member_metadata.rs`, `set_context_metadata.rs` — renamed/expanded from `set_group_alias.rs` / `set_member_alias.rs` (publish a `*MetadataSet` GroupOp); add the missing `set_context_metadata` (there was no `set_context_alias` handler before — context aliases were only set on the direct-persist apply path; we now need a user-facing setter for contexts too).
- `crates/server/src/admin/handlers/groups/set_group_metadata.rs`, `set_member_metadata.rs`, `set_context_metadata.rs` — renamed from `set_group_alias.rs` / `set_member_alias.rs`; add `set_context_metadata`.
- `crates/meroctl/src/cli/group/metadata.rs` — the `group metadata`, `group member metadata`, `group context metadata` subcommand tree.
- `apps/scaffolding-e2e/workflows/group-metadata.yml` — the e2e workflow.

**Deleted files:**
- `crates/context/src/group_store/aliases.rs` (→ `metadata.rs`)
- `crates/context/src/handlers/store_group_alias.rs`, `store_member_alias.rs`, `store_context_alias.rs`, `set_group_alias.rs`, `set_member_alias.rs`, `broadcast_group_aliases.rs` (the last is a dead stub — remove outright)
- `crates/server/src/admin/handlers/groups/set_group_alias.rs`, `set_member_alias.rs`

**Modified files (key ones):**
- `crates/context/config/src/lib.rs` — `CAN_MANAGE_METADATA = 1 << 8`; re-export `MetadataRecord` if it lands in a submodule.
- `crates/store/src/key/group/mod.rs` — replace `GroupAlias`/`GroupMemberAlias`/`GroupContextAlias` (prefixes `0x2E`/`0x2D`/`0x2F`) with `GroupMetadata`/`GroupMemberMetadata`/`GroupContextMetadata` (same prefixes, value type `MetadataRecord`).
- `crates/store/src/key.rs` — re-export rename.
- `crates/context/primitives/src/local_governance/mod.rs` — `GroupOp`: remove `GroupAliasSet`/`MemberAliasSet`/`ContextAliasSet`, add `GroupMetadataSet`/`MemberMetadataSet`/`ContextMetadataSet`; `op_kind_label` arms.
- `crates/context/src/group_store/mod.rs` — `mod aliases;` → `mod metadata;`; re-exports; the three apply arms in the `GroupOp` match; `compute_group_state_hash` doc-comment.
- `crates/context/src/group_store/group_settings.rs` — `set_group_alias` method → `set_group_metadata` (or fold into the apply arm directly).
- `crates/context/src/group_store/permission_checker.rs` — add `require_can_manage_metadata`.
- `crates/context/src/group_store/local_state.rs` — `delete_group_local_rows`: `delete_group_alias`/`delete_all_member_aliases` → `delete_group_metadata`/`delete_all_member_metadata`; also delete `GroupContextMetadata` rows for the group's contexts.
- `crates/context/src/handlers/mod.rs` (or wherever handlers are registered) — handler module renames.
- `crates/context/primitives/src/group.rs` — `NamespaceSummary.alias` → `name`; rename request types (`StoreGroupAliasRequest` → `StoreGroupMetadataRequest` etc., carrying `name`+`data`); add `StoreContextMetadataRequest` if missing; add `SetContextMetadataRequest`; remove `BroadcastGroupAliasesRequest`.
- `crates/context/primitives/src/group.rs` / `crates/context-client` — `NamespaceSummary` and any `GroupInfo`/`MemberInfo`/`ContextInfo` response types that carried `alias` now carry the `MetadataRecord` (or its `name`).
- `crates/server/src/admin/...` route table — `/alias` routes → `/metadata` routes; delete the alias routes.
- `crates/server/primitives/src/admin.rs` (or wherever `SetGroupAliasApiRequest` etc. live) — rename to `*MetadataApiRequest`, body `{ name: Option<String>, data: Map<String,String> }`.
- `calimero-client` (`crates/client/...`) — `set_group_alias`/`set_member_alias` → `set_group_metadata`/`set_member_metadata`/`set_context_metadata` + `get_*`.
- `crates/meroctl/src/cli/group/mod.rs` — wire the `metadata` subcommand; remove any `--alias` flags on `group create` / `group settings`.
- `.github/workflows/e2e-rust-apps.yml` — add the `group-metadata` matrix entry.
- `architecture/storage-schema.html`, `architecture/concepts.html`, `architecture/membership-and-leave.html`, `architecture/glossary.html` (and `local-governance.html` if it has a capability table) — bump the capability-bits table to 9 bits; rename the `0x2D/0x2E/0x2F` rows from `*Alias → String` to `*Metadata → MetadataRecord`.
- `Cargo.toml` — bump `[workspace.metadata.workspaces] version` rc number.

---

## Phase 1 — `MetadataRecord` type, store keys, `group_store::metadata` module, `CAN_MANAGE_METADATA`

> Phase 1 leaves the tree **not compiling** (alias keys removed, alias callers still present). That's fine — Phases 1–3 land as sequential commits on one branch; the branch only needs to compile + pass tests at the end of Phase 3. Run `cargo build -p calimero-store -p calimero-context-config` at the end of each task to catch local breakage early.

### Task 1.1: `MetadataRecord` type

**Files:**
- Modify: `crates/context/config/src/lib.rs` (add near `MemberCapabilities`, ~line 186)
- Test: `crates/context/config/src/lib.rs` `#[cfg(test)]` module (or wherever config tests live — check the file; if none, add a `#[cfg(test)] mod tests`)

- [ ] **Step 1: Write the failing test**

In the config crate's test module:

```rust
#[cfg(test)]
mod metadata_tests {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn metadata_record_roundtrips_borsh() {
        let pk = [7u8; 32];
        let mut data = BTreeMap::new();
        let _ = data.insert("topic".to_owned(), "general chatter".to_owned());
        let _ = data.insert("color".to_owned(), "#3366ff".to_owned());
        let rec = MetadataRecord {
            name: Some("general".to_owned()),
            data,
            updated_at: 1_700_000_000_000,
            updated_by: pk.into(),
        };
        let bytes = borsh::to_vec(&rec).unwrap();
        let back: MetadataRecord = borsh::from_slice(&bytes).unwrap();
        assert_eq!(back, rec);
    }

    #[test]
    fn metadata_record_default_is_empty() {
        let rec = MetadataRecord::default();
        assert!(rec.name.is_none());
        assert!(rec.data.is_empty());
        assert_eq!(rec.updated_at, 0);
    }
}
```

(If `PublicKey`/the config-crate identity type doesn't `impl From<[u8;32]>`, adjust to the constructor it does provide — check `crates/context/config/src/types.rs` or wherever `SignerId`/the pubkey type lives. Use the same pubkey type that `GroupOp` variants already use for `member`.)

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p calimero-context-config metadata_record -- --nocapture`
Expected: FAIL — `cannot find type MetadataRecord`.

- [ ] **Step 3: Add the type**

In `crates/context/config/src/lib.rs`, after the `MemberCapabilities` impl block:

```rust
use std::collections::BTreeMap;
// (add the import at the top of the file, grouped with the other `use std::...` lines)

/// App-extensible metadata for a group, a group member, or a context
/// registered in a group. A namespace is a root group, so the group
/// variant covers it.
///
/// `data` is **opaque to core** — core stores and replicates it verbatim
/// and never reads or interprets any key in it. (A future per-namespace
/// name-uniqueness policy will live in a typed field or a separate op,
/// never inside `data` — see the design spec §7.)
///
/// `updated_at` is stamped by the *applier* at apply time, so peers may
/// disagree by a few millis; that is acceptable because metadata is
/// deliberately excluded from `compute_group_state_hash` (exactly as the
/// former alias rows were) — it is replicated governance state but not
/// consensus-relevant state.
#[derive(Clone, Debug, Default, PartialEq, Eq, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct MetadataRecord {
    /// The entity's human-readable name (the field formerly called
    /// `alias`). `None` means "no name set".
    pub name: Option<String>,
    /// Arbitrary application-defined properties. Stored and replicated
    /// verbatim; never inspected by core.
    pub data: BTreeMap<String, String>,
    /// Wall-clock millis when the most recent `*MetadataSet` op was
    /// applied locally. Informational only.
    pub updated_at: u64,
    /// Public key of the signer of the most recent `*MetadataSet` op.
    pub updated_by: PublicKey,
}
```

Use whatever pubkey type the crate already uses for op fields (the `member: PublicKey` in `GroupOp::MemberAliasSet` — confirm the path; it may be `calimero_context_config::types::SignerId` or a re-exported `PublicKey`). Match the derives the crate uses elsewhere — confirm `BorshSerialize`/`BorshDeserialize` are in scope (the crate already borsh-derives op types) and `serde::{Serialize, Deserialize}` (used by `VisibilityMode` in the same file).

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p calimero-context-config metadata_record -- --nocapture`
Expected: PASS (both tests).

- [ ] **Step 5: Commit**

```bash
git add crates/context/config/src/lib.rs
git commit -m "feat(context-config): add MetadataRecord type"
```

### Task 1.2: `CAN_MANAGE_METADATA` capability bit

**Files:**
- Modify: `crates/context/config/src/lib.rs:184` (after `CAN_MANAGE_VISIBILITY`)
- Test: same test module as Task 1.1

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn can_manage_metadata_bit_value() {
    assert_eq!(MemberCapabilities::CAN_MANAGE_METADATA, 1 << 8);
    // no overlap with the existing bits
    let existing = MemberCapabilities::CAN_CREATE_CONTEXT
        | MemberCapabilities::CAN_INVITE_MEMBERS
        | MemberCapabilities::CAN_JOIN_OPEN_SUBGROUPS
        | MemberCapabilities::MANAGE_MEMBERS
        | MemberCapabilities::MANAGE_APPLICATION
        | MemberCapabilities::CAN_CREATE_SUBGROUP
        | MemberCapabilities::CAN_DELETE_SUBGROUP
        | MemberCapabilities::CAN_MANAGE_VISIBILITY;
    assert_eq!(existing & MemberCapabilities::CAN_MANAGE_METADATA, 0);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p calimero-context-config can_manage_metadata_bit_value`
Expected: FAIL — no associated const `CAN_MANAGE_METADATA`.

- [ ] **Step 3: Add the const**

In `crates/context/config/src/lib.rs`, inside `impl MemberCapabilities`, after `CAN_MANAGE_VISIBILITY`:

```rust
    /// Set the `name` / `data` of the group, its members, or its contexts
    /// (the `*MetadataSet` ops). Group admins hold this implicitly; a
    /// member may always set *their own* member metadata regardless of
    /// holding this bit. Like [`Self::CAN_MANAGE_VISIBILITY`], the
    /// `*MetadataSet` ops are group-scoped (encrypted to the target
    /// group's members), so this check is deterministic among exactly the
    /// peers that apply it — no root-level restriction needed.
    pub const CAN_MANAGE_METADATA: u32 = 1 << 8;
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p calimero-context-config can_manage_metadata_bit_value`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/context/config/src/lib.rs
git commit -m "feat(context-config): add CAN_MANAGE_METADATA capability bit"
```

### Task 1.3: Replace the three alias store keys with metadata keys

**Files:**
- Modify: `crates/store/src/key/group/mod.rs` (lines ~676–850 — the `GroupContextAlias` / `GroupMemberAlias` / `GroupAlias` blocks and their `*_PREFIX` consts)
- Modify: `crates/store/src/key.rs:33–50` (the re-export list)
- Test: `crates/store/src/key/group/mod.rs` test module (the file has unit tests for key roundtrips — mirror an existing one, e.g. for `GroupContextAlias`)

- [ ] **Step 1: Write the failing test**

In the `crates/store/src/key/group/mod.rs` test module, mirror the existing alias-key roundtrip tests but for the new names:

```rust
#[test]
fn group_metadata_key_roundtrip() {
    let gid = [3u8; 32];
    let k = GroupMetadata::new(gid);
    assert_eq!(k.group_id(), gid);
}

#[test]
fn group_member_metadata_key_roundtrip() {
    let gid = [3u8; 32];
    let m = PublicKey::from([9u8; 32]);
    let k = GroupMemberMetadata::new(gid, m);
    assert_eq!(k.group_id(), gid);
    assert_eq!(k.member(), m);
}

#[test]
fn group_context_metadata_key_roundtrip() {
    let gid = [3u8; 32];
    let ctx = ContextId::from([5u8; 32]);
    let k = GroupContextMetadata::new(gid, ctx);
    assert_eq!(k.group_id(), gid);
    assert_eq!(k.context_id(), ctx);
}
```

Match the exact constructor/accessor names and the `PublicKey`/`ContextId` import paths the existing alias-key tests use in that file.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p calimero-store group_metadata_key`
Expected: FAIL — `cannot find type GroupMetadata`.

- [ ] **Step 3: Rename the keys + change value type**

In `crates/store/src/key/group/mod.rs`:
- Rename `GROUP_CONTEXT_ALIAS_PREFIX` → `GROUP_CONTEXT_METADATA_PREFIX` (keep `0x2F`), `GROUP_MEMBER_ALIAS_PREFIX` → `GROUP_MEMBER_METADATA_PREFIX` (keep `0x2D`), `GROUP_ALIAS_PREFIX` → `GROUP_METADATA_PREFIX` (keep `0x2E`).
- Rename the structs `GroupContextAlias` → `GroupContextMetadata`, `GroupMemberAlias` → `GroupMemberMetadata`, `GroupAlias` → `GroupMetadata`. The `Key<...>` inner tuple, the `new`/`group_id`/`member`/`context_id` impls, and any `AsKeyParts`/`FromKeyParts`/`impl_key!`-style boilerplate stay byte-for-byte the same (only the identifier names change).
- The store's `Value` association: each alias key currently maps to `String` (via whatever `impl_key!` / `Value` trait wires the value type). Change the value type for all three to `calimero_context_config::MetadataRecord`. (Check how `GroupMetaValue` does this — it maps `GroupMeta` → a borsh value type; mirror that. `calimero-store` already depends on `calimero-context-config` since `GroupMetaValue` references its types — confirm in `crates/store/Cargo.toml`; if not, add the dep.)

In `crates/store/src/key.rs:33–50`, update the re-export list: `GroupAlias, GroupContextAlias, GroupMemberAlias` → `GroupMetadata, GroupContextMetadata, GroupMemberMetadata` (keep alphabetical ordering with the rest).

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p calimero-store group_metadata_key group_member_metadata_key group_context_metadata_key`
Expected: PASS. (`cargo build -p calimero-store` should also succeed — `calimero-store` itself has no alias callers.)

- [ ] **Step 5: Commit**

```bash
git add crates/store/src/key/group/mod.rs crates/store/src/key.rs crates/store/Cargo.toml
git commit -m "refactor(store): replace Group*Alias keys with Group*Metadata (value type MetadataRecord)"
```

### Task 1.4: `group_store::metadata` module (rewrite of `aliases.rs`)

**Files:**
- Delete: `crates/context/src/group_store/aliases.rs`
- Create: `crates/context/src/group_store/metadata.rs`
- Modify: `crates/context/src/group_store/mod.rs:15` (`mod aliases;` → `mod metadata;`) and `:44-48` (re-export block)
- Test: `crates/context/src/group_store/tests.rs` — exercise the new accessors directly (a store fixture exists; mirror an existing `aliases`-touching test if there is one, else write fresh)

- [ ] **Step 1: Write the failing test**

In `crates/context/src/group_store/tests.rs`, add (adapt the store-fixture helper name to whatever the file uses — search for `fn test_store` / `setup_store` / similar):

```rust
#[test]
fn group_metadata_set_get_delete_roundtrip() {
    let store = test_store();
    let gid = ContextGroupId::from([1u8; 32]);
    assert!(group_store::get_group_metadata(&store, &gid).unwrap().is_none());

    let mut data = std::collections::BTreeMap::new();
    let _ = data.insert("topic".to_owned(), "hi".to_owned());
    let rec = calimero_context_config::MetadataRecord {
        name: Some("general".to_owned()),
        data,
        updated_at: 42,
        updated_by: PrivateKey::random(&mut OsRng).public_key(),
    };
    group_store::set_group_metadata(&store, &gid, &rec).unwrap();
    assert_eq!(group_store::get_group_metadata(&store, &gid).unwrap().unwrap(), rec);

    group_store::delete_group_metadata(&store, &gid).unwrap();
    assert!(group_store::get_group_metadata(&store, &gid).unwrap().is_none());
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p calimero-context group_metadata_set_get_delete_roundtrip`
Expected: FAIL — `cannot find function get_group_metadata`.

- [ ] **Step 3: Write `metadata.rs`**

Create `crates/context/src/group_store/metadata.rs`. It is `aliases.rs` rewritten: every `*_alias` fn becomes `*_metadata`, the `String` value type becomes `calimero_context_config::MetadataRecord`, and the `set_*` fns take `&MetadataRecord` (the apply arm constructs the record with `updated_at`/`updated_by` filled). Concretely:

```rust
use std::collections::BTreeMap;

use calimero_context_config::types::ContextGroupId;
use calimero_context_config::MetadataRecord;
use calimero_primitives::context::ContextId;
use calimero_primitives::identity::PublicKey;
use calimero_store::key::{
    GroupContextIndex, GroupContextMetadata, GroupMemberMetadata, GroupMetaValue, GroupMetadata,
    GROUP_CONTEXT_INDEX_PREFIX, GROUP_MEMBER_METADATA_PREFIX,
};
use calimero_store::Store;
use eyre::Result as EyreResult;

use super::{
    check_group_membership, collect_keys_with_prefix, count_group_members, count_keys_with_prefix,
    enumerate_group_contexts, get_parent_group, list_child_groups,
};

/// Store the full [`MetadataRecord`] for a context within a group.
pub fn set_context_metadata(
    store: &Store,
    group_id: &ContextGroupId,
    context_id: &ContextId,
    record: &MetadataRecord,
) -> EyreResult<()> {
    let mut handle = store.handle();
    handle.put(&GroupContextMetadata::new(group_id.to_bytes(), *context_id), record)?;
    Ok(())
}

/// Returns the [`MetadataRecord`] for a context within a group, if one was set.
pub fn get_context_metadata(
    store: &Store,
    group_id: &ContextGroupId,
    context_id: &ContextId,
) -> EyreResult<Option<MetadataRecord>> {
    let handle = store.handle();
    handle
        .get(&GroupContextMetadata::new(group_id.to_bytes(), *context_id))
        .map_err(Into::into)
}

/// Returns context IDs together with their optional display names (`MetadataRecord.name`).
pub fn enumerate_group_contexts_with_names(
    store: &Store,
    group_id: &ContextGroupId,
    offset: usize,
    limit: usize,
) -> EyreResult<Vec<(ContextId, Option<String>)>> {
    let ids = enumerate_group_contexts(store, group_id, offset, limit)?;
    ids.into_iter()
        .map(|ctx_id| {
            let name = get_context_metadata(store, group_id, &ctx_id)?.and_then(|r| r.name);
            Ok((ctx_id, name))
        })
        .collect()
}

/// Store the full [`MetadataRecord`] for a group member.
pub fn set_member_metadata(
    store: &Store,
    group_id: &ContextGroupId,
    member: &PublicKey,
    record: &MetadataRecord,
) -> EyreResult<()> {
    let mut handle = store.handle();
    handle.put(&GroupMemberMetadata::new(group_id.to_bytes(), *member), record)?;
    Ok(())
}

/// Returns the [`MetadataRecord`] for a group member, if one was set.
pub fn get_member_metadata(
    store: &Store,
    group_id: &ContextGroupId,
    member: &PublicKey,
) -> EyreResult<Option<MetadataRecord>> {
    let handle = store.handle();
    handle
        .get(&GroupMemberMetadata::new(group_id.to_bytes(), *member))
        .map_err(Into::into)
}

/// Store the full [`MetadataRecord`] for the group itself.
pub fn set_group_metadata(
    store: &Store,
    group_id: &ContextGroupId,
    record: &MetadataRecord,
) -> EyreResult<()> {
    let mut handle = store.handle();
    handle.put(&GroupMetadata::new(group_id.to_bytes()), record)?;
    Ok(())
}

/// Returns the [`MetadataRecord`] for a group, if one was set.
pub fn get_group_metadata(
    store: &Store,
    group_id: &ContextGroupId,
) -> EyreResult<Option<MetadataRecord>> {
    let handle = store.handle();
    handle.get(&GroupMetadata::new(group_id.to_bytes())).map_err(Into::into)
}

/// Build a `NamespaceSummary` for a root group, fetching counts from the store.
///
/// Returns `None` if the group has a parent (not a namespace root) or if
/// `node_identity` is not a member.
pub fn build_namespace_summary(
    store: &Store,
    group_id: &ContextGroupId,
    meta: &GroupMetaValue,
    node_identity: &PublicKey,
) -> EyreResult<Option<calimero_context_client::group::NamespaceSummary>> {
    if get_parent_group(store, group_id)?.is_some() {
        return Ok(None);
    }
    if !check_group_membership(store, group_id, node_identity)? {
        return Ok(None);
    }

    let name = get_group_metadata(store, group_id).ok().flatten().and_then(|r| r.name);
    let member_count = count_group_members(store, group_id).unwrap_or(0);
    let context_count = enumerate_group_contexts(store, group_id, 0, usize::MAX)
        .unwrap_or_default()
        .len();
    let subgroup_count = list_child_groups(store, group_id).unwrap_or_default().len();

    Ok(Some(calimero_context_client::group::NamespaceSummary {
        namespace_id: *group_id,
        app_key: meta.app_key.into(),
        target_application_id: meta.target_application_id,
        upgrade_policy: meta.upgrade_policy.clone(),
        created_at: meta.created_at,
        name,
        member_count,
        context_count,
        subgroup_count,
    }))
}

/// Returns all member metadata stored for a group as `(PublicKey, MetadataRecord)` pairs.
pub fn enumerate_member_metadata(
    store: &Store,
    group_id: &ContextGroupId,
) -> EyreResult<Vec<(PublicKey, MetadataRecord)>> {
    let gid = group_id.to_bytes();
    let keys = collect_keys_with_prefix(
        store,
        GroupMemberMetadata::new(gid, PublicKey::from([0u8; 32])),
        GROUP_MEMBER_METADATA_PREFIX,
        |k| k.group_id() == gid,
    )?;
    let handle = store.handle();
    let mut results = Vec::new();
    for key in keys {
        let Some(rec) = handle.get(&key)? else { continue };
        results.push((key.member(), rec));
    }
    Ok(results)
}

pub fn count_group_contexts(store: &Store, group_id: &ContextGroupId) -> EyreResult<usize> {
    let gid = group_id.to_bytes();
    count_keys_with_prefix(
        store,
        GroupContextIndex::new(gid, ContextId::from([0u8; 32])),
        GROUP_CONTEXT_INDEX_PREFIX,
        |k| k.group_id() == gid,
    )
}

pub fn delete_group_metadata(store: &Store, group_id: &ContextGroupId) -> EyreResult<()> {
    let mut handle = store.handle();
    handle.delete(&GroupMetadata::new(group_id.to_bytes()))?;
    Ok(())
}

pub fn delete_member_metadata(
    store: &Store,
    group_id: &ContextGroupId,
    member: &PublicKey,
) -> EyreResult<()> {
    let mut handle = store.handle();
    handle.delete(&GroupMemberMetadata::new(group_id.to_bytes(), *member))?;
    Ok(())
}

pub fn delete_all_member_metadata(store: &Store, group_id: &ContextGroupId) -> EyreResult<()> {
    let gid = group_id.to_bytes();
    let keys = collect_keys_with_prefix(
        store,
        GroupMemberMetadata::new(gid, PublicKey::from([0u8; 32])),
        GROUP_MEMBER_METADATA_PREFIX,
        |k| k.group_id() == gid,
    )?;
    let mut handle = store.handle();
    for key in keys {
        handle.delete(&key)?;
    }
    Ok(())
}

pub fn delete_context_metadata(
    store: &Store,
    group_id: &ContextGroupId,
    context_id: &ContextId,
) -> EyreResult<()> {
    let mut handle = store.handle();
    handle.delete(&GroupContextMetadata::new(group_id.to_bytes(), *context_id))?;
    Ok(())
}
```

Then in `crates/context/src/group_store/mod.rs`:
- line 15: `mod aliases;` → `mod metadata;`
- the re-export block (lines ~44-48): replace the alias names with:

```rust
pub use self::metadata::{
    build_namespace_summary, count_group_contexts, delete_all_member_metadata,
    delete_context_metadata, delete_group_metadata, delete_member_metadata,
    enumerate_group_contexts_with_names, enumerate_member_metadata, get_context_metadata,
    get_group_metadata, get_member_metadata, set_context_metadata, set_group_metadata,
    set_member_metadata,
};
```

Delete `crates/context/src/group_store/aliases.rs`.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p calimero-context group_metadata_set_get_delete_roundtrip`
Expected: PASS. (The crate won't fully build yet — Phase 2/3 still pending — so use `cargo test -p calimero-context --lib group_metadata_set_get_delete_roundtrip 2>&1 | tail -30` and confirm the *test* compiles+passes; downstream-crate breakage is expected.)

- [ ] **Step 5: Commit**

```bash
git add crates/context/src/group_store/metadata.rs crates/context/src/group_store/mod.rs crates/context/src/group_store/tests.rs
git rm crates/context/src/group_store/aliases.rs
git commit -m "refactor(context): rewrite group_store::aliases as group_store::metadata"
```

### Task 1.5: `PermissionChecker::require_can_manage_metadata`

**Files:**
- Modify: `crates/context/src/group_store/permission_checker.rs` (add beside `require_can_manage_visibility`)
- Test: `crates/context/src/group_store/tests.rs` (mirror the existing `require_can_manage_visibility` test)

- [ ] **Step 1: Write the failing test**

Find the existing test for `require_can_manage_visibility` (search `require_can_manage_visibility` in `tests.rs`) and clone it with `_metadata` substituted — admin passes; bare member fails; member with `CAN_MANAGE_METADATA` granted passes. Example skeleton (adapt to the file's actual fixture API):

```rust
#[test]
fn require_can_manage_metadata_admin_or_cap() {
    let (store, group_id, admin_pk, member_pk) = setup_group_with_member(); // whatever the file uses
    let checker = PermissionChecker::new(&store, &group_id);
    assert!(checker.require_can_manage_metadata(&admin_pk).is_ok());
    assert!(checker.require_can_manage_metadata(&member_pk).is_err());
    set_member_capabilities(&store, &group_id, &member_pk, MemberCapabilities::CAN_MANAGE_METADATA).unwrap();
    let checker = PermissionChecker::new(&store, &group_id);
    assert!(checker.require_can_manage_metadata(&member_pk).is_ok());
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p calimero-context require_can_manage_metadata_admin_or_cap`
Expected: FAIL — no method `require_can_manage_metadata`.

- [ ] **Step 3: Add the method**

In `permission_checker.rs`, copy the `require_can_manage_visibility` method verbatim and substitute the capability constant + the error message:

```rust
    /// Allow if the identity is a group admin (incl. inherited admin) or
    /// holds [`MemberCapabilities::CAN_MANAGE_METADATA`] for this group.
    pub fn require_can_manage_metadata(&self, identity: &PublicKey) -> EyreResult<()> {
        self.is_authorized_with_capability(identity, MemberCapabilities::CAN_MANAGE_METADATA)
            .wrap_err("identity may not manage this group's metadata")
    }
```

(Match the exact helper name `require_can_manage_visibility` uses — it may be `is_authorized_with_capability` or `require_capability_or_admin`; copy it exactly.)

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p calimero-context require_can_manage_metadata_admin_or_cap`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/context/src/group_store/permission_checker.rs crates/context/src/group_store/tests.rs
git commit -m "feat(context): PermissionChecker::require_can_manage_metadata"
```

---

## Phase 2 — `GroupOp::*MetadataSet` variants, apply arms, authorization; remove `*AliasSet`

### Task 2.1: Replace the three `*AliasSet` `GroupOp` variants

**Files:**
- Modify: `crates/context/primitives/src/local_governance/mod.rs:113-120` (variant defs), `:188-214` (`op_kind_label`)
- Test: `crates/context/primitives/src/local_governance/mod.rs` test module (if it has `op_kind_label` tests) or `crates/context/src/group_store/tests.rs`

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn metadata_set_op_kind_labels() {
    use std::collections::BTreeMap;
    let g = GroupOp::GroupMetadataSet { name: None, data: BTreeMap::new() };
    assert_eq!(g.op_kind_label(), "group_metadata_set");
    let m = GroupOp::MemberMetadataSet {
        member: PublicKey::from([1u8; 32]),
        name: Some("x".to_owned()),
        data: BTreeMap::new(),
    };
    assert_eq!(m.op_kind_label(), "member_metadata_set");
    let c = GroupOp::ContextMetadataSet {
        context_id: ContextId::from([2u8; 32]),
        name: None,
        data: BTreeMap::new(),
    };
    assert_eq!(c.op_kind_label(), "context_metadata_set");
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p calimero-context-primitives metadata_set_op_kind_labels` (adjust crate name — check `crates/context/primitives/Cargo.toml` for the package name; likely `calimero-context-primitives`).
Expected: FAIL — no variant `GroupMetadataSet`.

- [ ] **Step 3: Replace the variants**

In `crates/context/primitives/src/local_governance/mod.rs`:
- Remove `GroupOp::ContextAliasSet { context_id, alias }`, `GroupOp::MemberAliasSet { member, alias }`, `GroupOp::GroupAliasSet { alias }`.
- Add (preserving the enum's existing field/derive conventions — `BTreeMap` needs `use std::collections::BTreeMap;` at the top of the file):

```rust
    /// Wholly replace the group's own metadata record.
    GroupMetadataSet {
        name: Option<String>,
        data: BTreeMap<String, String>,
    },
    /// Wholly replace a group member's metadata record.
    MemberMetadataSet {
        member: PublicKey,
        name: Option<String>,
        data: BTreeMap<String, String>,
    },
    /// Wholly replace a group-registered context's metadata record.
    ContextMetadataSet {
        context_id: ContextId,
        name: Option<String>,
        data: BTreeMap<String, String>,
    },
```

- In `op_kind_label`, remove the three `*_alias_set` arms and add:

```rust
        GroupOp::GroupMetadataSet { .. } => "group_metadata_set",
        GroupOp::MemberMetadataSet { .. } => "member_metadata_set",
        GroupOp::ContextMetadataSet { .. } => "context_metadata_set",
```

- [ ] **Step 4: Run test to verify it fails to compile elsewhere (expected) but the new test passes**

Run: `cargo test -p calimero-context-primitives metadata_set_op_kind_labels`
Expected: PASS for this crate (it has no alias *callers*; only the enum + label fn). Downstream crates won't build until Task 2.2 — that's expected.

- [ ] **Step 5: Commit**

```bash
git add crates/context/primitives/src/local_governance/mod.rs
git commit -m "feat(context): replace GroupOp *AliasSet variants with *MetadataSet"
```

### Task 2.2: Apply arms + authorization for `*MetadataSet`; remove `*AliasSet` apply arms

**Files:**
- Modify: `crates/context/src/group_store/mod.rs:889-899` (the `GroupOp` match arms) and `compute_group_state_hash` doc-comment in `crates/context/src/group_store/meta.rs:75`
- Modify: `crates/context/src/group_store/group_settings.rs` (remove/rename `set_group_alias` method) — or, simpler, inline the logic into the apply arm and delete the method
- Modify: `crates/context/src/group_store/local_state.rs` (`delete_group_local_rows`: alias deletes → metadata deletes; also delete `GroupContextMetadata` rows — enumerate the group's contexts via `enumerate_group_contexts` and `delete_context_metadata` each, mirroring whatever the existing code does for per-context cleanup, if anything; if it didn't clean per-context alias rows before, keep parity — but note context metadata rows for a deleted group become dead keys, acceptable per spec §2.2)
- Test: `crates/context/src/group_store/tests.rs`

- [ ] **Step 1: Write the failing tests**

Add to `tests.rs` (adapt fixture/helpers to the file's conventions — look at the existing `apply_local_member_alias_member_signer_or_admin` test, which this set replaces/supersedes):

```rust
#[test]
fn apply_group_metadata_set_replaces_record_and_stamps_signer() {
    let h = group_fixture_with_admin(); // -> { store, group_id, admin_sk }
    let mut data = std::collections::BTreeMap::new();
    let _ = data.insert("topic".to_owned(), "general".to_owned());
    apply_signed_group_op(&h, &h.admin_sk, GroupOp::GroupMetadataSet {
        name: Some("general".to_owned()),
        data: data.clone(),
    }).unwrap();
    let rec = group_store::get_group_metadata(&h.store, &h.group_id).unwrap().unwrap();
    assert_eq!(rec.name.as_deref(), Some("general"));
    assert_eq!(rec.data, data);
    assert_eq!(rec.updated_by, h.admin_sk.public_key());

    // re-apply with name cleared + data replaced
    apply_signed_group_op(&h, &h.admin_sk, GroupOp::GroupMetadataSet {
        name: None,
        data: std::collections::BTreeMap::new(),
    }).unwrap();
    let rec = group_store::get_group_metadata(&h.store, &h.group_id).unwrap().unwrap();
    assert!(rec.name.is_none());
    assert!(rec.data.is_empty());
}

#[test]
fn apply_group_metadata_set_rejects_bare_member() {
    let h = group_fixture_with_admin_and_member(); // adds member_sk (no caps)
    let err = apply_signed_group_op(&h, &h.member_sk, GroupOp::GroupMetadataSet {
        name: Some("x".to_owned()), data: Default::default(),
    }).unwrap_err();
    assert!(err.to_string().contains("metadata"));
}

#[test]
fn apply_member_metadata_set_allows_self_but_not_other_for_bare_member() {
    let h = group_fixture_with_admin_and_member();
    // member sets their own -> ok
    apply_signed_group_op(&h, &h.member_sk, GroupOp::MemberMetadataSet {
        member: h.member_sk.public_key(),
        name: Some("me".to_owned()), data: Default::default(),
    }).unwrap();
    assert_eq!(
        group_store::get_member_metadata(&h.store, &h.group_id, &h.member_sk.public_key()).unwrap().unwrap().name.as_deref(),
        Some("me")
    );
    // member tries to set the admin's -> rejected
    let err = apply_signed_group_op(&h, &h.member_sk, GroupOp::MemberMetadataSet {
        member: h.admin_sk.public_key(),
        name: Some("nope".to_owned()), data: Default::default(),
    }).unwrap_err();
    assert!(err.to_string().contains("metadata") || err.to_string().contains("admin"));
}

#[test]
fn can_manage_metadata_cap_unlocks_group_and_context_metadata() {
    let h = group_fixture_with_admin_and_member();
    set_member_capabilities(&h.store, &h.group_id, &h.member_sk.public_key(),
        MemberCapabilities::CAN_MANAGE_METADATA).unwrap();
    apply_signed_group_op(&h, &h.member_sk, GroupOp::GroupMetadataSet {
        name: Some("by-cap".to_owned()), data: Default::default(),
    }).unwrap();
    assert_eq!(
        group_store::get_group_metadata(&h.store, &h.group_id).unwrap().unwrap().name.as_deref(),
        Some("by-cap")
    );
}

#[test]
fn metadata_set_does_not_change_group_state_hash() {
    let h = group_fixture_with_admin();
    let before = group_store::compute_group_state_hash(&h.store, &h.group_id).unwrap();
    apply_signed_group_op(&h, &h.admin_sk, GroupOp::GroupMetadataSet {
        name: Some("x".to_owned()), data: Default::default(),
    }).unwrap();
    let after = group_store::compute_group_state_hash(&h.store, &h.group_id).unwrap();
    assert_eq!(before, after);
}
```

Use the real fixture/helper names from `tests.rs`. If there's no `apply_signed_group_op` helper, there is certainly *something* equivalent that drives the `GroupOp` match (the alias tests used it) — reuse that.

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p calimero-context metadata_set 2>&1 | tail -40`
Expected: FAIL — `GroupMetadataSet` apply not handled / `get_group_metadata` works but no op writes it.

- [ ] **Step 3: Implement the apply arms**

In `crates/context/src/group_store/mod.rs`, replace the three alias arms (~889-899) with:

```rust
GroupOp::GroupMetadataSet { name, data } => {
    permissions.require_can_manage_metadata(signer)?;
    set_group_metadata(store, group_id, &MetadataRecord {
        name: name.clone(),
        data: data.clone(),
        updated_at: now_ms(),
        updated_by: *signer,
    })?;
}
GroupOp::MemberMetadataSet { member, name, data } => {
    if signer != member {
        permissions.require_can_manage_metadata(signer)?;
    }
    set_member_metadata(store, group_id, member, &MetadataRecord {
        name: name.clone(),
        data: data.clone(),
        updated_at: now_ms(),
        updated_by: *signer,
    })?;
}
GroupOp::ContextMetadataSet { context_id, name, data } => {
    permissions.require_can_manage_metadata(signer)?;
    set_context_metadata(store, group_id, context_id, &MetadataRecord {
        name: name.clone(),
        data: data.clone(),
        updated_at: now_ms(),
        updated_by: *signer,
    })?;
}
```

- Add `use calimero_context_config::MetadataRecord;` and bring `set_group_metadata`/`set_member_metadata`/`set_context_metadata` into scope (they're already re-exported from `self::metadata` — use `self::set_group_metadata` etc. or whatever the file's import style is).
- `now_ms()`: use whatever wall-clock helper the codebase already uses for `created_at` on `GroupMetaValue` (search for `created_at` assignment in `execute_group_created` / wherever a group is born — there's a `now`/`unix_millis`/`SystemTime` helper; reuse it, don't add a new one).
- Delete `GroupSettingsService::set_group_alias` (and its declaration) from `group_settings.rs` — the apply arm no longer routes through it. If the service struct only existed for that one method, check whether anything else uses it before removing the whole struct; otherwise just remove the method.

In `crates/context/src/group_store/meta.rs`, update the `compute_group_state_hash` doc-comment to mention metadata:

```rust
/// Note: metadata records (`name` / `data` / `updated_at` / `updated_by`) are
/// intentionally **excluded** from this hash — exactly as the former alias
/// rows were — so the hash stays a function of consensus-relevant state only
/// (group meta + sorted member set + roles).
```

In `crates/context/src/group_store/local_state.rs`, in `delete_group_local_rows`: `delete_all_member_aliases(...)` → `delete_all_member_metadata(...)`, `delete_group_alias(...)` → `delete_group_metadata(...)`. (Per-context metadata rows: leave as-is unless the existing code already iterated contexts for cleanup — match prior behavior.)

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p calimero-context metadata_set 2>&1 | tail -40`
Expected: PASS (all five). The `calimero-context` crate should now compile (`cargo build -p calimero-context`). Downstream crates (`calimero-server`, `calimero-client`, `meroctl`, `calimero-node`) still won't until Phase 3.

- [ ] **Step 5: Commit**

```bash
git add crates/context/src/group_store/mod.rs crates/context/src/group_store/meta.rs crates/context/src/group_store/local_state.rs crates/context/src/group_store/group_settings.rs crates/context/src/group_store/tests.rs
git commit -m "feat(context): apply *MetadataSet GroupOps with admin-or-CAN_MANAGE_METADATA auth"
```

---

## Phase 3 — request types, ContextManager handlers, HTTP routes, client, meroctl; remove alias surface; `NamespaceSummary.name`

### Task 3.1: Rename request types + `NamespaceSummary.name`; remove `BroadcastGroupAliasesRequest`

**Files:**
- Modify: `crates/context/primitives/src/group.rs` — the `*AliasRequest` structs and `NamespaceSummary`
- Modify: every referrer (compiler will list them) — `crates/context/src/handlers/*.rs`, `crates/server/src/admin/...`, `crates/client/...`, `crates/meroctl/...`
- Test: covered by the e2e + the existing crate test suites; no new unit test for the rename itself (the compiler is the test). Optionally add a `serde` roundtrip test for the new request bodies.

- [ ] **Step 1: Apply the renames**

In `crates/context/primitives/src/group.rs`:
- `StoreGroupAliasRequest { group_id, alias: String }` → `StoreGroupMetadataRequest { group_id, record: MetadataRecord }` (direct-persist; carries the fully-formed record).
- `StoreMemberAliasRequest { group_id, member, alias }` → `StoreMemberMetadataRequest { group_id, member, record: MetadataRecord }`.
- `StoreContextAliasRequest { group_id, context_id, alias }` → `StoreContextMetadataRequest { group_id, context_id, record: MetadataRecord }`.
- `SetGroupAliasRequest { group_id, alias, requester }` → `SetGroupMetadataRequest { group_id, name: Option<String>, data: BTreeMap<String,String>, requester: Option<PublicKey> }`.
- `SetMemberAliasRequest { group_id, member, alias, requester }` → `SetMemberMetadataRequest { group_id, member, name, data, requester }`.
- Add `SetContextMetadataRequest { group_id, context_id, name, data, requester }` (new — there was no `SetContextAliasRequest`).
- `NamespaceSummary.alias: Option<String>` → `name: Option<String>` (no compat alias).
- Delete `BroadcastGroupAliasesRequest` and its `impl Message`.
- If any other response types in `group.rs` (e.g. a `GroupInfo` / `MemberInfo` / `ContextInfo` / `SubgroupSummary`) carry an `alias` field, rename it to `name` or to a `metadata: MetadataRecord` field — pick `metadata: MetadataRecord` for the detailed get-info responses (HTTP `GET` should return the full record per spec §5) and a bare `name: Option<String>` for list/summary rows.

`crates/context/primitives/Cargo.toml` already depends on `calimero-context-config` (it references `MemberCapabilities`); confirm and add `use calimero_context_config::MetadataRecord;`.

- [ ] **Step 2: Update the ContextManager handlers**

- Rename `crates/context/src/handlers/store_group_alias.rs` → `store_group_metadata.rs`; the body becomes `group_store::set_group_metadata(&self.datastore, &group_id, &record)`. Same for member/context.
- Rename `set_group_alias.rs` → `set_group_metadata.rs`: build `GroupOp::GroupMetadataSet { name, data }` and `sign_and_publish_group_op(...)` (mirror exactly what `set_group_alias.rs` did with `GroupOp::GroupAliasSet`). Same for `set_member_alias.rs` → `set_member_metadata.rs` (→ `GroupOp::MemberMetadataSet`).
- Add `crates/context/src/handlers/set_context_metadata.rs` → `GroupOp::ContextMetadataSet { context_id, name, data }` (clone the `set_member_metadata.rs` structure).
- Delete `crates/context/src/handlers/broadcast_group_aliases.rs`.
- Update `crates/context/src/handlers/mod.rs` (or wherever `mod store_group_alias;` etc. are declared and the `Handler` impls registered) — rename the `mod` lines, drop `broadcast_group_aliases`.
- Search for the `BroadcastGroupAliasesRequest` *sender* (something earlier was supposed to call it on namespace events) — if there's a `ctx.address().do_send(BroadcastGroupAliasesRequest { .. })` somewhere, delete that call site too.

- [ ] **Step 3: Update HTTP routes + admin request types**

- `crates/server/primitives/src/admin.rs` (or wherever `SetGroupAliasApiRequest` lives): rename to `SetGroupMetadataApiRequest { name: Option<String>, data: BTreeMap<String,String> }`; add `SetMemberMetadataApiRequest`, `SetContextMetadataApiRequest`.
- `crates/server/src/admin/handlers/groups/set_group_alias.rs` → `set_group_metadata.rs`: maps the API request → `SetGroupMetadataRequest` actor message; the route changes from `POST /admin-api/.../groups/:group_id/alias` to `.../groups/:group_id/metadata`. Same for member; add a context route `.../groups/:group_id/contexts/:context_id/metadata`.
- For the `GET` info endpoints (group info / member info / context info / namespace summary) that previously surfaced `alias`, surface the `MetadataRecord` (or `name`) — wherever the response is assembled, swap `get_group_alias` → `get_group_metadata` etc.
- Update the route registration in `crates/server/src/admin/mod.rs` (or the router builder) — rename the `/alias` routes to `/metadata`, add the context one, remove nothing else.
- If there was a `broadcast`-ish admin route, remove it.

- [ ] **Step 4: Update `calimero-client`**

In `crates/client/...` (search `set_group_alias` / `alias` in the client crate): rename the methods to `set_group_metadata(group_id, name, data)`, `set_member_metadata(...)`, add `set_context_metadata(...)`, and `get_group_metadata`/`get_member_metadata`/`get_context_metadata` returning `MetadataRecord`. Point them at the new `/metadata` routes. Remove the alias methods.

- [ ] **Step 5: Update meroctl**

- Create `crates/meroctl/src/cli/group/metadata.rs` with a `clap` subcommand tree:
  - `GroupMetadataCommand` enum: `Get { group_id }`, `Set { group_id, #[arg(long)] name: Option<String>, #[arg(long)] clear_name: bool, #[arg(long = "set", value_parser = parse_kv)] set: Vec<(String,String)>, #[arg(long = "unset")] unset: Vec<String>, #[arg(long)] replace_data: bool }`.
  - `MemberMetadataCommand`: same fields + `member: PublicKey`.
  - `ContextMetadataCommand`: same fields + `context_id: ContextId`.
  - `Set` semantics: fetch the current `MetadataRecord` (via `get_*_metadata`); if `--replace-data`, start from an empty `data` and insert the `--set` pairs; else patch the current `data` with `--set` inserts and `--unset` removes. `name`: `--name` overrides; `--clear-name` sets `None`; neither → keep current. Then call the client `set_*_metadata` with the resulting `(name, data)`.
  - `parse_kv`: split on the first `=` into `(key, value)`; error on missing `=`.
- Wire into `crates/meroctl/src/cli/group/mod.rs`: add `Metadata(GroupMetadataCommand)`, and under a `member`/`context` subgroup add `Metadata(...)` (mirror how `group member` / `group context` subcommands are already structured — if they aren't, add `group member metadata` / `group context metadata` as top-level `group` subcommands `MemberMetadata` / `ContextMetadata`).
- Remove any `--alias` flags on `group create` / `group settings` and any `group ... alias` subcommand. If `group create` had an `--alias`, drop it (a name is set post-create via `group metadata set`).
- Update the `group get` / `group members` / `group contexts` output rendering to print `name` (and a short `data` summary, e.g. `data: 3 keys` or the keys list).

- [ ] **Step 6: Update `architecture/` docs**

- `architecture/storage-schema.html`: the `0x2D` / `0x2E` / `0x2F` rows: `GroupMemberAlias … → String` becomes `GroupMemberMetadata … → MetadataRecord`, etc.
- `architecture/concepts.html`, `architecture/membership-and-leave.html`, `architecture/glossary.html` (and `local-governance.html` if present): the capability-bits table gains a 9th row `CAN_MANAGE_METADATA = 1 << 8 — set name/data of a group, its members, or its contexts`. Any prose mentioning "alias" in the group-governance sense → "metadata / name".

- [ ] **Step 7: Build the whole workspace**

Run: `cargo build --workspace 2>&1 | tail -40`
Expected: clean build. Fix any remaining `alias` references the compiler flags (search `git grep -wi alias -- 'crates/**' ':!crates/**/alias*'` to find stragglers — but be careful NOT to touch the node-local `calimero_primitives::alias::Alias<T>` system: `crates/node/primitives/src/client/alias.rs`, `crates/meroctl/src/cli/context/alias.rs`, the `/alias/context/:name` routes, `--as` flags — those stay).

- [ ] **Step 8: Run the full test suite**

Run: `cargo test --workspace 2>&1 | tail -40`
Expected: all pass. (Watch for any test that referenced the old alias request types / `NamespaceSummary.alias` — fix to `name`.)

- [ ] **Step 9: Commit**

```bash
git add -A
git commit -m "feat: metadata HTTP/client/CLI surface; remove group alias routes & flags; NamespaceSummary.name"
```

---

## Phase 4 — e2e workflow, CI matrix entry, version bump

### Task 4.1: `group-metadata.yml` e2e workflow + matrix entry

**Files:**
- Create: `apps/scaffolding-e2e/workflows/group-metadata.yml`
- Modify: `.github/workflows/e2e-rust-apps.yml` (add the matrix entry — note: editing files under `.github/workflows/` may trip a security-reminder pre-tool hook; if an Edit is blocked, retry the identical Edit and it goes through)
- Test: the workflow itself is the test; validate locally if a merobox runner is available, else rely on CI.

- [ ] **Step 1: Write `group-metadata.yml`**

Mirror the step vocabulary of an existing 2-node workflow (read `apps/scaffolding-e2e/workflows/group-subgroup-visibility-inheritance.yml` for the exact step names — `install_application`, `create_namespace`, `create_namespace_invitation`, `join_namespace`, `create_group_in_namespace`, `create_context`, `set_member_capabilities`, `wait_for_sync`, `wait`, `assert`, `json_assert`, `expected_failure`, plus whatever the *metadata* step is named — there will be a `set_group_metadata` / `get_group_metadata` step type once the merod side exists; if merobox's step set doesn't yet expose metadata steps, the workflow uses generic `call`/HTTP steps against the `/metadata` admin route, OR add the merobox steps in a companion merobox PR — check `apps/scaffolding-e2e` for how alias was driven; aliases were driven via... [confirm during impl — if there was no alias e2e step, the metadata steps need adding to merobox first; coordinate like the earlier merobox#231 work]).

Skeleton (adjust step names to merobox's actual vocabulary):

```yaml
name: group-metadata
description: >
  Generic metadata records on a namespace, a subgroup, and a context —
  set by an admin, read back after sync by a second node; CAN_MANAGE_METADATA
  unlocks it for a non-admin; a member can always set their own member metadata.

nodes:
  count: 2

steps:
  - name: install app on node-1
    type: install_application
    node: node-1
    path: ./res/blobstore.wasm   # or whatever the scaffolding app artifact is
    outputs: { app_id: applicationId }

  - name: node-1 creates namespace
    type: create_namespace
    node: node-1
    application_id: "{{app_id}}"
    outputs: { ns: namespaceId, node1_id: memberPublicKey }

  - name: node-1 creates a subgroup
    type: create_group_in_namespace
    node: node-1
    namespace_id: "{{ns}}"
    outputs: { sub: groupId }

  - name: node-1 creates a context in the subgroup
    type: create_context
    node: node-1
    group_id: "{{sub}}"
    application_id: "{{app_id}}"
    outputs: { ctx: contextId }

  - name: node-1 sets namespace metadata
    type: set_group_metadata
    node: node-1
    group_id: "{{ns}}"
    name: "Acme Workspace"
    data: { topic: "company-wide", icon: "rocket" }

  - name: node-1 sets subgroup metadata
    type: set_group_metadata
    node: node-1
    group_id: "{{sub}}"
    name: "general"
    data: { topic: "everything", archived: "false" }

  - name: node-1 sets context metadata
    type: set_context_metadata
    node: node-1
    group_id: "{{sub}}"
    context_id: "{{ctx}}"
    name: "general-main"
    data: { kind: "primary" }

  - name: invite node-2 (recursive)
    type: create_namespace_invitation
    node: node-1
    namespace_id: "{{ns}}"
    recursive: true
    outputs: { invite: invitation }

  - name: node-2 joins
    type: join_namespace
    node: node-2
    invitation: "{{invite}}"
    outputs: { node2_id: memberPublicKey }

  - name: wait for node-2 to converge on the subgroup
    type: wait_for_sync
    group_id: "{{sub}}"
    timeout: 30
    trigger_sync: true

  - name: wait
    type: wait
    seconds: 3

  - name: node-2 reads namespace metadata
    type: get_group_metadata
    node: node-2
    group_id: "{{ns}}"
    outputs: { ns_meta: . }

  - name: assert namespace name
    type: json_assert
    json_equal:
      actual: "{{ns_meta.name}}"
      expected: "Acme Workspace"

  - name: assert namespace data.topic
    type: json_assert
    json_equal:
      actual: "{{ns_meta.data.topic}}"
      expected: "company-wide"

  - name: node-2 reads subgroup metadata
    type: get_group_metadata
    node: node-2
    group_id: "{{sub}}"
    outputs: { sub_meta: . }

  - name: assert subgroup name
    type: json_assert
    json_equal: { actual: "{{sub_meta.name}}", expected: "general" }

  - name: node-2 reads context metadata
    type: get_context_metadata
    node: node-2
    group_id: "{{sub}}"
    context_id: "{{ctx}}"
    outputs: { ctx_meta: . }

  - name: assert context name
    type: json_assert
    json_equal: { actual: "{{ctx_meta.name}}", expected: "general-main" }

  # node-2 has no caps -> rejected setting subgroup metadata
  - name: node-2 cannot set subgroup metadata without cap
    type: set_group_metadata
    node: node-2
    group_id: "{{sub}}"
    name: "hijack"
    data: {}
    expected_failure: true

  # node-1 grants CAN_MANAGE_METADATA (1<<8 = 256) at the namespace root
  - name: grant node-2 CAN_MANAGE_METADATA
    type: set_member_capabilities
    node: node-1
    group_id: "{{ns}}"
    member: "{{node2_id}}"
    capabilities: 256

  - name: wait for node-2 to see its caps
    type: wait_for_sync
    group_id: "{{ns}}"
    timeout: 30
    trigger_sync: true

  - name: wait
    type: wait
    seconds: 3

  - name: node-2 now sets subgroup metadata
    type: set_group_metadata
    node: node-2
    group_id: "{{sub}}"
    name: "renamed-by-node2"
    data: { topic: "everything", archived: "true" }

  - name: wait for node-1 to converge
    type: wait_for_sync
    group_id: "{{sub}}"
    timeout: 30
    trigger_sync: true

  - name: wait
    type: wait
    seconds: 3

  - name: node-1 reads node-2's update
    type: get_group_metadata
    node: node-1
    group_id: "{{sub}}"
    outputs: { sub_meta2: . }

  - name: assert renamed
    type: json_assert
    json_equal: { actual: "{{sub_meta2.name}}", expected: "renamed-by-node2" }

  # node-2 sets its OWN member metadata in the namespace, no cap needed
  # (works even if we hadn't granted the cap — granting it above doesn't gate this)
  - name: node-2 sets its own member metadata
    type: set_member_metadata
    node: node-2
    group_id: "{{ns}}"
    member: "{{node2_id}}"
    name: "node-two"
    data: { status: "online" }

  - name: wait for node-1 to converge
    type: wait_for_sync
    group_id: "{{ns}}"
    timeout: 30
    trigger_sync: true

  - name: wait
    type: wait
    seconds: 3

  - name: node-1 reads node-2's member metadata
    type: get_member_metadata
    node: node-1
    group_id: "{{ns}}"
    member: "{{node2_id}}"
    outputs: { m2_meta: . }

  - name: assert member name
    type: json_assert
    json_equal: { actual: "{{m2_meta.name}}", expected: "node-two" }
```

**Important:** before finalizing, confirm merobox actually has `set_group_metadata` / `get_group_metadata` / `set_member_metadata` / `get_member_metadata` / `set_context_metadata` / `get_context_metadata` step types. If it doesn't (likely — it had alias steps?), either (a) add them to merobox in a companion PR (like the earlier merobox#231→#232 groundwork) and gate this workflow on that, or (b) drive metadata via the generic `call`/admin-HTTP step against `POST .../metadata` and `GET .../metadata`. Decide during implementation; do **not** invent merobox step types that don't exist.

- [ ] **Step 2: Add the CI matrix entry**

In `.github/workflows/e2e-rust-apps.yml`, in the `test-workflows` job's matrix `include:` list, after the `group-subgroup-visibility-inheritance` entry, add:

```yaml
          - workflow: group-metadata
            file: workflows/group-metadata.yml
            app: scaffolding-e2e
```

(Match the exact key names the other entries use.) If the Edit is blocked by the workflow-edit security hook, retry the identical Edit.

- [ ] **Step 3: Validate the workflow file shape**

Run: `cd apps/scaffolding-e2e && python3 -c "import yaml,sys; yaml.safe_load(open('workflows/group-metadata.yml'))" && echo OK`
Expected: `OK` (valid YAML). Full e2e run happens in CI.

- [ ] **Step 4: Commit**

```bash
git add apps/scaffolding-e2e/workflows/group-metadata.yml .github/workflows/e2e-rust-apps.yml
git commit -m "test(e2e): group-metadata workflow + CI matrix entry"
```

### Task 4.2: Version bump

**Files:**
- Modify: `Cargo.toml` (`[workspace.metadata.workspaces] version`)

- [ ] **Step 1: Bump the rc number**

In `Cargo.toml`, bump `version = "0.10.1-rc.NN"` → next rc (`rc.(NN+1)`). Confirm there isn't a `cargo workspaces version` tooling step that should be used instead (`docs/RELEASE.md`) — if there is and it's trivial to run, use it; otherwise the manual bump is fine for an in-flight feature branch.

- [ ] **Step 2: Verify nothing else needs the bump**

Run: `git grep -n "0.10.1-rc" Cargo.toml crates/*/Cargo.toml`
Expected: only `Cargo.toml`'s `[workspace.metadata.workspaces]` carries the explicit version; member crates inherit via `version.workspace = true`. If a member crate pins it, bump there too.

- [ ] **Step 3: Commit**

```bash
git add Cargo.toml
git commit -m "chore: bump workspace rc version"
```

---

## Final verification (before opening the PR)

- [ ] `cargo build --workspace` — clean.
- [ ] `cargo test --workspace` — all pass.
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` — clean (the repo runs clippy in CI; match it).
- [ ] `git grep -wi alias -- 'crates/**'` — only the node-local `Alias<T>` system remains (`client/alias.rs`, `cli/context/alias.rs`, `/alias/...` routes, `--as`); no group-scoped alias left.
- [ ] `cargo test -p calimero-context-config metadata` — the type/cap tests pass.
- [ ] The `architecture/` capability tables show 9 bits; the storage-schema rows say `*Metadata → MetadataRecord`.
- [ ] PR description: notes the wire break (old clients emitting `*AliasSet` fail at op deserialization), the no-migration behavior change (existing alias rows become dead keys), and links #2332 for the design. Does **not** include the design doc itself (that stays on #2332, unmerged).

---

## Self-review notes

- **Spec coverage:** §2 data model → Task 1.1, 1.3; §2.1 state-hash exclusion → Task 2.2 (doc-comment + regression test `metadata_set_does_not_change_group_state_hash`); §2.2 no migration → not a task (deliberate no-op), called out in the PR description checklist; §3.1 new ops → Task 2.1, 2.2; §3.2 removed ops → Task 2.1, 2.2, 3.1, 3.2; §3.3 authz → Task 2.2 (+ tests); §3.4 capability bit → Task 1.2, 1.5, and the `set-caps` flag + `CheckAccess` bit + arch docs → Task 3.1 step 5/6 (the `set-caps` flag: meroctl's `group set-caps` builds a u32 from named flags — add `--can-manage-metadata`; the `CheckAccess` output bit is in `crates/server/.../check_access` or wherever caps are surfaced — add the bit); §4 group_store API → Task 1.4; §5 HTTP/client/CLI → Task 3.1; §6 testing → Tasks throughout + Task 4.1; §7 out of scope → not implemented (correct); §8 phasing → this plan's 4 phases.
- **`set-caps` flag / `CheckAccess` bit:** folded into Task 3.1 step 5 (meroctl) and step 6 (docs) — call them out explicitly when executing; if `group set-caps` flag plumbing is non-trivial, it warrants its own sub-step. Search `can-manage-visibility` / `CAN_MANAGE_VISIBILITY` in `crates/meroctl` and `crates/server` and mirror every site for `can-manage-metadata`.
- **Placeholders:** the e2e step *type names* (`set_group_metadata` etc.) are the one genuine unknown — flagged loudly in Task 4.1 with a concrete fallback (drive via `call`/admin-HTTP) and a note to verify merobox's vocabulary first, not guess.
- **Type consistency:** `MetadataRecord` field names (`name`, `data`, `updated_at`, `updated_by`) are used identically in 1.1, 1.4, 2.2, 3.1. The `GroupOp::*MetadataSet` variants carry `{ name, data }` (+ `member` / `context_id`) — *not* a `record:` — because the signer/timestamp are stamped by the applier (2.1, 2.2). The actor `Store*MetadataRequest` types carry `record: MetadataRecord` (already-formed, used on the direct-persist path) while `Set*MetadataRequest` carry `{ name, data, requester }` (the publish path) — this asymmetry mirrors the existing `Store*AliasRequest` vs `Set*AliasRequest` split.
