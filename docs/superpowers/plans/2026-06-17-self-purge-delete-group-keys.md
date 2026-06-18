# Self-Purge Deletes Group Encryption Keys — Implementation Plan (Part 1 of disable-HA leave+purge)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the self-purge / namespace-leave cascade actually delete the AES group **encryption** keys (`GroupKeyEntry`) for the namespace root and every subgroup — closing a forward-secrecy gap where an evicted TEE replica keeps decryption keys.

**Architecture:** Today `delete_group_local_rows` (run per group by the purge cascade) deletes `GroupSigningKey` via `SigningKeysRepository::delete_all_for_group` but never deletes `GroupKeyEntry` (the raw 32-byte AES key managed by `GroupKeyring`). No code deletes group encryption keys anywhere. We add `GroupKeyring::delete_all_for_group` (mirroring the signing-keys pattern) and call it from `delete_group_local_rows`, so every purge path (`cascade_namespace_state` for namespace-root leave, `purge_subgroup_for_self` for subgroup leave) sweeps the AES keys automatically.

**Tech Stack:** Rust (toolchain 1.88.0 — fmt via `rustup run 1.88.0 cargo fmt`), Cargo workspace, `calimero-governance-store` + `calimero-context` crates, `calimero-store` key types, `cargo test`.

**Scope guardrails:**
- ONLY Part 1 (the core key-deletion). The sidecar trigger (Part 2, mero-tee) and mdma allowlist (Part 3) are out of scope.
- No AI attribution in commits.
- Spec: `docs/superpowers/specs/2026-06-16-disable-ha-leave-and-purge-design.md`.

**Design note (verified, no code needed):** the namespace purge deletes governance rows + signing keys + context-tree *index/edge* pointers, but does **not** separately delete the context state-CRDT / application data rows. Deleting the AES group keys **crypto-shreds** that data (ciphertext becomes unreadable), which is the intended forward-secrecy outcome. This plan does not add separate state-row deletion; the spec records this.

---

## File Structure

| File | Change | Responsibility |
|---|---|---|
| `crates/governance-store/src/group_keys.rs` | Modify | Add `GroupKeyring::delete_all_for_group(&self)` + unit test. |
| `crates/governance-store/src/local_state.rs` | Modify | Call `GroupKeyring::new(store, *group_id).delete_all_for_group()` inside `delete_group_local_rows` (add import). |
| `crates/context/src/self_purge.rs` | Modify (tests only) | Add a cascade test asserting AES keys are purged for root + subgroup. |

---

## Task 1: Add `GroupKeyring::delete_all_for_group`

Mirror `SigningKeysRepository::delete_all_for_group`. `GroupKeyEntry`, `GROUP_KEY_PREFIX`, and `collect_keys_with_prefix` are already imported in `group_keys.rs`; the enumeration pattern already exists in `load_current_key_record`.

**Files:**
- Modify: `crates/governance-store/src/group_keys.rs`
- Test: same file, new `#[cfg(test)]` module.

- [ ] **Step 1: Write the failing test**

Add at the end of `crates/governance-store/src/group_keys.rs`:

```rust
#[cfg(test)]
mod delete_tests {
    use super::*;
    use calimero_store::db::InMemoryDB;
    use std::sync::Arc;

    fn test_store() -> Store {
        Store::new(Arc::new(InMemoryDB::owned()))
    }

    #[test]
    fn delete_all_for_group_removes_all_keys_and_is_scoped() {
        let store = test_store();
        let gid = ContextGroupId::from([0x42u8; 32]);
        let ring = GroupKeyring::new(&store, gid);

        let id1 = ring.store_key(&[0x01u8; 32]).unwrap();
        let _id2 = ring.store_key(&[0x02u8; 32]).unwrap();
        assert!(ring.load_current_key().unwrap().is_some());
        assert!(ring.load_key_by_id(&id1).unwrap().is_some());

        // Seed a different group; it must survive the targeted delete.
        let other = ContextGroupId::from([0x99u8; 32]);
        let other_ring = GroupKeyring::new(&store, other);
        let _ = other_ring.store_key(&[0x03u8; 32]).unwrap();

        ring.delete_all_for_group().unwrap();

        assert!(
            ring.load_current_key().unwrap().is_none(),
            "all group encryption keys for the target group must be gone"
        );
        assert!(ring.load_key_by_id(&id1).unwrap().is_none());
        assert!(
            other_ring.load_current_key().unwrap().is_some(),
            "another group's keys must NOT be deleted"
        );

        // Idempotent: deleting again is a no-op.
        ring.delete_all_for_group().unwrap();
    }
}
```

- [ ] **Step 2: Run the test, verify it fails**

Run: `rustup run 1.88.0 cargo test -p calimero-governance-store delete_all_for_group_removes_all_keys -- --nocapture`
Expected: FAIL — `no method named delete_all_for_group found for struct GroupKeyring`.

- [ ] **Step 3: Implement the method**

In `crates/governance-store/src/group_keys.rs`, inside `impl<'a> GroupKeyring<'a>` (next to `load_current_key`), add:

```rust
/// Delete every stored group encryption key (`GroupKeyEntry`) for this
/// group. Used by the purge/leave cascade for forward-secrecy hygiene —
/// mirrors `SigningKeysRepository::delete_all_for_group`. Idempotent.
pub fn delete_all_for_group(&self) -> EyreResult<()> {
    let gid = self.group_id.to_bytes();
    let keys = collect_keys_with_prefix(
        self.store,
        GroupKeyEntry::new(gid, [0u8; 32]),
        GROUP_KEY_PREFIX,
        |k| k.group_id() == gid,
    )?;
    let mut handle = self.store.handle();
    for key in keys {
        handle.delete(&key)?;
    }
    Ok(())
}
```

(No new imports: `GroupKeyEntry`, `GROUP_KEY_PREFIX` come from the existing `use calimero_store::key::{GroupKeyEntry, GroupKeyValue, GROUP_KEY_PREFIX};`, and `collect_keys_with_prefix` from `use super::collect_keys_with_prefix;`. The seek-start is `GroupKeyEntry::new(gid, [0u8; 32])` — note the key_id tail is a raw `[u8; 32]`, so **no** `.into()`.)

- [ ] **Step 4: Run the test, verify it passes**

Run: `rustup run 1.88.0 cargo test -p calimero-governance-store delete_all_for_group_removes_all_keys`
Expected: PASS.

- [ ] **Step 5: Format + commit**

```bash
rustup run 1.88.0 cargo fmt -p calimero-governance-store
git add crates/governance-store/src/group_keys.rs
git commit -m "feat(governance-store): add GroupKeyring::delete_all_for_group"
```

---

## Task 2: Delete group encryption keys in `delete_group_local_rows`

Wire the new method into the per-group purge so it fires on every cascade path (namespace-root leave and subgroup leave).

**Files:**
- Modify: `crates/governance-store/src/local_state.rs`

- [ ] **Step 1: Add the import**

In `crates/governance-store/src/local_state.rs`, the `delete_group_local_rows` body uses repositories imported via `use crate::{...}` near the top of the file. Add `GroupKeyring` to that import group. Find the existing `use crate::{` block (it pulls in `MembershipRepository`, `CapabilitiesRepository`, `MetadataRepository`, `UpgradesRepository`, `UpgradeLadderRepository`, `SigningKeysRepository`, `DenyListRepository`, `MetaRepository`) and add `GroupKeyring` to it. If the imports are individual `use crate::X;` lines instead, add `use crate::GroupKeyring;` alongside them.

- [ ] **Step 2: Insert the deletion call**

In `delete_group_local_rows`, immediately after this existing line (currently `local_state.rs:480`):

```rust
    SigningKeysRepository::new(store).delete_all_for_group(group_id)?;
```

add:

```rust
    GroupKeyring::new(store, *group_id).delete_all_for_group()?;
```

(`GroupKeyring::new` takes `group_id` by value — `ContextGroupId` is `Copy` — hence `*group_id`. Same `?`/`EyreResult` propagation as the surrounding deletes, so an AES-delete failure is load-bearing exactly like the signing-key delete, keeping the cascade's existing retry-anchor semantics.)

- [ ] **Step 3: Build to verify it compiles**

Run: `rustup run 1.88.0 cargo build -p calimero-governance-store`
Expected: compiles clean. (Behavioral verification is Task 3's cascade test, which runs `delete_group_local_rows` via the real purge path.)

- [ ] **Step 4: Run existing crate tests for no regressions**

Run: `rustup run 1.88.0 cargo test -p calimero-governance-store`
Expected: PASS.

- [ ] **Step 5: Format + commit**

```bash
rustup run 1.88.0 cargo fmt -p calimero-governance-store
git add crates/governance-store/src/local_state.rs
git commit -m "feat(governance-store): purge group encryption keys in delete_group_local_rows"
```

---

## Task 3: Cascade test — AES keys purged for root + subgroup

Prove the end-to-end namespace purge deletes the AES group keys for the root **and** a nested subgroup. Self-contained test (controls all ids), placed alongside the existing `cascade_namespace_state_*` tests in `self_purge.rs`.

**Files:**
- Modify: `crates/context/src/self_purge.rs` (test module only)

- [ ] **Step 1: Add `GroupKeyring` to the test module imports**

In the `#[cfg(test)]` module of `crates/context/src/self_purge.rs`, the import line (around `self_purge.rs:1255`):

```rust
use calimero_governance_store::{
    MembershipRepository, MetaRepository, PendingSelfPurgeRepository, SigningKeysRepository,
};
```

change to add `GroupKeyring` and `NamespaceRepository` (NamespaceRepository is already in scope via `use super::*`, but import it explicitly here if the build complains):

```rust
use calimero_governance_store::{
    GroupKeyring, MembershipRepository, MetaRepository, PendingSelfPurgeRepository,
    SigningKeysRepository,
};
```

- [ ] **Step 2: Write the failing test**

Add this test to the same `#[cfg(test)]` module (it reuses the module's existing `empty_store()` and `make_meta()` helpers):

```rust
#[test]
fn cascade_namespace_state_purges_group_encryption_keys_root_and_subgroups() {
    // A namespace root + one nested subgroup, each holding an AES group
    // encryption key. A namespace-root cascade must delete BOTH — an
    // evicted TEE replica must not retain decryption keys (forward secrecy).
    let mut rng = OsRng;
    let store = empty_store();

    let ns_id = ContextGroupId::from([0x70u8; 32]);
    let sub_id = ContextGroupId::from([0x71u8; 32]);

    let self_sk = PrivateKey::random(&mut rng);
    let self_pk = self_sk.public_key();

    // Root group.
    MetaRepository::new(&store)
        .save(&ns_id, &make_meta(self_pk))
        .unwrap();
    MembershipRepository::new(&store)
        .add_member_with_keys(
            &ns_id,
            &self_pk,
            GroupMemberRole::Admin,
            Some([0xAA; 32]),
            Some([0xBB; 32]),
        )
        .unwrap();
    NamespaceRepository::new(&store)
        .store_identity(&ns_id, &self_pk, &[0xAA; 32], &[0xBB; 32])
        .unwrap();

    // Nested subgroup under the namespace root.
    MetaRepository::new(&store)
        .save(&sub_id, &make_meta(self_pk))
        .unwrap();
    MembershipRepository::new(&store)
        .add_member_with_keys(
            &sub_id,
            &self_pk,
            GroupMemberRole::ReadOnlyTee,
            Some([0xCC; 32]),
            Some([0xDD; 32]),
        )
        .unwrap();
    NamespaceRepository::new(&store).nest(&ns_id, &sub_id).unwrap();

    // Seed an AES group encryption key on BOTH groups.
    GroupKeyring::new(&store, ns_id).store_key(&[0x11u8; 32]).unwrap();
    GroupKeyring::new(&store, sub_id).store_key(&[0x22u8; 32]).unwrap();
    assert!(GroupKeyring::new(&store, ns_id)
        .load_current_key()
        .unwrap()
        .is_some());
    assert!(GroupKeyring::new(&store, sub_id)
        .load_current_key()
        .unwrap()
        .is_some());

    let result = cascade_namespace_state(&store, ns_id);
    assert!(
        result.purged_groups >= 2,
        "root + subgroup must be purged, got {}",
        result.purged_groups
    );

    assert!(
        GroupKeyring::new(&store, ns_id)
            .load_current_key()
            .unwrap()
            .is_none(),
        "root AES group encryption key MUST be purged"
    );
    assert!(
        GroupKeyring::new(&store, sub_id)
            .load_current_key()
            .unwrap()
            .is_none(),
        "subgroup AES group encryption key MUST be purged"
    );
}
```

- [ ] **Step 3: Run the test, verify it fails — then passes after Tasks 1+2**

Run: `rustup run 1.88.0 cargo test -p calimero-context cascade_namespace_state_purges_group_encryption_keys -- --nocapture`
Expected: With Tasks 1+2 already committed, this should **PASS**. If you are running Task 3 before Tasks 1+2 are in, it FAILS on the post-cascade `is_none()` assertions (keys survive). Confirm the assertions are real by temporarily reverting the Task 2 line if in doubt — then ensure it passes with Task 2 present.

> NOTE: confirm `NamespaceRepository::nest(&parent, &child)` and `MembershipRepository::add_member_with_keys(&gid, &pk, role, Some([u8;32]), Some([u8;32]))` signatures against the file (they match `seed_namespace_self_member` + the Phase 1 tests). If `nest` differs, use whatever the existing multi-group cascade test (`cascade_namespace_state_drops_multi_group_subtree`) uses to build a nested subgroup.

- [ ] **Step 4: Run the broader suites for no regressions**

Run: `rustup run 1.88.0 cargo test -p calimero-context -p calimero-governance-store`
Expected: PASS.

- [ ] **Step 5: Format + commit**

```bash
rustup run 1.88.0 cargo fmt -p calimero-context
git add crates/context/src/self_purge.rs
git commit -m "test(context): assert namespace purge deletes group encryption keys (root + subgroup)"
```

---

## Final verification

- [ ] **Workspace fmt + the affected crates**

```bash
rustup run 1.88.0 cargo fmt --all -- --check
rustup run 1.88.0 cargo test -p calimero-governance-store -p calimero-context
```
Expected: fmt clean (CI fmt uses the 1.88 rustfmt), both crates green including the three new/extended tests.

- [ ] **Scope self-check:** confirm only `group_keys.rs`, `local_state.rs`, and `self_purge.rs` (tests) changed; no public-API change to `calimero-server-primitives` / `calimero-tee-attestation`; no mero-tee/mdma files.
