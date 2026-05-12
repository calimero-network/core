# Opaque-leaf sync in HashComparison — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking. Run `/code-review:code-review` on the PR after Task group A is committed and again after Task group B.

**Goal:** Make HashComparison sync reconcile no-`crdt_type` ("opaque") Merkle leaves — today the `Root<T>` app-state entry `Id::new([118;32])` — so two context members converge to the same Merkle root and `wait_for_sync` stops timing out; and stop minting new opaque entities by giving the `Root<T>` entry an LWW `crdt_type` on creation (no migration).

**Architecture:** Spec: `docs/superpowers/specs/2026-05-13-opaque-leaf-sync-design.md`. Two PRs in `core`: **PR A (Fix 2)** — opaque leaves are first-class in HashComparison: `get_local_tree_node` and `collect_leaves_recursive` emit a *leaf* `TreeNode` for a no-`crdt_type` entity (carrying its raw bytes + an LWW-equivalent wire `crdt_type`) instead of a malformed `internal` node with empty children that the peer's `is_valid()` rejects; the existing LWW apply path on the receiver handles it unchanged. **PR B (Fix 1)** — `Root::new_internal` creates its inner collection with `crdt_type = LwwRegister`, so new contexts' root entries go through the normal leaf path; existing entries rely on PR A. PR B depends on PR A being merged.

**Tech Stack:** Rust, `crates/node` (sync protocol), `crates/node/primitives` (wire types), `crates/storage` (Merkle index + collections), `borsh` (wire encoding), the `crates/node/tests/sync_*` integration harness.

**Branch:** `fix/opaque-leaf-sync` (already created off `master` @ `054a784f`).

---

## ⚠️ Gating investigation — do Task A0 FIRST; it picks the model

The wire type `calimero_node_primitives::sync::hash_comparison::LeafMetadata.crdt_type` is a **non-optional `CrdtType`** (`crates/node/primitives/src/sync/hash_comparison.rs:322`), while the storage-side `index.metadata.crdt_type` is `Option<CrdtType>` (`None` ⇒ "legacy data, LWW" — `crates/storage/src/interface.rs`, the `let Some(crdt_type) = &metadata.crdt_type else { /* LWW: incoming if newer */ }` branch). So a no-`crdt_type` entity can't be put into a wire `LeafMetadata` faithfully. The merge fallback shows `crdt_type == None` and `crdt_type == Some(LwwRegister { .. })` behave **identically** for merge (both: "incoming wins iff `updated_at >= existing`"). So:

- **Model S (synthetic LWW, no wire change — preferred if safe):** `get_local_tree_node` / `collect_leaves_recursive` emit a *leaf* `TreeNode` for a no-`crdt_type` entity with `LeafMetadata { crdt_type: CrdtType::LwwRegister { inner_type: "Opaque" }, hlc_timestamp: index.metadata.updated_at(), created_at: index.metadata.created_at(), .. }` and `value = Interface::find_by_id_raw(entity_id)`. The receiver's existing LWW path applies it; `is_valid()` passes (it's a structurally-valid leaf). **Safe iff the entity's Merkle `full_hash`/`own_hash` does NOT include `crdt_type`** — otherwise a receiver that re-derives a different `crdt_type` than the sender stored would compute a different `full_hash`, the next sync would see the hashes differ again, and it would never converge.
- **Model O (`Option<CrdtType>` on the wire — fallback if Model S is unsafe):** change `LeafMetadata.crdt_type` to `Option<CrdtType>` (borsh-layout change to `TreeLeafData` — version-skew sensitive; check whether the sync protocol has a version negotiation / whether master tolerates a one-time wire bump — mero-drive e2e uses `merod:edge` on both nodes so it's fine there). `None` ⇒ opaque; receiver applies via LWW and stores with `crdt_type: None` (matching the sender). `is_valid()` already only checks structural shape, no change.

**Task A0 decision (2026-05-13, branch `fix/opaque-leaf-sync`): → MODEL S. `metadata.crdt_type` is NOT an input to a leaf's Merkle hash, so the synthetic-LWW approach is safe and no wire-format change is needed.**

Hash-input audit (the `hash` field that `compare_tree_nodes` compares for a leaf `TreeNode` is `index.full_hash()` — `crates/node/src/sync/hash_comparison_protocol.rs:774`):

- A leaf entity's `own_hash` = `Sha256::digest(&to_vec(entity))` — the borsh-serialized **entity data bytes only** (`crates/storage/src/interface.rs:224` in `add_child_to`; same in `save_raw` `crates/storage/src/interface.rs:1494`, the merge path `crates/storage/src/interface.rs:1394`/`:1429`, and the placeholder path `crates/storage/src/interface.rs:681`). `Metadata` is `#[borsh(skip)]` on `Element` (`crates/storage/src/entities.rs:172-178`), so `crdt_type`, `created_at`, `updated_at`, `storage_type`, `field_name` are **not** in `own_hash`.
- `full_hash` = `calculate_full_hash_for_children(own_hash, children)` = `Sha256(own_hash ‖ child₀.merkle_hash ‖ child₁.merkle_hash ‖ …)` (`crates/storage/src/index.rs:376-390`). For a leaf (`children == None`) this is just `Sha256(own_hash)`. Still no metadata.
- The **only** way a `Metadata` field reaches any hash: a *parent's* children list is kept sorted by `ChildInfo`'s `Ord` = `created_at` then `id` (`crates/storage/src/entities.rs:106-118`, `crates/storage/src/index.rs:319-339`), and that order feeds the parent's `full_hash` (and ultimately the root hash). So `created_at` is hash-relevant — which is exactly why `LeafMetadata` carries `created_at` over the wire (the #2319 note at `crates/node/primitives/src/sync/hash_comparison.rs:326-340`). `updated_at` is **not** hashed (it's the LWW merge tiebreak only). `crdt_type` is **not** hashed at any level.

⇒ A receiver that stores the opaque entity with the synthetic `crdt_type: Some(CrdtType::LwwRegister { inner_type: "Opaque".to_string() })` while the sender stored `crdt_type: None` still computes the **same** `own_hash`/`full_hash`/root hash (identical data bytes, identical `created_at`), so HashComparison converges. And per `crates/storage/src/interface.rs`, `crdt_type == None` and `crdt_type == Some(LwwRegister { .. })` merge identically, so future merges are also unaffected. Model S is safe; **proceed with Model S** (the rest of the plan's Model-S arms). The Model-O arms can be ignored.

---

## File map

| File | Responsibility | PR | Change |
|---|---|---|---|
| `crates/storage/src/index.rs`, `crates/storage/src/entities.rs` | Merkle hash of an entity | A | read only (Task A0) |
| `crates/node/src/sync/hash_comparison_protocol.rs` | initiator: `get_local_tree_node`, `collect_leaves_recursive`, `push_local_subtrees`; responder: `handle_entity_push` route | A | modify |
| `crates/node/primitives/src/sync/hash_comparison.rs` | wire types `TreeNode`/`TreeLeafData`/`LeafMetadata`, `is_valid()` | A | modify only under **Model O** (make `crdt_type: Option<CrdtType>`); under Model S: no change beyond a unit test |
| `crates/node/src/sync/hash_comparison.rs` | responder `handle_entity_push` (the other copy) / `EntityPush` handling | A | verify; modify only if it special-cases `crdt_type` |
| `crates/node/src/sync/helpers.rs` | `handle_entity_push` shared impl | A | verify the LWW path applies an opaque/`LwwRegister` leaf; modify if it skips no-crdt entities |
| `crates/node/tests/sync_*` (`sync_protocols.rs` / `sync_sim/` / new file) | integration test: two-node opaque-divergence converges | A | add test |
| `crates/storage/src/collections/root.rs` | `Root::new_internal` | B | give the inner `Collection` an `LwwRegister` crdt_type |
| `crates/storage/src/collections.rs` (and `Collection::new_with_field_name_and_crdt_type` / siblings) | `Collection` constructors with crdt_type | B | read only — pick the right constructor |

---

## PR A — Fix 2: opaque leaves first-class in HashComparison

### Task A0: Decide the model (gating investigation)

**Files:** read `crates/storage/src/index.rs`, `crates/storage/src/entities.rs`, `crates/storage/src/element.rs` (or wherever `merkle_hash` / `Element::merkle_hash` lives).

- [ ] **Step 1:** `rg -n 'fn merkle_hash|fn own_hash|fn calculate_full_merkle_hash|crdt_type' crates/storage/src` and read every `merkle_hash`/`own_hash` definition. Determine: is `metadata.crdt_type` hashed into a leaf entity's `own_hash` (and therefore its `full_hash`, and therefore the `TreeNode.hash` `compare_tree_nodes` uses)?
- [ ] **Step 2:** Record the answer in this plan file (edit the "Gating investigation" section) and commit:
  ```bash
  git add docs/superpowers/plans/2026-05-13-opaque-leaf-sync.md
  git commit -m "docs: record opaque-leaf model decision (Model S/O) from hash audit"
  ```
- [ ] **Step 3:** If Model O, also `rg -n 'protocol.*version|VERSION|negotiat' crates/node/src/sync` to confirm whether a wire-format bump is tolerable; note it in the plan.

### Task A1: Failing integration test — two nodes diverge on an opaque entity, never converge

**Files:** `crates/node/tests/sync_protocols.rs` (or a new `crates/node/tests/sync_opaque_leaf.rs` mirroring an existing two-node HashComparison test — look at the existing `crates/node/tests/sync_*` files for the harness pattern; `crates/node/tests/sync_sim/scenarios/hash_comparison.rs` is a good template).

- [ ] **Step 1:** Write a test that: builds two in-memory storage states for the same context where node A has the `Root<T>` entry (`Id::new([118;32])`) with one value and node B has it with an older/absent value (a no-`crdt_type` leaf); runs a HashComparison sync A↔B; asserts (a) after sync, `Interface::find_by_id_raw(Id::new([118;32]))` matches on both, and (b) the two contexts' root `full_hash` are equal. Use the existing harness's "two states + run sync" helper. If the harness only supports CRDT-typed entities, extend it minimally to seed a no-`crdt_type` entity (or use `Root::new`/the storage API directly to create one).
- [ ] **Step 2:** Run it, confirm it FAILS today (roots don't converge / entity not present on B):
  ```bash
  cargo test -p calimero-node --test sync_protocols opaque -- --nocapture
  ```
  Expected: assertion failure on root-hash equality (or on the entity being present on B).
- [ ] **Step 3:** Commit the failing test:
  ```bash
  git add crates/node/tests/
  git commit -m "test(node/sync): failing test — opaque leaf diverges, HashComparison never converges"
  ```

### Task A2: `get_local_tree_node` emits a real leaf for a no-`crdt_type` entity

**Files:** `crates/node/src/sync/hash_comparison_protocol.rs` — the `else { /* No CRDT type */ ... return TreeNode::internal(...) }` block inside `get_local_tree_node` (~line 783).

- [ ] **Step 1:** Replace the `Some(crdt_type) = index.metadata.crdt_type.clone() else { warn!(..); return Ok(Some(TreeNode::internal(*entity_id.as_bytes(), full_hash, vec![]))) }` with:
  - **Model S:** build `LeafMetadata::new(CrdtType::LwwRegister { inner_type: "Opaque".to_string() }, index.metadata.updated_at(), [0u8; 32]).with_created_at(index.metadata.created_at())`, then `TreeLeafData::new(*entity_id.as_bytes(), entry_data, metadata)`, then `Ok(Some(TreeNode::leaf(*entity_id.as_bytes(), full_hash, leaf_data)))`. Keep a `trace!`/`debug!` noting "opaque leaf, synthesised LWW wire type". Drop the alarming `warn!`.
  - **Model O:** same but `LeafMetadata` carries `crdt_type: None`; `LeafMetadata::new` signature changes to take `Option<CrdtType>` (do that in Task A2b first).
- [ ] **Step 2:** `cargo build -p calimero-node` — expect compile error if `LeafMetadata::new` signature mismatches (Model O) or clean (Model S). Fix imports (`CrdtType` is already re-exported in scope).
- [ ] **Step 3 (Model O only — A2b):** in `crates/node/primitives/src/sync/hash_comparison.rs`, change `LeafMetadata.crdt_type: CrdtType` → `Option<CrdtType>`; `LeafMetadata::new(crdt_type: CrdtType, ..)` → `new(crdt_type: Option<CrdtType>, ..)`; update the existing call sites (`collect_leaves_recursive`, `get_local_tree_node` CRDT branch, the in-module tests). Update `is_valid()` doc comment to note a leaf may carry `crdt_type: None`. Run `cargo test -p calimero-node-primitives`.
- [ ] **Step 4:** Commit:
  ```bash
  git add crates/node/
  git commit -m "fix(node/sync): get_local_tree_node returns a leaf, not a malformed internal node, for opaque entities"
  ```

### Task A3: `collect_leaves_recursive` carries opaque leaves

**Files:** `crates/node/src/sync/hash_comparison_protocol.rs` — the `else { warn!(.., "leaf missing crdt_type, skipping") }` block in `collect_leaves_recursive` (~line 648).

- [ ] **Step 1:** Replace the `if let Some(ref crdt_type) = index.metadata.crdt_type { .. } else { warn!(..) }` so the `else` branch builds the same leaf as Task A2 (Model S: `LeafMetadata::new(CrdtType::LwwRegister { inner_type: "Opaque".into() }, index.metadata.updated_at(), [0u8;32]).with_created_at(index.metadata.created_at())`; Model O: `crdt_type: None`) and `leaves.push(leaf_data)`. So `push_local_subtrees` → `collect_local_leaves` now includes opaque leaves and `entities_pushed` reflects them.
- [ ] **Step 2:** `cargo build -p calimero-node`.
- [ ] **Step 3:** Commit:
  ```bash
  git add crates/node/src/sync/hash_comparison_protocol.rs
  git commit -m "fix(node/sync): collect_leaves_recursive carries opaque leaves for bidirectional push"
  ```

### Task A4: Verify the responder applies an opaque leaf via LWW (no change expected)

**Files:** `crates/node/src/sync/helpers.rs` (`handle_entity_push`), `crates/node/src/sync/hash_comparison_protocol.rs:496` and `crates/node/src/sync/hash_comparison.rs:178` (the two `InitPayload::EntityPush` handlers), and the merge call into `crates/storage` (`apply_leaf_with_crdt_merge` → the storage `else`/LWW path).

- [ ] **Step 1:** Read `handle_entity_push` and `apply_leaf_with_crdt_merge`. Confirm: a leaf whose wire `crdt_type` is `LwwRegister { .. }` (Model S) — or `None` (Model O) — is applied by overwriting iff incoming `updated_at >= existing` (or local absent), and counted in `applied_count`. The storage layer already does this (`interface.rs`: `is_lww` branch / `else`-LWW branch).
- [ ] **Step 2:** If (and only if) `handle_entity_push` or `apply_leaf_with_crdt_merge` *skips* a leaf with no/Opaque crdt_type (e.g. a `continue` on `crdt_type == None`), add the LWW path there. Otherwise no change.
- [ ] **Step 3 (only if changed):** Commit:
  ```bash
  git add crates/node/src/sync/
  git commit -m "fix(node/sync): EntityPush responder applies opaque leaves via raw LWW"
  ```

### Task A5: Make the failing test pass; add unit tests

**Files:** the test file from Task A1; `crates/node/src/sync/hash_comparison_protocol.rs` (a `#[cfg(test)]` unit test for `get_local_tree_node`); `crates/node/primitives/src/sync/hash_comparison.rs` (unit test that an opaque leaf node is `is_valid()`).

- [ ] **Step 1:** Run the Task A1 test — expect PASS now:
  ```bash
  cargo test -p calimero-node --test sync_protocols opaque -- --nocapture
  ```
- [ ] **Step 2:** Add a unit test `get_local_tree_node_returns_leaf_for_no_crdt_entity` — seed a no-`crdt_type` entity, call `get_local_tree_node`, assert the result `.is_leaf()` and `!.is_internal()` and `is_valid()`. (Model O: also assert `leaf_data.metadata.crdt_type == None`; Model S: assert it's `Some(LwwRegister { .. })`.)
- [ ] **Step 3:** Add `crates/node/primitives` unit test: a `TreeNode::leaf(..)` carrying `LeafMetadata` with the opaque/`None` crdt_type returns `is_valid() == true`.
- [ ] **Step 4:** Run the full sync test suite — expect no regressions:
  ```bash
  cargo test -p calimero-node --test sync_protocols
  cargo test -p calimero-node sync
  cargo test -p calimero-node-primitives
  ```
- [ ] **Step 5:** Commit:
  ```bash
  git add crates/node/
  git commit -m "test(node/sync): opaque leaf converges via HashComparison; unit tests for leaf emission + is_valid"
  ```

### Task A6: Audit this week's PRs for other Merkle-tree opaque entities; full build/clippy/test

- [ ] **Step 1:** `git log --since=2026-05-05 --oneline -- crates/context crates/storage | cat` then for each governance/namespace PR, check whether it writes a *Merkle-tree* entity (via `Interface`/`Element`/collections, `Id`-keyed) without a `crdt_type` (most write RocksDB via `store.handle().put(&calimero_store::key::*, ..)` — those are out of scope). If any Merkle-tree opaque entity is found, note it for PR B's list.
- [ ] **Step 2:** `cargo build --workspace && cargo clippy --workspace --all-targets -- -D warnings && cargo test -p calimero-node`.
- [ ] **Step 3:** Push the branch and open PR A; run `/code-review:code-review` on it; address findings.

---

## PR B — Fix 1 (option A): `Root<T>` entry gets an LWW `crdt_type` on creation

*(Branch off `master` after PR A merges, or stack on PR A. Depends on PR A for mixed-version safety.)*

### Task B1: `Root::new_internal` creates its inner collection with `crdt_type = LwwRegister`

**Files:** `crates/storage/src/collections/root.rs:1065` (`Root::new_internal`); `crates/storage/src/collections.rs` (find the `Collection` constructor that takes a `crdt_type` — `Collection::new_with_field_name_and_crdt_type` per `crates/storage/src/collections.rs:190`, or whatever the public path is — pick the one that lets us pass `Some(*ROOT_ID)` as the id *and* a `crdt_type`).

- [ ] **Step 1:** Read `crates/storage/src/collections.rs:190` (`new_with_field_name_and_crdt_type`) and `crates/storage/src/entities.rs:233`/`512` (`Element::new_with_field_name_and_crdt_type`) — confirm the API and what `field_name` / `crdt_type` arguments are expected.
- [ ] **Step 2:** Change `Root::new_internal`: instead of `let mut inner = Collection::new(Some(*ROOT_ID));`, construct the inner collection with `crdt_type = CrdtType::LwwRegister { inner_type: <T's name or a generic "RootValue"> }` (use whatever name convention the storage layer expects; `"RootValue"` is fine if there's no `type_name::<T>()` available without a bound). If the only crdt-type-aware `Collection` constructor needs a `field_name`, pass `"root"`.
- [ ] **Step 3:** `cargo build -p calimero-storage`. Fix any signature/import issues.

### Task B2: Tests — new `Root` carries `LwwRegister`; existing behaviour unchanged; group-state-hash unaffected

**Files:** `crates/storage/src/collections/root.rs` (`#[cfg(test)]`), `crates/storage/tests/*` if there's an existing root test; `crates/context/src/group_store/tests.rs` (sanity-run `compute_group_state_hash` related tests).

- [ ] **Step 1:** Unit test: `Root::new(|| MyVal::default())`, then read the index for `Id::new([118;32])`, assert `index.metadata.crdt_type == Some(CrdtType::LwwRegister { .. })`.
- [ ] **Step 2:** Run existing storage tests: `cargo test -p calimero-storage` — expect no regressions (`Root` read/write/commit, delta apply).
- [ ] **Step 3:** Run `cargo test -p calimero-context group_store` and confirm `compute_group_state_hash` / `groupStateHash` tests are unaffected (expected — that hash is over RocksDB state, not the WASM Merkle tree; but verify).
- [ ] **Step 4:** Commit:
  ```bash
  git add crates/storage/ crates/context/
  git commit -m "fix(storage): Root<T> entry gets an LwwRegister crdt_type on creation (#opaque-leaf-sync fix 1)"
  ```

### Task B3: Full build/clippy/test; PR; code-review

- [ ] **Step 1:** `cargo build --workspace && cargo clippy --workspace --all-targets -- -D warnings && cargo test -p calimero-storage -p calimero-context -p calimero-node`.
- [ ] **Step 2:** Push, open PR B (note "depends on PR A"), run `/code-review:code-review`, address findings.

---

## Self-review notes (against the spec)

- **Spec Fix 2 step 1 (model)** ↦ Task A0 + A2b (Model O) / Task A2 (Model S). The spec assumed Model O (`Option<CrdtType>`); the plan adds Model S as the preferred alternative because the wire `LeafMetadata` already discards `collection_id`/`version` (so it's clearly not the source of truth for stored metadata) and an `LwwRegister`-with-sentinel is merge-equivalent to `None` — *iff* `crdt_type` isn't in the leaf hash, which A0 confirms. If A0 says it *is* in the hash, fall to Model O exactly as the spec described.
- **Spec Fix 2 steps 2–3** ↦ Tasks A2, A3. **Step 4 (apply path)** ↦ Task A4 (verify; the storage LWW fallback already covers it, so likely no change). **Step 5 (tests)** ↦ Tasks A1, A5.
- **Spec Fix 1 (option A)** ↦ Tasks B1, B2. **Mixed-version safety** ↦ PR B stacked after PR A.
- **Spec open items 1/2/3** ↦ Tasks A4, A6 step 1, B2 step 3 respectively. **New open item (leaf hash)** ↦ Task A0.
- No placeholders: each task names exact files/functions/commands; the one genuine branch (Model S vs O) is resolved by A0 with both arms spelled out.
- Type consistency: `CrdtType::LwwRegister { inner_type: String }` used consistently (Model S); `Option<CrdtType>` on `LeafMetadata.crdt_type` consistently (Model O). `Id::new([118;32])` = `Root::<T>::entry_id()` used consistently for the opaque entity in tests.
