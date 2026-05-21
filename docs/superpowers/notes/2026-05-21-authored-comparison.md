# `AuthoredMap` vs `AuthoredVector` comparison

## Author-tracking mechanism — are they identical?

| Aspect | AuthoredMap | AuthoredVector | Identical? |
|---|---|---|---|
| Where owner sourced | `env::executor_id().into()` at `insert` (`authored_map.rs:136`) | `env::executor_id().into()` at `push` (`authored_vector.rs:125`) | ✅ |
| Stamp format | `StorageType::User { owner, signature_data: None }` written via `inner.insert_with_storage_type(k, v, storage_type, None)` (`authored_map.rs:137-145`) | `StorageType::User { owner, signature_data: None }` written via `inner.push_with_storage_type(value, storage_type)` (`authored_vector.rs:126-130`) | ✅ |
| Update gate | `if stored_owner != executor { return Err(StorageError::ActionNotAllowed("AuthoredMap::update: not entry owner")) }` (`authored_map.rs:163-167`) | `if stored_owner != executor { return Err(StorageError::ActionNotAllowed("AuthoredVector::update: not entry owner")) }` (`authored_vector.rs:146-150`) | ✅ |
| Remove / tombstone gate | `if stored_owner != executor { return Err(StorageError::ActionNotAllowed("AuthoredMap::remove: not entry owner")) }` then `inner.remove(k)` (physical delete) (`authored_map.rs:192-200`) | `tombstone` delegates to `update(index, V::default())` (`authored_vector.rs:166-171`), reusing the update gate; there is no physical delete primitive | ✅ (authorization gate is identical; effect differs — see below) |
| Merge behaviour | `Mergeable::merge` is a no-op (`authored_map.rs:290-292`); container-level merge dispatches via `CrdtType::UserStorage => Ok(incoming.to_vec())` in `merge.rs:260-261`; per-entry signature verification runs in `Interface::apply_action`'s `StorageType::User` arms (`interface.rs:629-820` upsert, `985-1050+` delete) | Identical: `Mergeable::merge` is a no-op (`authored_vector.rs:284-286`); same `CrdtType::UserStorage` byte path; same `apply_action` arms | ✅ |
| Owner-on-update preservation | `update` calls `inner.get_mut(k)` and mutates via `EntryMut`; the entry's `Element` metadata — including `StorageType::User { owner }` — is preserved verbatim (`authored_map.rs:173-178`, comment at `169-172`) | `update` calls `inner.update(index, value)` which itself uses `get_mut` (`vector.rs:215-219`); same in-place mutation, same metadata preservation (`authored_vector.rs:152-156`) | ✅ |

Notes on the partial-divergence row:
- The remove/tombstone *authorization* is identical: both gate on
  `stored_owner == env::executor_id()`. The *post-authorization effect*
  diverges because vectors must preserve index stability across concurrent
  pushes (the module doc-comment at `authored_vector.rs:7-8` explains the
  rationale). This is a **collection-shape** difference, not an
  author-tracking difference, so it belongs in the next table — but it's
  worth flagging here that the *gate code* on both sides is the same.

## Collection-shape surface — do the methods unify?

| Concept | AuthoredMap | AuthoredVector | Compatible? |
|---|---|---|---|
| Add new entry | `insert(K, V) -> Result<(), StoreError>`; rejects existing keys with `ActionNotAllowed` ("AuthoredMap::insert: key already exists", `authored_map.rs:129-146`) | `push(V) -> Result<usize, StoreError>`; returns the assigned slot index (`authored_vector.rs:124-131`) | ❌ |
| Address an entry | `&K` (typed key, `AsRef<[u8]> + PartialEq`) | `usize` (slot index returned by `push`) | ❌ |
| Update entry | `update(&K, V) -> Result<(), StoreError>` (`authored_map.rs:156-179`) | `update(usize, V) -> Result<(), StoreError>` (`authored_vector.rs:142-157`) | ❌ (same return shape, but the address argument type differs and there is no trait that would erase that) |
| Retract entry | `remove(&K) -> Result<Option<V>, StoreError>`; physical delete; `Ok(None)` on missing key (`authored_map.rs:187-200`) | `tombstone(usize) -> Result<(), StoreError> where V: Default`; logical retract that writes `V::default()` in place; out-of-bounds is `Err(InvalidData)` (`authored_vector.rs:166-171`) | ❌ |
| Iterate | `entries() -> Iterator<(K, V)>` (`authored_map.rs:235-237`) | `iter() -> Iterator<V>` (`authored_vector.rs:200-202`) | ❌ |
| Owner lookup | `owner_of(&K) -> Result<Option<PublicKey>, StoreError>` | `owner_of(usize) -> Result<Option<PublicKey>, StoreError>` | ❌ (same return shape, addressed-by argument differs) |
| Length | `len() -> Result<usize, StoreError>` | `len() -> Result<usize, StoreError>` (counts tombstoned slots) | ✅ shape, ❌ semantics |
| Read by address | `get(&K) -> Result<Option<V>, StoreError>` | `get(usize) -> Result<Option<V>, StoreError>` | ❌ (same shape, addressed-by differs) |
| Existence test | `contains(&K) -> Result<bool, StoreError>` | (no equivalent; would be `get(idx).map(|o| o.is_some())`) | ❌ |

The map↔vector shape mismatch is fundamental: there is no single
`Authored<C>::insert` signature that could simultaneously offer
"reject-if-exists with caller-supplied key K" *and* "auto-assign slot and
return usize". Likewise `remove` (physical) and `tombstone` (logical) have
deliberately different semantics that flow from the underlying merge
strategy (index stability for vectors vs key-keyed addressing for maps).

## Decision

**Path A (full `Authored<C>` wrapper)** requires: every author-tracking row
AND every collection-shape row to be ✅.

**Path B (shared author-helper module)** requires: every author-tracking
row ✅, but at least one collection-shape row ❌.

**Path C (wontfix)** requires: at least one author-tracking row ❌.

### Chosen path: **Path B**

**Justification**:

Every author-tracking row is ✅: both collections source the owner from
`env::executor_id()` (identical line shape, see rows 1-2), stamp the
identical `StorageType::User { owner, signature_data: None }` literal
(row 2), compare against `stored_owner` with the same gate-error idiom
(rows 3-4), preserve the original owner across in-place updates by going
through `get_mut`-style mutation paths (row 6), and merge through the
identical byte-level `CrdtType::UserStorage => Ok(incoming.to_vec())`
dispatch in `merge.rs:260-261` (row 5). The duplication across
`authored_map.rs:129-200` and `authored_vector.rs:124-171` is mechanical
— a few `executor_id`-fetching, stamp-building, gate-checking, and
metadata-reading snippets repeated verbatim with the addressing argument
swapped — and that duplication is the maintenance hazard #2309 is
worried about.

However, **every** collection-shape row is ❌ except `len`: the surfaces
diverge on addressing (`&K` vs `usize`), on add semantics
(reject-or-bail vs auto-slot-return), and on retraction model
(physical delete vs in-place tombstone with `V: Default`). A single
`Authored<C>` wrapper trait cannot unify these without either erasing
the type-safety of `&K` addressing (regression for AuthoredMap) or
inventing a synthetic slot id for AuthoredVector (regression for
AuthoredVector). The right factoring is to extract the *author-tracking
mechanics* — the four primitives `current_executor_stamp()`,
`extract_owner(metadata)`, `require_owner_eq(stored, executor, op_name)`,
and the no-op `Mergeable::merge` body with its safety comment — into a
shared module that both files consume, leaving the public method shapes
untouched.

### Implications for the rest of the plan

- **Files Task 4 will touch**:
  - New: `crates/storage/src/collections/authored.rs` (module-private
    helpers exposed via `pub(super)`).
  - Modified: `crates/storage/src/collections/authored_map.rs` — replace
    inline stamp construction (lines 136-141), inline owner-gate
    construction (lines 162-167, 192-197), and inline metadata
    extraction (`owner_of`, lines 222-229) with calls into the helper.
    Mergeable no-op body becomes a single helper-call. Tests untouched.
  - Modified: `crates/storage/src/collections/authored_vector.rs` —
    same substitutions on lines 124-131, 145-150, 185-194, 220-242
    (the `require_owner` private helper itself either becomes a thin
    wrapper or is hoisted into the shared module if the address type
    can be abstracted via `Fn(addr) -> Result<Option<Id>, _>`).
  - Modified: `crates/storage/src/collections.rs` (or wherever the
    `mod authored_map; mod authored_vector;` declarations live) — add
    `mod authored;` declaration. Verified via `grep` would confirm path.
  - **NOT touched**: any public API of `AuthoredMap` or `AuthoredVector`;
    `interface.rs`; `merge.rs`; `entities.rs`; any test file.

- **LOC delta expectations**: the helper module is ~40-60 LOC (four short
  helper fns + a `Mergeable` macro or trait-default body, plus their
  doc-comments lifting the existing inline comments). The two consuming
  files shrink by roughly the same amount in aggregate (each call site
  collapses from 5-10 lines to 1-2). Net delta should be roughly zero
  with a slight reduction; the value is in single-source-of-truth, not
  in line count.

- **New contract tests in `tests/crdt_contract.rs`?** Not required for
  Path B. The existing 11+9=20 tests in the per-file
  `#[cfg(test)] mod tests` blocks cover the behaviours that matter
  (insert/push stamps owner, update gate, remove/tombstone gate,
  iteration, owner_of lookup, missing-address handling). The helper
  module is `pub(super)` and exercised only through the existing public
  APIs, so the existing tests are the contract. If we *also* want a
  belt-and-braces lock on the helper invariants directly, a single
  module-private `#[cfg(test)]` block inside `authored.rs` with 2-3
  unit tests on the helpers would suffice; that's a Task-4 judgement
  call, not a prerequisite. The `tests/crdt_contract.rs` suite is
  shape-trait-flavoured (`CrdtMap`, `CrdtSequence`) and these
  collections deliberately don't participate (see the module doc-notes
  in both files), so adding to it would be off-pattern.
