# Opaque-leaf sync in HashComparison — design

**Date:** 2026-05-13
**Status:** approved (brainstorming complete; next: implementation plan)

## Problem

A storage entity whose `index.metadata.crdt_type` is `None` — an **opaque leaf** — cannot be
reconciled by the HashComparison sync protocol. The most prominent such entity is the WASM app's
root-state value, `Id::new([118; 32])` = `Root::<T>::entry_id()` (`crates/storage/src/collections/root.rs`),
created via `Collection::new(Some(ROOT_ID))` with no `crdt_type`.

Concretely, in `crates/node/src/sync/hash_comparison_protocol.rs`:

1. `get_local_tree_node` — when the local Merkle leaf has no `crdt_type`, it logs
   `"leaf has no CRDT type, treating as opaque node"` and returns
   `TreeNode::internal(id, hash, /*children=*/[])` — i.e. an **internal node with empty children**.
2. `TreeNode::is_valid()` (`crates/node/primitives/src/sync/hash_comparison.rs`) requires internal
   nodes to have non-empty children, so that node is invalid → the receiving peer does
   `warn!("Invalid TreeNode, skipping"); continue;` → the entity is never compared and never pulled.
3. `collect_leaves_recursive` only collects entities with `Some(crdt_type)`
   (`"leaf missing crdt_type, skipping"`), so `push_local_subtrees` → `collect_local_leaves` returns
   zero leaves for an opaque-only subtree → `push_entities` is never called → `entities_pushed = 0`.
4. The code comment claims opaque leaves "sync via delta exchange, not EntityPush", but the
   delta-exchange phase carries only entities that are part of an exchanged DAG delta; an opaque
   entity present on one peer and not the other (e.g. a transient delta-lag divergence in the
   `Root<T>` value) is in no exchanged delta and is never carried.

**Net effect:** if an opaque leaf is present/different on one context member and not the other,
HashComparison can neither pull nor push it → the two members' Merkle root hashes stay divergent
forever → `wait_for_sync` (which checks root-hash equality across nodes) times out → e2e / fuzzy
tests fail with `Sync verification failed after 30s, N attempts`.

This is a latent bug: the opaque-leaf code paths date to `#2052` (bidirectional HashComparison) and
`#2096` (namespace identity model). Recent governance/namespace work (`#2298`, `#2325`, `#2326`,
`#2338`, `#2340`, `#2335`) is the *trigger* — it changed how the Registry context's app state evolves
in a way that now produces a divergence HashComparison can't close — not the cause.

Out of scope, noted for clarity: the group/member/context `MetadataRecord`s introduced by `#2338`
live in the node's RocksDB (`calimero_store`), replicate via the governance DAG / `SignedGroupOp`s,
and are *not* HashComparison-synced; `crdt_type` does not apply to them and they are unrelated to
this fix. Also out of scope: the always-empty-gossipsub-mesh issue (`#2293` / `#2336`) — separate.

## Goals

- A divergent opaque leaf reconciles via HashComparison, so two context members converge to the same
  Merkle root and `wait_for_sync` succeeds.
- Stop minting new opaque entities where a CRDT type is appropriate — specifically the `Root<T>`
  app-state entry — without requiring a data migration.
- No regression for existing on-disk data (entries already written with `crdt_type: None`).

## Non-goals

- No migration / backfill of `crdt_type` on existing entities (option A).
- No change to how RocksDB-side governance/namespace state replicates.
- No change to the gossipsub mesh behaviour.
- Not moving `MetadataRecord`s into the Merkle tree (that would be a separate spec).

## Design

### Fix 2 — opaque leaves are first-class in HashComparison *(unblocks `wait_for_sync`)*

Files: `crates/node/primitives/src/sync/hash_comparison.rs`,
`crates/node/src/sync/hash_comparison_protocol.rs`, `crates/node/src/sync/hash_comparison.rs`,
the EntityPush responder handler, and tests under `crates/node/tests/sync_*`.

1. **Model.** Make the leaf's CRDT type optional: `LeafMetadata.crdt_type: Option<CrdtType>`
   (currently non-optional, constructed via `LeafMetadata::new(crdt_type, updated_at, ..)`).
   `None` ⇒ "opaque leaf — reconciled by last-writer-wins on `updated_at`", which is exactly the
   storage layer's documented fallback for `crdt_type == None` (`crates/storage/src/interface.rs`,
   the `let Some(crdt_type) = &metadata.crdt_type else { /* LWW */ }` branch). Chosen over a
   dedicated `TreeLeafKind::{Crdt, Opaque}` enum because the rest of the protocol already keys off
   "is there a CRDT type or not" and `Option` is the minimal change.
   `TreeNode::is_valid()` already validates only the structural shape (internal ⇒ non-empty children;
   leaf ⇒ has `leaf_data`) and does not reference `crdt_type`, so a leaf node carrying `leaf_data`
   with `crdt_type: None` is already valid — nothing to relax there.

2. **`get_local_tree_node`.** For a Merkle leaf with no `crdt_type`, build and return a *leaf*
   `TreeNode` — `leaf_data` = the raw entry bytes (`Interface::find_by_id_raw(entity_id)`) +
   `LeafMetadata { crdt_type: None, updated_at: index.metadata.updated_at(), .. }` — instead of the
   current fake `TreeNode::internal(id, hash, [])`. This is the core fix: the malformed `internal`
   node is what the peer drops as "Invalid TreeNode".

3. **`collect_leaves_recursive`.** Include opaque leaves (with `crdt_type: None`) instead of
   skipping them, so `push_local_subtrees` / `collect_local_leaves` carries them and `entities_pushed`
   reflects them.

4. **Apply path** — two consumers of a remote leaf:
   - Tree-walk initiator, `apply_leaf_with_crdt_merge(context_id, leaf_data)`: a `crdt_type: None`
     leaf is applied through the storage layer's existing LWW-fallback path (compare incoming
     `updated_at` against the local entity's `updated_at`; overwrite if incoming is `>=` local or
     local is absent). It is likely this already works once the leaf actually *reaches* this code —
     verify during implementation; add an explicit route if not.
   - EntityPush responder (handles `InitPayload::EntityPush { entities }`, replies with
     `MessagePayload::EntityPushAck { applied_count }`): for a `crdt_type: None` entity, apply via
     the same raw-LWW path and count it in `applied_count`.

5. **Tests.**
   - Integration: two nodes, one side has a divergent opaque entity (or a divergent `Root<T>` value)
     → trigger HashComparison → assert both Merkle root hashes converge and the entity matches on
     both. (Use the existing `crates/node/tests/sync_*` harness; mirror an existing two-node test.)
   - Unit: `get_local_tree_node` returns a *leaf* (not `internal`) for a no-`crdt_type` entity;
     `TreeNode::is_valid()` accepts an opaque leaf; the EntityPush responder applies an opaque entity.

### Fix 1 (option A) — give the `Root<T>` Merkle entry an LWW `crdt_type` on creation

File: `crates/storage/src/collections/root.rs` (and any *other* Merkle-tree governance entity found
during implementation to lack a `crdt_type` — RocksDB-side state is out of scope).

- `Root::new_internal` currently does `Collection::new(Some(ROOT_ID))`, so its entry is written with
  `crdt_type: None`. Change it to create the inner collection with `crdt_type = CrdtType::LwwRegister`
  (via the existing `Collection::new_with_field_name_and_crdt_type`-style API). New contexts' root
  entries then carry `LwwRegister` and go through the normal leaf path; LWW-on-`updated_at` is the
  right semantics for the app-root value (the more-recently-written side is the more up-to-date one).
- **No migration.** Entries already on disk keep `crdt_type: None` and are reconciled by Fix 2.
- **Mixed-version safety.** A peer on the old build, syncing a context whose root was created by a
  new-build peer, receives a `crdt_type: LwwRegister` leaf and stores it with that type — so the type
  propagates; and either way Fix 2 covers the `None` case. So Fix 1 depends on Fix 1 having Fix 2
  already merged.

## Error handling

- Opaque-leaf apply: a stale incoming `updated_at` (older than local) is a no-op, not an error —
  consistent with LWW.
- A leaf `TreeNode` with `leaf_data: None` (no bytes) remains invalid — `is_valid()` unchanged for
  that case; the responder rejects it as today.
- EntityPush of an opaque entity that fails to deserialize / write: counted as not-applied (excluded
  from `applied_count`), logged at `warn`; the sync session continues (matches current behaviour for
  CRDT entities).

## Testing strategy

- Unit tests as listed under Fix 2 step 5.
- Integration test reproducing the bug: pre-fix, the two-node opaque-divergence test must fail with
  the roots not converging; post-fix it passes. This is the regression guard.
- Run the existing `crates/node/tests/sync_*` suite to confirm no regression in CRDT-typed sync.
- (Best-effort) re-run the mero-drive `e2e (main)` workflow against a `merod:edge` built from the
  Fix-2 branch to confirm `wait_for_sync` converges end-to-end.

## Open items to resolve during implementation (not design blockers)

1. Confirm `apply_leaf_with_crdt_merge` / the storage `else`-branch actually applies a `None`-crdt_type
   leaf as LWW once it reaches it (vs. needing a dedicated path).
2. Audit this week's namespace/subgroup/metadata PRs for any *Merkle-tree* entity created without a
   `crdt_type` (most are RocksDB); if found, add to Fix 1's list.
3. Confirm changing `Root`'s `crdt_type` does not perturb `compute_group_state_hash`
   (`crates/context/src/group_store/meta.rs`) — expected no, that hash is over RocksDB state, not the
   WASM Merkle tree — and re-check that the e2e `groupStateHash` assertions are unaffected.

## PR shape (both in `core`)

- **PR 1 = Fix 2** — standalone, fully testable, no migration. The `wait_for_sync` fix.
- **PR 2 = Fix 1 (option A)** — give the `Root<T>` entry an LWW `crdt_type` on creation. Depends on
  PR 1 being merged (mixed-version safety). Smaller.
