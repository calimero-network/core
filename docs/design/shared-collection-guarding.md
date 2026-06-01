# Design — `SharedStorage<Collection>` guards the whole subtree (with rotation)

| | |
|---|---|
| **Status** | Proposed |
| **Date** | 2026-06-01 |
| **Owner** | Storage / Node sync |
| **Problem** | `SharedStorage<Collection>` compiles but only gates its own wrapper entity; the collection's child entries default to `StorageType::Public` and are world-writable. A wrapped collection looks protected and isn't. |
| **Decision** | Keep one type (`SharedStorage<T>`); make it propagate the writer domain into a collection value's children, **with rotation**. |
| **Reuses** | per-entity rotation log + `writers_at` causal resolution (ADR 0001 / #2266/#2267), `effective_writers` apply hook (`interface.rs`). |

> Illustrative code; not final signatures.

## 1. Goal

```rust
doc: SharedStorage<UnorderedMap<String, V>>
```
must mean: **every entry of the map (and nested descendants) is writable only by the current writer set, verified at merge** — and `rotate_writers` re-resolves who can write, convergently across nodes.

Today `SharedStorage` is a *guarded cell*: it stamps `Shared{writers}` on its own entity and stores `value: T` inline, so a scalar `T` (e.g. `LwwRegister`) is fully covered. But a collection `T`'s entries are **separate entities** created `Public` by `UnorderedMap::insert` (`unordered_map.rs:288`) — the wrapper never touches them. This closes that gap.

## 2. The machinery we build on (don't reinvent)

- **Rotation log, per entity.** `calimero_storage::rotation_log` stores a per-`entity_id` log of writer-set rotations. The node loads it via `load_rotation_log_direct(ctx, entity_id)` (`delta_store.rs:648`).
- **Causal resolution.** `rotation_log_reader::writers_at(log, causal_parents, happens_before)` returns the writer set as of an op's causal cut, implementing ADR 0001's full merge rule (reachable-latest, HLC tiebreak, signer-bytes tiebreak). **Pure, already tested.**
- **Apply hook.** `interface.rs` verification takes `effective_writers: Option<BTreeSet>`: `Some` = node pre-resolved via `writers_at`; `None` = fall back to the entity's inline `storage_type.writers` (snapshot path). Verification is `ed25519_verify(sig, writer.digest(), payload)` against that set.

The merge rule for "who can write" already exists and is correct. The only thing missing is **routing a collection's children to the right rotation log.**

## 3. Core idea: domain anchoring (not per-child writer copies)

Two ways to guard children:

- **Inline copy (rejected):** stamp every child `Shared{writers: <full set>}`. Rotation must re-stamp **all N children** → O(N) writes and N independent concurrent-rotation merges. Fragile, exactly the split-brain trap.
- **Anchor inheritance (chosen):** each child is stamped `Shared{ domain_anchor: Some(A), writers: <cache> }` where `A` = the `SharedStorage` entity's id. The **one** rotation log lives at `A`. At verification, a child with `domain_anchor = Some(A)` resolves its writers from **A's** log via `writers_at(A_log, child_op.parents, …)`. Rotation appends one entry to **A's** log — **O(1), children untouched.**

Anchor inheritance reuses `writers_at` verbatim (same ADR 0001 rule), so concurrent insert-vs-rotate and rotate-vs-rotate inherit the existing, tested semantics — no new merge math.

### Representation change
`StorageType::Shared` gains an optional anchor:
```rust
Shared {
    writers: BTreeSet<PublicKey>,   // the anchor's own set; on a child, a fallback cache
    domain_anchor: Option<Id>,      // None = this entity owns its domain (today's behaviour)
    signature_data: Option<SignatureData>,
}
```
- `domain_anchor: None` → unchanged from today (the `SharedStorage` cell, and the anchor entity itself).
- `domain_anchor: Some(A)` → "I'm a member of A's writer domain; resolve writers from A's rotation log."

Backward-compatible: existing Shared entities deserialize with `domain_anchor: None` (borsh field add → needs a versioned/`Option` default; see §6).

## 4. Propagation: how `SharedStorage<Collection>` stamps children

`SharedStorage<T>` is generic; it can't call "stamp your children" only when `T` is a collection without specialization. Reuse the **`TypeId` registry pattern from `rekey.rs`**: collections register a "apply writer domain" thunk in their constructor; `SharedStorage` calls `apply_domain_dyn::<T>(&mut value, anchor, writers)` which **no-ops for non-collection `T`** (scalars are inline in the anchor → already covered).

- A domained collection carries `(anchor, writers)` and passes `StorageType::Shared{ writers, domain_anchor: Some(anchor), .. }` into `insert_with_storage_type` instead of `Public`.
- **Recursion:** a nested collection value re-keyed under a domained parent inherits the same anchor (extend the existing `rekey` walk, which already descends nested collections relative to a parent id).

This is the same shape as the deterministic-ID and rekey registries — consistent, source-compatible, no trait bound on `T`.

## 5. Verification & rotation flow

**Write a child** (insert): child entity stamped `Shared{ writers: current, domain_anchor: Some(A) }`, signed by the executor (must be a current writer).

**Apply/merge a child delta:** node resolves `effective_writers`:
```
if child.storage_type.domain_anchor == Some(A):
    log = load_rotation_log(A)                      // anchor's log, not child's id
    effective = writers_at(log, child_op.parents, happens_before)
else:
    (today's path: child's own id / inline writers)
```
then `interface.rs` verifies the signature against `effective`. **One line of routing change** in the node's effective-writers resolution.

**Rotate:** `shared.rotate_writers(new)` (existing path) appends to **A's** log, signed by a current writer. Children are not rewritten; their next write resolves the new set automatically. Concurrent rotations converge by ADR 0001 (already implemented in `writers_at`).

## 6. Risks / must-handle

- **Wire format.** Adding `domain_anchor` to `StorageType::Shared` changes borsh layout → version the metadata or use a trailing `Option` with a default-on-missing decoder; verify ABI/snapshot compatibility.
- **Anchor not yet synced.** A child delta may arrive before the anchor's rotation log → reuse the existing orphan/queued-delta handling (interface.rs already stores orphaned children); resolution retries once the anchor lands.
- **Anchor id stability.** The anchor is the `SharedStorage` entity id, which is deterministic (field-name derived) — good. Children must capture it at insert.
- **Snapshot path** (`effective_writers: None`): child falls back to its inline `writers` cache — must be kept ~current, or snapshots resolve via the embedded rotation-log snapshot.

## 7. Phased plan

1. **P1 — representation + propagation (fixed writers).** Add `domain_anchor` to `StorageType::Shared` (default `None`, back-compat). Add the `apply_domain_dyn` registry; `UnorderedMap` (first) stamps children with the anchor. `SharedStorage<UnorderedMap>` sets the domain. **No rotation yet.** Tests: a child entry is `Shared`, and an **adversarial non-writer delta to a child is rejected at apply** (the proof this guards the subtree).
2. **P2 — rotation.** Route child verification to the anchor's rotation log in the node layer; wire `rotate_writers` on the domained collection. Convergence tests: concurrent insert-vs-rotate, rotate-vs-rotate (ADR 0001), across 2–3 nodes (merobox).
3. **P3 — all collections + nesting.** `UnorderedSet`/`Vector`/`RGA`/`Sorted*` register; nested collections inherit the anchor through the rekey walk. Map-of-maps test.
4. **P4 — sync/snapshot/back-compat hardening.** Snapshot leaf push, orphan-before-anchor, wire-version migration, ABI guard.

## 8. Test matrix (the part that prevents split-brain)

- **Adversarial:** non-writer-signed delta to any child → rejected on every node (P1).
- **Convergence:** concurrent `insert` (writer A) ∥ `rotate` (writer B) → all nodes converge, op evaluated at its cut (P2).
- **Rotate ∥ rotate:** ADR 0001 outcome identical across nodes (P2).
- **Nested:** map-of-maps, deep child guarded + converges (P3).
- **Snapshot / orphan-before-anchor:** child arriving before its anchor resolves correctly (P4).

## 9. Relationship to #2541

This *is* a slice of #2541's "collection-scope" capabilities, implemented concretely on the existing Shared machinery: a collection-level writer ACL evaluated at the causal cut and enforced at merge. It stays write-granular (all-or-nothing per writer); #2541's op-granularity (`Insert` vs `Delete`) layers on top later via the same anchor. Build it here so it composes with `calimero-components` (`Owned`/`AccessControl` become typed facades over a *genuinely* guarded collection), not as a parallel system.
