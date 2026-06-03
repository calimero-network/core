# Issue #2309 — `Authored<C>` Wrapper Audit & Decision Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use `superpowers:subagent-driven-development` (recommended) or `superpowers:executing-plans` to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Audit `AuthoredMap` (`crates/storage/src/collections/authored_map.rs`, 497 LOC) and `AuthoredVector` (`authored_vector.rs`, 431 LOC) to decide whether their per-entry author-tracking is sufficiently identical to collapse into a single generic `Authored<C: Mergeable>` wrapper, then execute the chosen outcome. Closes GitHub issue [#2309](https://github.com/calimero-network/core/issues/2309) under epic [#2301](https://github.com/calimero-network/core/issues/2301).

**Architecture:** This is a *conditional* PR — the audit is the load-bearing step. There are three possible outcomes the audit can lead to:

- **Path A — Full wrapper.** Author-tracking IS identical AND the collection-specific methods (insert vs. push, remove vs. tombstone) can be unified under a single generic surface. Replace both files with a single `Authored<C>` plus type aliases.
- **Path B — Shared helper module.** Author-tracking IS identical but collection-specific surfaces are genuinely different. Extract the duplicated stamping/gating logic into a shared module (`authored_common.rs` or similar); keep `AuthoredMap` and `AuthoredVector` as separate types that call into it. Saves real LOC but preserves the per-type API.
- **Path C — wontfix.** Author-tracking diverges semantically (different stamping rule, different conflict resolution, different ownership semantics). Document the divergence in both module doc comments, close #2309 as wontfix, and update the forward-references in PR #2435's PR-C doc-notes accordingly.

Per the audit prep I already did, the most likely outcome is **Path B** (identical stamping mechanism, divergent collection surfaces). Plan tasks accommodate all three with a single fork point after Task 3.

**Tech Stack:** Rust 1.x workspace toolchain, `cargo`, `cargo test`, `borsh`, GitHub CLI (`gh`).

**Scope of this plan:**
- Audit document committed to the repo (regardless of path).
- Implementation of whichever path the audit endorses.
- Updates to PR #2435's wrapper doc-notes if the outcome changes their framing.
- Update to issue #2309 with the decision and reasoning.

**Out of scope:**
- Touching `crate-level` consumers of `AuthoredMap` / `AuthoredVector` beyond fixing imports the unification requires.
- The physical extraction in #2310 — that's a separate PR.
- Any refactor of the underlying `StorageType::User { owner }` mechanism in the storage engine.

---

## Task 0: Worktree setup off PR #2435's branch

**Files:**
- No code changes; environment prep.

- [ ] **Step 1: Verify the core repo and PR-2435 branch are reachable**

Run:
```bash
git -C /Users/beast/Developer/Calimero/core fetch origin feat/2308-crdt-trait-hierarchy
```
Expected: fetch completes; the branch ref updates. If the branch doesn't exist on origin, stop and verify PR #2435 is still open.

- [ ] **Step 2: Create isolated worktree off PR-2435's branch**

```bash
git -C /Users/beast/Developer/Calimero/core worktree add \
    .worktrees/issue-2309-authored-audit \
    -b feat/2309-authored-wrapper-audit \
    origin/feat/2308-crdt-trait-hierarchy
cd /Users/beast/Developer/Calimero/core/.worktrees/issue-2309-authored-audit
```
Expected: new worktree at the listed path, branch `feat/2309-authored-wrapper-audit` based on PR-2435 (so the new `CrdtMap`/`CrdtSequence`/`CrdtSet` traits are already available). All subsequent paths in this plan are relative to this worktree root.

- [ ] **Step 3: Baseline-green check**

Run:
```bash
cargo test -p calimero-storage --lib --tests 2>&1 | tail -10
```
Expected: all green, including the PR-2435 contract tests. If anything is red on the PR-2435 baseline, **stop and report** — do not start audit on a broken base.

- [ ] **Step 4: Refresh memory on the contract test file**

Read `crates/storage/tests/crdt_contract.rs` once. The new `Authored*` decision may want to add contract tests here (Path A or B) using the existing `assert_crdt_laws` helper.

---

## Task 1: Audit AuthoredMap

**Files:**
- Read only: `crates/storage/src/collections/authored_map.rs` (497 LOC)
- No edits yet.

- [ ] **Step 1: Read the whole file end-to-end**

Open `crates/storage/src/collections/authored_map.rs` and read every line. Pay attention to:
- The struct definition (around line 51).
- Each `pub fn` body (read the actual logic, not just signatures).
- The `Mergeable` impl (if any).
- The `#[cfg(test)]` test module — what behaviours are pinned down?

- [ ] **Step 2: Record audit notes in a scratch file**

Create `docs/superpowers/notes/2026-05-21-authored-map-audit.md` (a working scratch file — not a permanent artifact; it'll be replaced by the comparison doc in Task 3) with the following structure, filled in from the read:

```markdown
# AuthoredMap audit notes

## Storage layout
- Inner collection: <e.g. UnorderedMap<K, _OwnedEntry<V>>>
- How owner is stored: <e.g. StorageType::User { owner: PublicKey } stamp on the entry>
- Key derivation: <e.g. K is the user-supplied key, no transformation>

## Method-by-method behaviour

| Method | Signature | Author-touching logic | Notes |
|---|---|---|---|
| `new` | `() -> Self` | none | |
| `new_with_field_name` | ... | none | |
| `insert` | `(K, V) -> Result<(), StoreError>` | reads env::executor_id() at line N; stamps owner | **Asymmetric: rejects existing keys** |
| `update` | `(&K, V) -> Result<(), StoreError>` | reads env::executor_id() at line N; checks against stored owner | gated |
| `remove` | `(&K) -> Result<Option<V>, StoreError>` | env::executor_id() vs stored owner at line N | gated |
| `get` | `(&K) -> Result<Option<V>, StoreError>` | none | unrestricted |
| `contains` | `(&K) -> Result<bool, StoreError>` | none | unrestricted |
| `owner_of` | `(&K) -> Result<Option<PublicKey>, StoreError>` | reads owner stamp | |
| `entries` | iter | none | |
| `len` | `() -> Result<usize, StoreError>` | none | |

## Mergeable impl
- Lines: ...
- Body: <one-line summary; likely a no-op delegating to UserStorage byte path>
- Why no-op (if so): <quote the relevant comment>

## Tests covered
- List the names of tests that exist in `#[cfg(test)] mod tests`. About 11 tests per the earlier grep.

## Surprises / non-obvious behaviour
- ...
```

Be precise — quote line numbers and copy-paste short code snippets where the behaviour is non-obvious.

- [ ] **Step 3: Commit the scratch file**

```bash
git add docs/superpowers/notes/2026-05-21-authored-map-audit.md
git commit -m "docs: authored_map audit notes (scratch — replaced in Task 3)

Refs #2309."
```

---

## Task 2: Audit AuthoredVector

**Files:**
- Read only: `crates/storage/src/collections/authored_vector.rs` (431 LOC)

- [ ] **Step 1: Read the whole file end-to-end**

Same approach as Task 1.

- [ ] **Step 2: Record audit notes**

Create `docs/superpowers/notes/2026-05-21-authored-vector-audit.md` with the same structure as Task 1's scratch:

```markdown
# AuthoredVector audit notes

## Storage layout
- Inner collection: <e.g. Vector<_OwnedSlot<V>>>
- How owner is stored: <copy from the file>
- Index derivation: <slots are append-only, indices monotonic; or whatever the file says>

## Method-by-method behaviour
| Method | Signature | Author-touching logic | Notes |
|---|---|---|---|
| `new` | `() -> Self` | none | |
| `new_with_field_name` | ... | none | |
| `push` | `(V) -> Result<usize, StoreError>` | env::executor_id() at line N → owner stamp | **Returns assigned slot** |
| `update` | `(usize, V) -> Result<(), StoreError>` | env::executor_id() vs stored owner | gated |
| `tombstone` | `(usize) -> Result<(), StoreError>` | env::executor_id() vs stored owner | gated; slot-preserving (NOT physical remove) |
| `get` | `(usize) -> Result<Option<V>, StoreError>` | none | unrestricted; returns None for tombstoned? Check. |
| `owner_of` | `(usize) -> Result<Option<PublicKey>, StoreError>` | reads owner | |
| `iter` | iter | none | iterates skipping tombstones? Verify. |
| `len` | `() -> Result<usize, StoreError>` | none | total or live? Verify. |

## Mergeable impl
- Lines, body, no-op reason.

## Tests covered
- List of tests.

## Surprises
- ...
```

- [ ] **Step 3: Commit**

```bash
git add docs/superpowers/notes/2026-05-21-authored-vector-audit.md
git commit -m "docs: authored_vector audit notes (scratch — replaced in Task 3)

Refs #2309."
```

---

## Task 3: Write comparison + decision document

**Files:**
- Create: `docs/superpowers/notes/2026-05-21-authored-comparison.md`
- (Delete the two scratch files from Tasks 1 & 2 once the comparison subsumes them.)

This task produces the authoritative artifact for the audit. It must end with a clear **Path A / B / C** decision.

- [ ] **Step 1: Side-by-side comparison table**

Create `docs/superpowers/notes/2026-05-21-authored-comparison.md` with the following structure:

```markdown
# `AuthoredMap` vs `AuthoredVector` comparison

## Author-tracking mechanism — are they identical?

| Aspect | AuthoredMap | AuthoredVector | Identical? |
|---|---|---|---|
| Where owner comes from | `env::executor_id()` at insert | `env::executor_id()` at push | ✅ |
| Stamp format | `StorageType::User { owner: PublicKey }` | same | ✅ |
| Update gate | `executor == stored_owner` else Err | same | ✅ |
| Remove gate | `executor == stored_owner` else Err | applies to `tombstone` | ✅ structurally |
| Merge behaviour | <copy from notes; likely no-op delegating to UserStorage byte path> | <same> | ✅/❌ |
| Owner-on-update | <preserves original owner or rewrites?> | <same?> | check |

If every row is ✅, the author-tracking mechanism unifies cleanly.

## Collection-shape surface — do the methods unify?

| Method shape | AuthoredMap | AuthoredVector | Compatible? |
|---|---|---|---|
| Add new entry | `insert(K, V) -> Result<(), _>` rejects existing | `push(V) -> Result<usize, _>` returns slot | ❌ different signatures |
| Mutate entry | `update(&K, V) -> Result<(), _>` | `update(usize, V) -> Result<(), _>` | ✅ shape |
| Remove entry | `remove(&K) -> Result<Option<V>, _>` | `tombstone(usize) -> Result<(), _>` slot-preserving | ❌ different semantics |
| Read entry | `get(&K) -> Result<Option<V>, _>` | `get(usize) -> Result<Option<V>, _>` | ✅ shape |
| Membership | `contains(&K) -> Result<bool, _>` | <no equivalent> | ❌ |
| Owner lookup | `owner_of(&K) -> Result<Option<PublicKey>, _>` | `owner_of(usize) -> Result<Option<PublicKey>, _>` | ✅ shape |
| Iterate | `entries() -> impl Iterator<(K, V)>` | `iter() -> impl Iterator<V>` | ✅ shape; different yield type |
| Length | `len() -> Result<usize, _>` | `len() -> Result<usize, _>` | ✅ |

## Decision

**Path A criteria** (full `Authored<C>` wrapper): every row in BOTH tables is ✅. The wrapper has a single `add_entry`/`mutate_entry`/`remove_entry` shape that's identical across map and sequence.

**Path B criteria** (shared author-helper module): all rows in the author-tracking table are ✅, but at least one row in the collection-shape table is ❌. The duplicated logic is the author stamping + gating; the diverging logic is per-collection storage.

**Path C criteria** (wontfix): at least one row in the author-tracking table is ❌. The two collections handle ownership *differently*, not just store it differently.

### My decision

Based on the audit above, the chosen path is **Path A / Path B / Path C**.

**Justification** (3-5 sentences): <explain why this path was chosen, citing specific rows from the tables>.

### What this means for the rest of the plan
- If Path A: skip to Task 4A (full wrapper).
- If Path B: skip to Task 4B (shared helper module).
- If Path C: skip to Task 4C (document wontfix).
```

- [ ] **Step 2: Delete the two scratch files**

```bash
git rm docs/superpowers/notes/2026-05-21-authored-map-audit.md \
       docs/superpowers/notes/2026-05-21-authored-vector-audit.md
```

- [ ] **Step 3: Commit the comparison + scratch deletion**

```bash
git add docs/superpowers/notes/2026-05-21-authored-comparison.md
git commit -m "docs: AuthoredMap vs AuthoredVector audit + decision

Decision: <Path A / B / C> — <one-line justification>.

Refs #2309."
```

- [ ] **Step 4: Pick exactly one of Tasks 4A, 4B, or 4C based on the decision**

The remaining tasks fork. Do **not** execute multiple paths.

---

## Task 4A — Full `Authored<C>` wrapper (ONLY if Path A from Task 3)

**Files:**
- Modify: `crates/storage/src/collections/authored_map.rs` (rewrite as type alias)
- Modify: `crates/storage/src/collections/authored_vector.rs` (rewrite as type alias)
- Create: `crates/storage/src/collections/authored.rs` (new generic wrapper)
- Modify: `crates/storage/src/collections.rs` (add `pub mod authored;` and re-exports)
- Modify: `crates/storage/tests/crdt_contract.rs` (add `Authored<T>` contract tests)

Skip this task if the audit chose Path B or C.

### Step 1: Write the failing wrapper contract test

Append to `crates/storage/tests/crdt_contract.rs`:

```rust
#[test]
fn authored_map_via_wrapper_alias_satisfies_crdt_laws() {
    use calimero_storage::collections::{AuthoredMap, Counter};
    use calimero_storage::env;
    use calimero_storage::store::MainStorage;

    env::reset_for_testing();

    let make = |executor: [u8; 32], private_key: &'static str, shared_count: usize| {
        move || {
            env::set_executor_id(executor);
            let mut m: AuthoredMap<String, Counter, MainStorage> = AuthoredMap::new();

            // Shared key — every replica writes to it under its own actor.
            let mut shared = Counter::<false, MainStorage>::new();
            for _ in 0..shared_count {
                shared.increment().unwrap();
            }
            m.insert("shared".to_owned(), shared).unwrap();

            // Private key — only this replica writes.
            let mut priv_c = Counter::<false, MainStorage>::new();
            priv_c.increment().unwrap();
            m.insert(private_key.to_owned(), priv_c).unwrap();
            m
        }
    };

    let eq = |a: &AuthoredMap<String, Counter, MainStorage>,
              b: &AuthoredMap<String, Counter, MainStorage>| {
        let mut a_entries: Vec<(String, u64)> = a
            .entries()
            .unwrap()
            .map(|(k, v)| (k, v.value().unwrap()))
            .collect();
        let mut b_entries: Vec<(String, u64)> = b
            .entries()
            .unwrap()
            .map(|(k, v)| (k, v.value().unwrap()))
            .collect();
        a_entries.sort();
        b_entries.sort();
        a_entries == b_entries
    };

    assert_crdt_laws(
        make([11; 32], "alice", 2),
        make([22; 32], "bob", 3),
        make([33; 32], "carol", 5),
        eq,
    );
}
```

This test compiles only after `AuthoredMap` is re-aliased to `Authored<UnorderedMap<K, V>>`. The signature must remain backward-compatible with the existing tests in `authored_map.rs`.

### Step 2: Run the test — expect compile failure

Run: `cargo test -p calimero-storage --test crdt_contract authored_map_via_wrapper_alias 2>&1 | tail -10`
Expected: PASS *or* compile error if `AuthoredMap` doesn't yet exist as the alias form.

(Note: if `AuthoredMap` is still a struct, this test will compile and pass because the surface is the same. The real check is in Step 5 below — the wrapper file must exist.)

### Step 3: Write the `Authored<C>` wrapper

Create `crates/storage/src/collections/authored.rs`:

```rust
//! Generic per-entry author-tracking wrapper around any `Mergeable` collection.
//!
//! `Authored<C>` adds owner-stamping (`StorageType::User { owner }`) at write time
//! and owner-gated mutation to whatever inner collection `C` provides. The author
//! is read from `env::executor_id()` at the moment of the write.
//!
//! Type aliases preserve the legacy `AuthoredMap` / `AuthoredVector` names for
//! one release cycle; `#[deprecated]` will land in a follow-up PR.

use borsh::{BorshDeserialize, BorshSerialize};
use calimero_primitives::identity::PublicKey;

use super::crdt_meta::{MergeError, Mergeable};
use super::error::StoreError;
use crate::address::StorageType;
use crate::env;

/// A `C` whose entries each carry a `StorageType::User { owner }` stamp.
///
/// The wrapper itself doesn't store an owner — it stamps each entry on write
/// and verifies the executor matches the stored stamp on mutation. The inner
/// `C` provides the storage shape (map, sequence, etc).
#[derive(Debug, BorshSerialize, BorshDeserialize)]
pub struct Authored<C> {
    pub(crate) inner: C,
}

impl<C: Mergeable> Mergeable for Authored<C> {
    fn merge(&mut self, other: &Self) -> Result<(), MergeError> {
        // Per-entry merge runs through the UserStorage byte path in `merge.rs`;
        // delegate to the inner collection's no-op `merge` for the
        // collection-level metadata (length, root id).
        self.inner.merge(&other.inner)
    }
}

// CrdtMeta + Decomposable impls match whichever they were on AuthoredMap /
// AuthoredVector before. Copy the implementations verbatim from those files,
// adjusting `Self` references.

impl<C> Authored<C> {
    /// Construct from an existing inner collection.
    pub fn from_inner(inner: C) -> Self {
        Self { inner }
    }

    /// Borrow the current executor as a `PublicKey` — the value that gets
    /// stamped onto every new entry.
    pub(crate) fn current_executor() -> PublicKey {
        env::executor_id().into()
    }

    /// Return the bare User-storage stamp for the current executor. Used by
    /// the map and sequence impl-specific add methods.
    pub(crate) fn make_owner_stamp() -> StorageType {
        StorageType::User {
            owner: Self::current_executor(),
        }
    }
}
```

### Step 4: Re-wire `authored_map.rs` as a thin shim

Replace the contents of `crates/storage/src/collections/authored_map.rs` with the existing module-level doc (the `# CRDT trait surface` block already explains why we don't implement `CrdtMap`) plus:

```rust
use super::authored::Authored;
use super::UnorderedMap;
use crate::store::{MainStorage, StorageAdaptor};

/// `AuthoredMap<K, V, S>` — alias preserving the legacy name.
///
/// Implemented as `Authored<UnorderedMap<K, OwnedEntry<V>, S>>`. The pre-existing
/// `pub fn insert / update / remove / …` surface is provided by the impl block
/// below; the actual author-stamping logic lives in `Authored`.
pub type AuthoredMap<K, V, S = MainStorage> = Authored<UnorderedMap<K, _AuthoredEntry<V>, S>>;

// _AuthoredEntry, the impl block with insert/update/remove/get/owner_of/entries,
// and the existing tests all live below this point — body preserved verbatim from
// the original file. Methods now delegate to `self.inner` (the underlying
// UnorderedMap) and use `Authored::current_executor()` / `make_owner_stamp()`
// for the author bits.
```

The point is to keep external callers (`use calimero_storage::collections::AuthoredMap`) working unchanged, while collapsing the duplicated author logic.

### Step 5: Re-wire `authored_vector.rs` analogously

Same approach for `AuthoredVector`: alias to `Authored<Vector<_OwnedSlot<V>, S>>`, preserve the existing `pub fn push / update / tombstone / get / iter / owner_of / len` surface as inherent methods that delegate to the inner Vector and call the shared author helpers.

### Step 6: Add `pub mod authored;` and re-export

Edit `crates/storage/src/collections.rs` to add `pub mod authored;` near the other mod declarations and add `Authored` to the public re-export list.

### Step 7: Run the full test suite

Run: `cargo test -p calimero-storage --lib --tests 2>&1 | tail -15`
Expected: all green, including the existing 11 `AuthoredMap` tests + 9 `AuthoredVector` tests + the new contract test.

### Step 8: Update PR-2435's doc-notes to reflect that `Authored<C>` now exists

Edit `crates/storage/src/collections/authored_map.rs` module-level doc — the `# CRDT trait surface` block now reflects history rather than intent. Reword the "Authored<C> wrapper exploration in issue #2309" sentence to "the generic Authored<C> wrapper introduced in #2309 — see `authored.rs`."

Same edit in `authored_vector.rs`.

### Step 9: Commit (Path A complete)

```bash
git add crates/storage/src/collections/authored.rs \
        crates/storage/src/collections/authored_map.rs \
        crates/storage/src/collections/authored_vector.rs \
        crates/storage/src/collections.rs \
        crates/storage/tests/crdt_contract.rs
git commit -m "feat(storage): Authored<C> wrapper unifies AuthoredMap / AuthoredVector

AuthoredMap is now a type alias for Authored<UnorderedMap<...>>;
AuthoredVector is Authored<Vector<...>>. Legacy method surfaces preserved
as inherent impls so external callers don't need to change. The duplicated
owner-stamping and owner-gating logic now lives once in authored.rs.

Net LOC: ~<measured delta> lines.

Closes #2309. Refs #2308.
"
```

---

## Task 4B — Shared helper module (ONLY if Path B from Task 3)

**Files:**
- Create: `crates/storage/src/collections/authored_common.rs` (shared stamping/gating helpers)
- Modify: `crates/storage/src/collections/authored_map.rs` (replace inline author logic with calls into the helper)
- Modify: `crates/storage/src/collections/authored_vector.rs` (same)

Skip this task if the audit chose Path A or C.

### Step 1: Write a failing helper-coverage test

Append to `crates/storage/src/collections/authored_common.rs` (in the new `#[cfg(test)] mod tests` block):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use calimero_primitives::identity::PublicKey;
    use crate::env;

    #[test]
    fn current_executor_returns_env_executor_id() {
        env::reset_for_testing();
        env::set_executor_id([42; 32]);
        let expected: PublicKey = [42; 32].into();
        assert_eq!(current_executor(), expected);
    }

    #[test]
    fn owner_check_accepts_matching_executor() {
        env::reset_for_testing();
        env::set_executor_id([42; 32]);
        let owner: PublicKey = [42; 32].into();
        assert!(executor_matches_owner(&owner));
    }

    #[test]
    fn owner_check_rejects_mismatched_executor() {
        env::reset_for_testing();
        env::set_executor_id([42; 32]);
        let owner: PublicKey = [99; 32].into();
        assert!(!executor_matches_owner(&owner));
    }
}
```

### Step 2: Run — expect compile failure

Run: `cargo test -p calimero-storage --lib collections::authored_common 2>&1 | tail -10`
Expected: FAIL — `unresolved module 'authored_common'`.

### Step 3: Create the helper module

Create `crates/storage/src/collections/authored_common.rs`:

```rust
//! Shared author-tracking primitives for [`AuthoredMap`](super::authored_map::AuthoredMap)
//! and [`AuthoredVector`](super::authored_vector::AuthoredVector).
//!
//! Both collections stamp each entry with the current executor identity at
//! write time and reject non-owner mutations. The signatures of the
//! collection-specific methods differ enough (map vs sequence) that a single
//! generic wrapper would still need bespoke impl blocks per shape (see the
//! `2026-05-21-authored-comparison.md` decision doc for why we chose this
//! shared-helper approach instead).
//!
//! This module owns the **identical** part: how the owner is sourced, how
//! the stamp is constructed, and how the owner-gate check decides accept vs
//! reject.

use calimero_primitives::identity::PublicKey;

use crate::address::StorageType;
use crate::env;

/// Return the current executor as a `PublicKey` — the value that gets
/// stamped onto every new author-tracked entry.
pub(super) fn current_executor() -> PublicKey {
    env::executor_id().into()
}

/// Build the `StorageType::User { owner }` stamp for the current executor.
/// Called by `AuthoredMap::insert` and `AuthoredVector::push`.
pub(super) fn make_owner_stamp() -> StorageType {
    StorageType::User {
        owner: current_executor(),
    }
}

/// Predicate: the current executor matches `owner`.
/// Called by every gated mutation (`update`, `remove`, `tombstone`).
pub(super) fn executor_matches_owner(owner: &PublicKey) -> bool {
    &current_executor() == owner
}
```

### Step 4: Add `mod authored_common;` to the collections module

Edit `crates/storage/src/collections.rs`:

Before (the relevant line near the other `mod` declarations):
```rust
mod authored_map;
mod authored_vector;
```

After:
```rust
mod authored_common;
mod authored_map;
mod authored_vector;
```

Do NOT add `pub use` for `authored_common` — it's `pub(super)` only.

### Step 5: Re-run the helper tests

Run: `cargo test -p calimero-storage --lib collections::authored_common 2>&1 | tail -10`
Expected: 3 passing tests.

### Step 6: Replace inline author logic in `authored_map.rs`

In `crates/storage/src/collections/authored_map.rs`:

Identify the three sites that read `env::executor_id()`:
- `insert` body around line 136 (owner stamping).
- `update` body around line 162 (owner-gate check).
- `remove` body around line 192 (owner-gate check).

Replace each with a call into `authored_common`:

```rust
// in insert body, around the existing owner stamp:
use super::authored_common::{make_owner_stamp};
let storage_type = make_owner_stamp();

// in update body:
use super::authored_common::executor_matches_owner;
// the existing block that constructs `executor` and compares against owner:
//   let executor: PublicKey = env::executor_id().into();
//   if executor != stored_owner { return Err(StoreError::...) }
// becomes:
if !executor_matches_owner(&stored_owner) {
    return Err(StoreError::...);
}

// in remove body: same pattern.
```

(Read the exact existing structure before editing — the surrounding match arms and error variants matter and must be preserved.)

### Step 7: Replace inline author logic in `authored_vector.rs`

Same approach: find the three sites (`push`, `update`, `tombstone`) and replace them with `make_owner_stamp` / `executor_matches_owner` calls.

### Step 8: Run the full test suite

Run: `cargo test -p calimero-storage --lib --tests 2>&1 | tail -15`
Expected: all green, including the existing 11 + 9 AuthoredMap/AuthoredVector tests.

### Step 9: Measure LOC delta

Run:
```bash
wc -l crates/storage/src/collections/authored_map.rs \
      crates/storage/src/collections/authored_vector.rs \
      crates/storage/src/collections/authored_common.rs
```
Expected: `authored_common.rs` ~40-50 LOC; each of `authored_map.rs` / `authored_vector.rs` shrinks by ~15-25 LOC each (the de-duplicated stamping/gating bodies). Record the actual numbers for the commit message.

### Step 10: Update PR-2435's doc-notes (optional polish)

The existing module doc-notes in `authored_map.rs` and `authored_vector.rs` say "Implicit-author hazard. The shape-trait surface takes no `author` parameter; a `CrdtMap` impl would have to read the executor id from ambient `env` state." That framing is still correct — the shared helper doesn't change the trait-fit decision. Leave the doc-notes unchanged unless they actively claim things that are now wrong.

### Step 11: Commit (Path B complete)

```bash
git add crates/storage/src/collections/authored_common.rs \
        crates/storage/src/collections/authored_map.rs \
        crates/storage/src/collections/authored_vector.rs \
        crates/storage/src/collections.rs
git commit -m "refactor(storage): extract shared author-stamping into authored_common

AuthoredMap and AuthoredVector share an identical author-tracking mechanism
(env::executor_id → StorageType::User { owner }, plus a matching owner-gate
on update/remove). Their collection-shape surfaces diverge — see the
2026-05-21-authored-comparison.md decision doc — so a full Authored<C>
wrapper would still need per-shape impl blocks and wouldn't save much LOC.
Extracting just the duplicated stamping/gating logic into authored_common
is the cleanest win.

Net LOC: <actual delta>.

Closes #2309. Refs #2308.
"
```

---

## Task 4C — Document wontfix (ONLY if Path C from Task 3)

**Files:**
- Modify: `crates/storage/src/collections/authored_map.rs` (extend the existing module doc-note)
- Modify: `crates/storage/src/collections/authored_vector.rs` (same)

Skip this task if the audit chose Path A or B.

### Step 1: Extend `authored_map.rs` module doc-note

Find the current `# CRDT trait surface` block (PR-2435 added it). After the existing two-numbered-points justification for not implementing `CrdtMap`, append:

```rust
//!
//! # Why this isn't unified with `AuthoredVector` either
//!
//! Issue #2309 audited whether `AuthoredMap` and `AuthoredVector` could share
//! a single `Authored<C>` wrapper. The audit found that the author-tracking
//! mechanisms diverge in <specific way found during audit>:
//!
//! - `AuthoredMap`: <quote the divergent behaviour from the comparison doc>.
//! - `AuthoredVector`: <quote the divergent behaviour>.
//!
//! Unifying would require either parameterising over the difference (which
//! adds a type-system burden equal to the duplication it removes) or hiding
//! the divergence behind a runtime branch (which moves the divergence from
//! the type system into untyped state). Neither is a net win. See the
//! comparison doc at `docs/superpowers/notes/2026-05-21-authored-comparison.md`
//! for the full audit.
```

Fill in the bracketed sections with the actual findings from Task 3's comparison doc.

### Step 2: Same edit in `authored_vector.rs`

Mirror Step 1's edit at the bottom of `authored_vector.rs`'s module doc-note.

### Step 3: Run the test suite (no behavioural change expected)

Run: `cargo test -p calimero-storage --lib --tests 2>&1 | tail -10`
Expected: same passing baseline as before; this is a docs-only change.

### Step 4: Commit (Path C complete)

```bash
git add crates/storage/src/collections/authored_map.rs \
        crates/storage/src/collections/authored_vector.rs
git commit -m "docs(storage): explain why AuthoredMap / AuthoredVector stay separate

Issue #2309 audited a possible Authored<C> wrapper unification. The audit
(see docs/superpowers/notes/2026-05-21-authored-comparison.md) found that
<one-line summary of divergence>. Documenting the decision in both module
doc-notes so a future reader doesn't re-litigate the question.

Closes #2309 as wontfix. Refs #2308."
```

---

## Task 5: Update issue #2309 with the decision

**Files:**
- No code changes.

- [ ] **Step 1: Post the decision as a comment on the issue**

```bash
gh issue comment 2309 --repo calimero-network/core --body "$(cat <<'EOF'
Audit complete — see [the comparison doc](https://github.com/calimero-network/core/blob/feat/2309-authored-wrapper-audit/docs/superpowers/notes/2026-05-21-authored-comparison.md) for the full table.

**Decision: <Path A / B / C>**

- <One short sentence on why>.
- Implementation in PR #<TBD — fill in after PR creation>.
- Net LOC change: <measured delta> (or "no code change" for Path C).

Branch: `feat/2309-authored-wrapper-audit`. Depends on #2308 / PR #2435.
EOF
)"
```

- [ ] **Step 2: Close the issue if Path C**

Only if the audit chose Path C:
```bash
gh issue close 2309 --repo calimero-network/core --reason "not planned" --comment "wontfix per audit — see comment above."
```

For Path A or B, leave the issue open until the PR merges.

---

## Task 6: Push branch + open draft PR

**Files:**
- No code changes; publication.

- [ ] **Step 1: Push the branch**

Run:
```bash
git push -u origin feat/2309-authored-wrapper-audit
```

- [ ] **Step 2: Open a draft PR**

The PR title and body vary by path. Use whichever of the three below matches the chosen path.

**Path A title:** `feat(storage): Authored<C> wrapper unifies AuthoredMap / AuthoredVector — closes #2309`

**Path B title:** `refactor(storage): shared authored_common helper for AuthoredMap / AuthoredVector — closes #2309`

**Path C title:** `docs(storage): wontfix Authored<C> unification audit — closes #2309`

Body template (adapt per path):
```bash
gh pr create --draft --repo calimero-network/core \
  --base feat/2308-crdt-trait-hierarchy \
  --head feat/2309-authored-wrapper-audit \
  --title "<title from above>" \
  --body "$(cat <<'EOF'
Closes #2309 under epic #2301. Base branch is PR #2435 (#2308's implementation), so this stack lands after #2435.

## Audit summary

See [the comparison doc](docs/superpowers/notes/2026-05-21-authored-comparison.md) for the full table.

**Decision: <Path A / B / C>** — <one-sentence reason>.

## What changed

<bullet list for the chosen path; copy from the relevant Task 4* commit message>

## Test plan

- [x] `cargo test -p calimero-storage --lib --tests` — green.
- [x] All 11 existing AuthoredMap tests + 9 AuthoredVector tests pass unchanged.
- [<x if A or B>] New contract test in `tests/crdt_contract.rs` for `AuthoredMap<String, Counter>` (only if Path A or B added one).

## Net LOC

- Path A: <actual delta>.
- Path B: <actual delta>.
- Path C: 0 (docs-only).

cc @chefsale @rtb-12
EOF
)"
```

- [ ] **Step 3: Stop and wait for review**

Same checkpoint as PR-2435 — do not push the PR out of draft until a maintainer has reviewed.

---

## Self-review notes (for the plan author)

**Spec coverage vs. issue #2309:**
- ✅ "Open the audit first; close as wontfix if the unification doesn't hold." — Task 3 produces the audit and gates the next tasks on a real decision.
- ✅ "Read `authored_map.rs` and `authored_vector.rs` carefully" — Tasks 1 & 2.
- ✅ "Is the author-tracking semantically identical?" — the comparison-table row in Task 3 Step 1.
- ✅ "Do they diverge in conflict resolution? merge behaviour? key derivation?" — same row.
- ✅ "Is there per-collection logic that would break under a generic wrapper?" — separate collection-shape table.
- ✅ "If they diverge: close this as wontfix and document why" — Task 4C + Task 5 Step 2.
- ✅ "If they don't diverge: collapse them into the wrapper" — Task 4A.
- ✅ "Keep `AuthoredMap = Authored<UnorderedMap<K, V>>` ... as type aliases" — Task 4A Step 4.
- ✅ "Existing call sites unchanged" — type-alias approach in 4A; bodies unchanged in 4B; no code change in 4C.

**Placeholder scan:**
- "Add appropriate error handling" — none.
- "TODO" / "TBD" — present only in PR-body templates as `<TBD — fill in after PR creation>`, which is intentional (the PR number isn't known until creation).
- "Similar to Task N" — none; each fork (4A/4B/4C) is self-contained.
- Steps without code blocks — Tasks 1 & 2 are "read the file" steps and intentionally don't show code; the artifact is the audit notes structure given verbatim.

**Type consistency:**
- `Authored<C>` field is `inner: C` everywhere (4A Step 3 and the alias forms in Steps 4 & 5).
- Helper fns in `authored_common.rs` are named `current_executor`, `make_owner_stamp`, `executor_matches_owner` consistently (4B Steps 3, 6, 7, and test in Step 1).
- `StorageType::User { owner }` reference matches the existing field name in `crate::address`.

**Known risk:**
- 4B Steps 6 & 7 (replace inline logic with helper calls) hand-edit existing match arms. The instruction is "read the exact existing structure before editing" — the implementer must do that, or risks losing the surrounding error-variant logic that's not part of the deduplication target. This is the same pattern that worked for the PR-2435 `map_err → ?` cleanup in `crdt_impls.rs`.
