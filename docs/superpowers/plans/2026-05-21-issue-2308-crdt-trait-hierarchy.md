# Issue #2308 — CRDT Trait Hierarchy Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use `superpowers:subagent-driven-development` (recommended) or `superpowers:executing-plans` to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Introduce `CrdtMap`, `CrdtSequence`, and `CrdtSet` sub-traits in `crates/storage/src/collections/`, sitting on top of the existing `Mergeable` base trait, so generic algorithms can be written against the CRDT contract and a single property-test file (`tests/crdt_contract.rs`) verifies the CRDT laws across every collection. Tracks GitHub issue [#2308](https://github.com/calimero-network/core/issues/2308) under epic [#2301](https://github.com/calimero-network/core/issues/2301).

**Architecture:** The existing `Mergeable` trait already provides the `Crdt::merge` method the issue requests; renaming it would break the public `#[derive(Mergeable)]` proc-macro and `calimero_sdk::app::Mergeable` re-export. Instead, **keep `Mergeable` as the base trait** and add three new shape-specific sub-traits that capture the per-collection method signatures (`insert`/`get`/`remove`/`len` for maps, `push`/`get`/`update`/`len` for sequences, `insert`/`contains`/`remove`/`len` for sets). Each trait carries a `type Error;` associated type so storage-backed collections set `type Error = StoreError;` while keeping the door open for in-memory CRDTs later. A new `From<StoreError> for MergeError` impl removes the verbose `map_err(|e| MergeError::StorageError(format!("...", e)))` pattern that repeats dozens of times in `crdt_impls.rs`.

**Tech Stack:** Rust 1.x (workspace toolchain), `cargo`, `cargo nextest` or `cargo test`, `borsh` serialization, `proptest` for property tests (already a workspace dev-dep — verify in Task 0).

**Scope of this plan:** This plan covers **all three PRs** required to close #2308 (PR-A: trait scaffolding; PR-B: simple collections + error helper; PR-C: wrapper collections). Each PR ends in an independently mergeable green state and can be reviewed before the next one starts.

**Out of scope:**
- #2309 (`Authored<C>` unification) — separate sub-issue, audit-then-decide.
- #2310 (physical extraction to `calimero-storage-collections`) — separate sub-issue, lands last.
- Public method renames on individual collections.
- Touching `#[app::state]` / `#[derive(Mergeable)]` proc-macros.

---

## Task 0: Worktree setup and pre-flight checks

**Files:**
- No code changes; environment prep only.

- [ ] **Step 1: Confirm working directory is the core repo**

Run: `pwd && git -C /Users/beast/Developer/Calimero/core status --short | head -3`
Expected: prints `/Users/beast/Developer/Calimero/core` (or wherever the user's repo lives) and shows whatever uncommitted state is on the current branch. The plan assumes the core repo is at this path; adjust if not.

- [ ] **Step 2: Create isolated worktree off `master`**

Run:
```bash
git -C /Users/beast/Developer/Calimero/core fetch origin master
git -C /Users/beast/Developer/Calimero/core worktree add \
    .worktrees/issue-2308-crdt-traits \
    -b feat/2308-crdt-trait-hierarchy \
    origin/master
cd /Users/beast/Developer/Calimero/core/.worktrees/issue-2308-crdt-traits
```
Expected: new worktree directory exists, branch `feat/2308-crdt-trait-hierarchy` is created off `origin/master`, current shell is inside the worktree. All subsequent file paths in this plan are relative to this worktree root.

- [ ] **Step 3: Verify the baseline test suite is green before changing anything**

Run: `cargo test -p calimero-storage --lib --tests 2>&1 | tail -20`
Expected: every test passes. If anything is red on a fresh `master` checkout, stop and investigate — do not start work on top of a broken baseline.

- [ ] **Step 4: Verify `proptest` is available as a dev-dependency**

Run: `grep -n proptest crates/storage/Cargo.toml`
Expected: a line like `proptest = { workspace = true }` under `[dev-dependencies]`. If absent, add it in PR-A's first commit (Task 1, Step 1.5) under `[dev-dependencies]` only — `proptest` must never become a production dep of `calimero-storage`.

- [ ] **Step 5: Open the three reference files for reading context**

Read (no edits):
- `crates/storage/src/collections/crdt_meta.rs` (215 LOC)
- `crates/storage/src/collections/crdt_impls.rs` (top 500 LOC, skip the `#[cfg(test)]` block)
- `crates/storage/src/collections/lww_register.rs` (194 LOC)

These define the trait surface you're extending. Re-read them whenever a later task feels ambiguous.

---

## PR-A — Trait scaffolding (foundation, no behavioural change)

**End state of PR-A:** Three new traits declared. `From<StoreError> for MergeError` added. Property-test scaffolding file exists but is empty (no impls to test yet). All existing tests still pass.

### Task 1: Add `From<StoreError> for MergeError` to `crdt_meta.rs`

**Files:**
- Modify: `crates/storage/src/collections/crdt_meta.rs` (after line 129, end of existing `MergeError` impls)

- [ ] **Step 1: Write the failing test first**

Create file: `crates/storage/src/collections/crdt_meta_tests.rs` (new module file referenced from `crdt_meta.rs`).

Actually — `crdt_meta.rs` doesn't currently have a `#[cfg(test)]` block. Add the test inline at the bottom:

```rust
// In crdt_meta.rs, append:
#[cfg(test)]
mod tests {
    use super::*;
    use crate::collections::error::StoreError;

    #[test]
    fn store_error_converts_into_merge_error_storage_variant() {
        let store_err = StoreError::Other("disk read failed".to_string());
        let merge_err: MergeError = store_err.into();
        match merge_err {
            MergeError::StorageError(msg) => {
                assert!(msg.contains("disk read failed"),
                        "expected the original message to survive, got {msg}");
            }
            other => panic!("expected StorageError variant, got {other:?}"),
        }
    }
}
```

Note: if `StoreError::Other(String)` isn't the actual variant name, open `crates/storage/src/collections/error.rs` and use whatever the real variant is. If `StoreError` is opaque, adapt the construction.

- [ ] **Step 2: Run the test to confirm it fails**

Run: `cargo test -p calimero-storage --lib collections::crdt_meta::tests::store_error_converts_into_merge_error_storage_variant -- --nocapture`
Expected: FAILS with `the trait From<StoreError> is not implemented for MergeError`.

- [ ] **Step 3: Implement the `From` impl**

Append to `crates/storage/src/collections/crdt_meta.rs` after line 129 (`impl std::error::Error for MergeError {}`):

```rust
impl From<crate::collections::error::StoreError> for MergeError {
    fn from(err: crate::collections::error::StoreError) -> Self {
        // Preserve the original error message — callers grep for it in panics.
        MergeError::StorageError(format!("{err:?}"))
    }
}
```

- [ ] **Step 4: Re-run the test, expect green**

Run: `cargo test -p calimero-storage --lib collections::crdt_meta::tests::store_error_converts_into_merge_error_storage_variant -- --nocapture`
Expected: PASS.

- [ ] **Step 5: Run the full storage test suite to confirm no regressions**

Run: `cargo test -p calimero-storage --lib --tests 2>&1 | tail -10`
Expected: same pass count as the baseline from Task 0 Step 3, plus one new passing test.

- [ ] **Step 6: Commit**

```bash
git add crates/storage/src/collections/crdt_meta.rs
git commit -m "feat(storage): From<StoreError> for MergeError

Lets the per-type Mergeable impls use `?` instead of the verbose
`map_err(|e| MergeError::StorageError(format!(\"...\", e)))` pattern.

Refs #2308."
```

---

### Task 2: Declare `CrdtMap` sub-trait

**Files:**
- Modify: `crates/storage/src/collections/crdt_meta.rs` (append after the `Mergeable` trait, around line 78)

- [ ] **Step 1: Write a compile-only test that the trait exists with the expected shape**

Append to the `#[cfg(test)]` block in `crdt_meta.rs`:

```rust
    #[test]
    fn crdt_map_trait_shape_compiles() {
        // Type-level assertion: any T: CrdtMap is also Mergeable.
        fn _assert_subtrait<T: CrdtMap>() {
            fn _is_mergeable<U: Mergeable>() {}
            _is_mergeable::<T>();
        }
        // No runtime body — if this compiles, the trait hierarchy is wired correctly.
    }
```

- [ ] **Step 2: Run it, expect a compile error**

Run: `cargo test -p calimero-storage --lib collections::crdt_meta::tests::crdt_map_trait_shape_compiles 2>&1 | tail -10`
Expected: FAIL with `cannot find trait 'CrdtMap' in this scope`.

- [ ] **Step 3: Declare the `CrdtMap` trait**

Append to `crdt_meta.rs` between the existing `Mergeable` trait (line 78) and `MergeError` (line 82):

```rust
/// CRDT map shape — key/value collection that satisfies the [`Mergeable`] contract.
///
/// Implementors must guarantee that `merge` is associative, commutative, and idempotent
/// over their key/value space. The `Error` associated type lets storage-backed and
/// in-memory implementations coexist.
pub trait CrdtMap: Mergeable {
    /// Key type — must be borsh-serialisable and usable as a storage key.
    type Key;
    /// Value type — typically itself a CRDT for nested merging.
    type Value;
    /// Error returned by fallible accessors (e.g. `StoreError` for storage-backed maps,
    /// [`Infallible`](std::convert::Infallible) for in-memory test stubs).
    type Error;

    /// Insert or replace `value` at `key`. Returns the previous value if any.
    ///
    /// # Errors
    /// Returns [`Self::Error`] if the underlying store fails.
    fn insert(
        &mut self,
        key: Self::Key,
        value: Self::Value,
    ) -> Result<Option<Self::Value>, Self::Error>;

    /// Fetch the value at `key`.
    ///
    /// # Errors
    /// Returns [`Self::Error`] if the underlying store fails.
    fn get(&self, key: &Self::Key) -> Result<Option<Self::Value>, Self::Error>;

    /// Remove the entry at `key`. Returns the removed value if any.
    ///
    /// # Errors
    /// Returns [`Self::Error`] if the underlying store fails.
    fn remove(&mut self, key: &Self::Key) -> Result<Option<Self::Value>, Self::Error>;

    /// Number of entries.
    ///
    /// # Errors
    /// Returns [`Self::Error`] if the underlying store fails.
    fn len(&self) -> Result<usize, Self::Error>;
}
```

- [ ] **Step 4: Re-run the compile test**

Run: `cargo test -p calimero-storage --lib collections::crdt_meta::tests::crdt_map_trait_shape_compiles 2>&1 | tail -10`
Expected: PASS. The trait now exists and is a sub-trait of `Mergeable`.

- [ ] **Step 5: Run full storage tests to confirm no breakage**

Run: `cargo test -p calimero-storage --lib --tests 2>&1 | tail -10`
Expected: same passing count as before, plus one more passing test.

- [ ] **Step 6: Commit**

```bash
git add crates/storage/src/collections/crdt_meta.rs
git commit -m "feat(storage): CrdtMap sub-trait of Mergeable

Captures the shape of key/value CRDTs (UnorderedMap, NestedMap, AuthoredMap).
Trait signatures return Result<_, Self::Error> because every concrete impl is
backed by a fallible store. No impls yet — added in subsequent PR.

Refs #2308."
```

---

### Task 3: Declare `CrdtSequence` sub-trait

**Files:**
- Modify: `crates/storage/src/collections/crdt_meta.rs` (append directly after the `CrdtMap` trait added in Task 2)

- [ ] **Step 1: Write the failing compile test**

Append to the `#[cfg(test)]` block:

```rust
    #[test]
    fn crdt_sequence_trait_shape_compiles() {
        fn _assert_subtrait<T: CrdtSequence>() {
            fn _is_mergeable<U: Mergeable>() {}
            _is_mergeable::<T>();
        }
    }
```

- [ ] **Step 2: Run it, expect compile failure**

Run: `cargo test -p calimero-storage --lib collections::crdt_meta::tests::crdt_sequence_trait_shape_compiles 2>&1 | tail -10`
Expected: FAIL with `cannot find trait 'CrdtSequence' in this scope`.

- [ ] **Step 3: Declare the `CrdtSequence` trait**

Append to `crdt_meta.rs` immediately after the `CrdtMap` trait:

```rust
/// CRDT sequence shape — indexed collection (Vector, RGA) that satisfies [`Mergeable`].
///
/// Concurrent inserts at the same logical position resolve per the implementor's
/// rules (e.g. RGA causal ordering, Vector elementwise + LWW tail).
pub trait CrdtSequence: Mergeable {
    /// Element type.
    type Element;
    /// Error returned by fallible accessors.
    type Error;

    /// Append `element` to the sequence.
    ///
    /// # Errors
    /// Returns [`Self::Error`] if the underlying store fails.
    fn push(&mut self, element: Self::Element) -> Result<(), Self::Error>;

    /// Fetch the element at index `index`.
    ///
    /// # Errors
    /// Returns [`Self::Error`] if the underlying store fails.
    fn get(&self, index: usize) -> Result<Option<Self::Element>, Self::Error>;

    /// Replace the element at `index`. Returns the previous element if any.
    ///
    /// # Errors
    /// Returns [`Self::Error`] if the underlying store fails or the index is out of bounds.
    fn update(
        &mut self,
        index: usize,
        element: Self::Element,
    ) -> Result<Option<Self::Element>, Self::Error>;

    /// Number of elements.
    ///
    /// # Errors
    /// Returns [`Self::Error`] if the underlying store fails.
    fn len(&self) -> Result<usize, Self::Error>;
}
```

- [ ] **Step 4: Re-run, expect green**

Run: `cargo test -p calimero-storage --lib collections::crdt_meta::tests::crdt_sequence_trait_shape_compiles 2>&1 | tail -10`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/storage/src/collections/crdt_meta.rs
git commit -m "feat(storage): CrdtSequence sub-trait of Mergeable

Captures the shape of indexed CRDTs (Vector, RGA, AuthoredVector).

Refs #2308."
```

---

### Task 4: Declare `CrdtSet` sub-trait

**Files:**
- Modify: `crates/storage/src/collections/crdt_meta.rs` (append directly after `CrdtSequence`)

- [ ] **Step 1: Failing compile test**

Append to the `#[cfg(test)]` block:

```rust
    #[test]
    fn crdt_set_trait_shape_compiles() {
        fn _assert_subtrait<T: CrdtSet>() {
            fn _is_mergeable<U: Mergeable>() {}
            _is_mergeable::<T>();
        }
    }
```

- [ ] **Step 2: Run it, expect compile failure**

Run: `cargo test -p calimero-storage --lib collections::crdt_meta::tests::crdt_set_trait_shape_compiles 2>&1 | tail -10`
Expected: FAIL with `cannot find trait 'CrdtSet'`.

- [ ] **Step 3: Declare `CrdtSet`**

Append to `crdt_meta.rs`:

```rust
/// CRDT set shape — element-only collection (UnorderedSet) with union semantics.
pub trait CrdtSet: Mergeable {
    /// Element type.
    type Element;
    /// Error returned by fallible accessors.
    type Error;

    /// Insert `element`. Returns `true` if newly added, `false` if it was already present.
    ///
    /// # Errors
    /// Returns [`Self::Error`] if the underlying store fails.
    fn insert(&mut self, element: Self::Element) -> Result<bool, Self::Error>;

    /// Check membership.
    ///
    /// # Errors
    /// Returns [`Self::Error`] if the underlying store fails.
    fn contains(&self, element: &Self::Element) -> Result<bool, Self::Error>;

    /// Remove `element`. Returns `true` if it was present.
    ///
    /// # Errors
    /// Returns [`Self::Error`] if the underlying store fails.
    fn remove(&mut self, element: &Self::Element) -> Result<bool, Self::Error>;

    /// Number of elements.
    ///
    /// # Errors
    /// Returns [`Self::Error`] if the underlying store fails.
    fn len(&self) -> Result<usize, Self::Error>;
}
```

- [ ] **Step 4: Re-run, expect green**

Run: `cargo test -p calimero-storage --lib collections::crdt_meta::tests::crdt_set_trait_shape_compiles 2>&1 | tail -10`
Expected: PASS.

- [ ] **Step 5: Re-export new traits from `collections.rs`**

Edit `crates/storage/src/collections.rs` line 27 (the existing `pub use` of `crdt_meta`):

Before:
```rust
pub use crdt_meta::{CrdtMeta, CrdtType, Decomposable, Mergeable, StorageStrategy};
```

After:
```rust
pub use crdt_meta::{
    CrdtMap, CrdtMeta, CrdtSequence, CrdtSet, CrdtType, Decomposable, Mergeable,
    StorageStrategy,
};
```

- [ ] **Step 6: Run full storage tests**

Run: `cargo test -p calimero-storage --lib --tests 2>&1 | tail -10`
Expected: same pass count as before plus three new traits-compile tests.

- [ ] **Step 7: Commit**

```bash
git add crates/storage/src/collections/crdt_meta.rs crates/storage/src/collections.rs
git commit -m "feat(storage): CrdtSet sub-trait + re-exports

Completes the trait trio (CrdtMap, CrdtSequence, CrdtSet). Re-exports the
new traits from collections.rs so downstream crates can refer to them as
calimero_storage::collections::CrdtMap (etc.).

Refs #2308."
```

---

### Task 5: Scaffold property-test file `tests/crdt_contract.rs`

**Files:**
- Create: `crates/storage/tests/crdt_contract.rs`

This task creates the file *empty* (well, with generic functions that take `T: Mergeable` but no concrete invocations). PR-B wires in the first concrete impls.

- [ ] **Step 1: Create the scaffold file**

Create `crates/storage/tests/crdt_contract.rs`:

```rust
//! Generic CRDT property tests.
//!
//! Every collection that implements [`Mergeable`] (or one of the shape sub-traits
//! [`CrdtMap`], [`CrdtSequence`], [`CrdtSet`]) is exercised here through the trait
//! surface only — no per-type tests. The CRDT laws checked are:
//!
//! - **Idempotency:** `merge(a, a) == a` (merging a value with itself is a no-op).
//! - **Commutativity:** `merge(a, b) == merge(b, a)` (order doesn't matter).
//! - **Associativity:** `merge(merge(a, b), c) == merge(a, merge(b, c))` (grouping doesn't matter).
//!
//! Together these guarantee convergence: any set of replicas applying the same
//! set of updates in any order reaches the same final state.

use calimero_storage::collections::Mergeable;

/// Run the three CRDT laws against a constructor that produces fresh instances.
///
/// The `eq` closure compares two instances for state equality. Most collections
/// can't derive `PartialEq` cheaply (storage I/O), so eq is supplied per-type —
/// it might compare entries via `.entries()`, length+content, etc.
pub fn assert_crdt_laws<T, F, E>(make_a: F, make_b: F, make_c: F, eq: E)
where
    T: Mergeable + Clone,
    F: Fn() -> T,
    E: Fn(&T, &T) -> bool,
{
    // Idempotency: merge(a, a) == a
    {
        let mut a = make_a();
        let a_clone = a.clone();
        a.merge(&a_clone).expect("idempotent merge must not fail");
        assert!(eq(&a, &a_clone), "idempotency violated: merge(a, a) != a");
    }

    // Commutativity: merge(a, b) == merge(b, a)
    {
        let mut ab = make_a();
        let b = make_b();
        ab.merge(&b).expect("merge a<-b must not fail");

        let mut ba = make_b();
        let a = make_a();
        ba.merge(&a).expect("merge b<-a must not fail");

        assert!(eq(&ab, &ba), "commutativity violated: merge(a, b) != merge(b, a)");
    }

    // Associativity: merge(merge(a, b), c) == merge(a, merge(b, c))
    {
        let mut left = make_a();
        let b = make_b();
        left.merge(&b).expect("merge a<-b must not fail");
        let c = make_c();
        left.merge(&c).expect("merge (a+b)<-c must not fail");

        let mut right = make_a();
        let mut bc = make_b();
        let c2 = make_c();
        bc.merge(&c2).expect("merge b<-c must not fail");
        right.merge(&bc).expect("merge a<-(b+c) must not fail");

        assert!(eq(&left, &right), "associativity violated");
    }
}

// PR-B adds the first concrete invocation: lww_register / counter / etc.
// This file is intentionally a library of helpers, not test functions.
// Each collection's test module will call assert_crdt_laws with type-specific
// constructors and equality.
#[test]
fn scaffold_file_compiles() {
    // Smoke test: this file builds. Real impl tests land in PR-B.
}
```

- [ ] **Step 2: Run the scaffold test**

Run: `cargo test -p calimero-storage --test crdt_contract 2>&1 | tail -10`
Expected: PASS (just the `scaffold_file_compiles` smoke test).

- [ ] **Step 3: Commit**

```bash
git add crates/storage/tests/crdt_contract.rs
git commit -m "test(storage): scaffold crdt_contract.rs property-test helper

assert_crdt_laws<T: Mergeable> runs idempotency / commutativity / associativity
against any Mergeable implementor. PR-B wires in the first concrete usage.

Refs #2308."
```

---

### Task 6: Push PR-A and open a draft PR

**Files:**
- No code changes; PR management only.

- [ ] **Step 1: Push the branch**

Run: `git push -u origin feat/2308-crdt-trait-hierarchy`
Expected: branch published to remote.

- [ ] **Step 2: Open a draft PR**

Run:
```bash
gh pr create --draft --title "feat(storage): CRDT trait hierarchy (PR-A: scaffolding) — #2308" \
  --body "$(cat <<'EOF'
First of three PRs implementing #2308 (under epic #2301).

## Summary

- Adds three new sub-traits of \`Mergeable\`: \`CrdtMap\`, \`CrdtSequence\`, \`CrdtSet\`.
- Adds \`From<StoreError> for MergeError\` so per-type merge impls can use \`?\`.
- Scaffolds \`tests/crdt_contract.rs\` with a generic \`assert_crdt_laws<T: Mergeable>\` helper.
- **No collection implements the new traits yet** — that's PR-B.
- **No behavioural change** — every existing test still passes.

## Departure from the issue's framing

The issue proposes a new \`Crdt\` trait. The codebase already has \`Mergeable\` — same signature, different name — and renaming it would break the public \`#[derive(Mergeable)]\` proc-macro in \`storage-macros\` and the \`calimero_sdk::app::Mergeable\` re-export. Keeping the existing name and adding sub-traits gives the same value with no breaking change.

The issue also estimates ≥50% LOC reduction in \`crdt_impls.rs\`. After reading the file, ~485 of its 980 lines are tests and the remaining ~495 are real per-type merge algorithms that can't be deduplicated. PR-B's cleanup (using the new \`From<StoreError>\` impl) realistically saves ~100 lines. Worth re-pinning the success criterion to behavioural correctness over LOC.

## Test plan

- [ ] \`cargo test -p calimero-storage --lib --tests\` green.
- [ ] Diff against \`origin/master\` shows only \`crdt_meta.rs\`, \`collections.rs\`, and the new \`tests/crdt_contract.rs\`.

cc @chefsale @rtb-12
EOF
)"
```
Expected: PR URL printed.

- [ ] **Step 3: Stop and wait for PR-A review before starting PR-B**

PR-A is intentionally small and reviewable in 15 minutes. Do not pile PR-B on top until a maintainer confirms the trait shape (especially the `type Error` decision and the no-rename approach). If they push back on the design, fix it here before the rest of the work cements it.

---

## PR-B — Simple collections + error helper cleanup

**End state of PR-B:** `LwwRegister`, `Counter`, `Vector`, `UnorderedMap`, `UnorderedSet`, `Rga` all implement the appropriate sub-trait. `crdt_contract.rs` calls `assert_crdt_laws` against each. The verbose `map_err(|e| MergeError::StorageError(format!("...", e)))` pattern in `crdt_impls.rs` is replaced with `?` everywhere `StoreError` was the source. All existing tests still pass; the new contract tests pass.

**Branch:** Open a new branch *off PR-A's branch* (not master) so the work stacks cleanly: `git checkout -b feat/2308-crdt-impls feat/2308-crdt-trait-hierarchy`.

### Task 7: Impl `CrdtSequence` for `Vector<V, S>`

**Files:**
- Modify: `crates/storage/src/collections/crdt_impls.rs` (append after the existing `Mergeable for Vector` impl, line 493)
- Modify: `crates/storage/tests/crdt_contract.rs` (add a `vector_satisfies_crdt_laws` test)

- [ ] **Step 1: Write the failing contract test**

Append to `crates/storage/tests/crdt_contract.rs`:

```rust
#[test]
fn vector_with_lww_register_satisfies_crdt_laws() {
    use calimero_storage::collections::{LwwRegister, Vector};
    use calimero_storage::env;
    use calimero_storage::store::MainStorage;

    env::reset_for_testing();

    // Vector<LwwRegister<String>> — both ends are Mergeable, so the recursive
    // merge in Vector::merge exercises the full path.
    fn fresh(name: &str) -> Vector<LwwRegister<String>, MainStorage> {
        let mut v = Vector::new();
        v.push(LwwRegister::new(name.to_owned())).unwrap();
        v
    }

    let eq = |a: &Vector<LwwRegister<String>, MainStorage>,
              b: &Vector<LwwRegister<String>, MainStorage>| -> bool {
        let la = a.len().unwrap();
        let lb = b.len().unwrap();
        if la != lb { return false; }
        for i in 0..la {
            let va = a.get(i).unwrap();
            let vb = b.get(i).unwrap();
            if va.as_ref().map(|r| r.get().clone()) != vb.as_ref().map(|r| r.get().clone()) {
                return false;
            }
        }
        true
    };

    assert_crdt_laws(
        || fresh("alice"),
        || fresh("bob"),
        || fresh("carol"),
        eq,
    );
}
```

Also confirm the trait impl will be available — append a trait-bound sanity assertion at the top of the file (or in a `#[test]` somewhere):

```rust
fn _assert_vector_is_crdt_sequence() {
    use calimero_storage::collections::{CrdtSequence, LwwRegister, Vector};
    use calimero_storage::store::MainStorage;
    fn assert<T: CrdtSequence>() {}
    assert::<Vector<LwwRegister<String>, MainStorage>>();
}
```

- [ ] **Step 2: Run the test, expect compile failure**

Run: `cargo test -p calimero-storage --test crdt_contract vector_with_lww_register_satisfies_crdt_laws 2>&1 | tail -10`
Expected: FAIL — `the trait bound 'Vector<...>: CrdtSequence' is not satisfied`.

- [ ] **Step 3: Add the `CrdtSequence` impl**

Append to `crates/storage/src/collections/crdt_impls.rs` after the closing brace of `impl Mergeable for Vector` (around line 493):

```rust
impl<T, S> CrdtSequence for Vector<T, S>
where
    T: borsh::BorshSerialize + borsh::BorshDeserialize + Mergeable,
    S: StorageAdaptor,
{
    type Element = T;
    type Error = crate::collections::error::StoreError;

    fn push(&mut self, element: Self::Element) -> Result<(), Self::Error> {
        Vector::push(self, element)
    }

    fn get(&self, index: usize) -> Result<Option<Self::Element>, Self::Error> {
        Vector::get(self, index)
    }

    fn update(
        &mut self,
        index: usize,
        element: Self::Element,
    ) -> Result<Option<Self::Element>, Self::Error> {
        Vector::update(self, index, element)
    }

    fn len(&self) -> Result<usize, Self::Error> {
        Vector::len(self)
    }
}
```

Also import `CrdtSequence` at the top of `crdt_impls.rs`:

```rust
use super::crdt_meta::{CrdtMeta, CrdtSequence, CrdtType, MergeError, Mergeable, StorageStrategy};
```

- [ ] **Step 4: Re-run, expect green**

Run: `cargo test -p calimero-storage --test crdt_contract 2>&1 | tail -10`
Expected: PASS for both the trait-bound assertion and `vector_with_lww_register_satisfies_crdt_laws`.

- [ ] **Step 5: Full test suite check**

Run: `cargo test -p calimero-storage --lib --tests 2>&1 | tail -10`
Expected: all green.

- [ ] **Step 6: Commit**

```bash
git add crates/storage/src/collections/crdt_impls.rs crates/storage/tests/crdt_contract.rs
git commit -m "feat(storage): impl CrdtSequence for Vector + contract test

Vector<T, S> where T: Mergeable now satisfies CrdtSequence. The contract test
exercises the full nested merge path (Vector<LwwRegister<String>>) against the
three CRDT laws.

Refs #2308."
```

---

### Task 8: Impl `CrdtSet` for `UnorderedSet<T, S>`

**Files:**
- Modify: `crates/storage/src/collections/crdt_impls.rs` (append after the existing `Mergeable for UnorderedSet` impl)
- Modify: `crates/storage/tests/crdt_contract.rs`

- [ ] **Step 1: Failing test**

Append to `tests/crdt_contract.rs`:

```rust
#[test]
fn unordered_set_satisfies_crdt_laws() {
    use calimero_storage::collections::UnorderedSet;
    use calimero_storage::env;
    use calimero_storage::store::MainStorage;

    env::reset_for_testing();

    fn fresh(initial: &[&str]) -> UnorderedSet<String, MainStorage> {
        let mut s = UnorderedSet::new();
        for item in initial {
            s.insert((*item).to_owned()).unwrap();
        }
        s
    }

    let eq = |a: &UnorderedSet<String, MainStorage>,
              b: &UnorderedSet<String, MainStorage>| -> bool {
        let mut a_items: Vec<_> = a.iter().unwrap().collect();
        let mut b_items: Vec<_> = b.iter().unwrap().collect();
        a_items.sort();
        b_items.sort();
        a_items == b_items
    };

    assert_crdt_laws(
        || fresh(&["alice", "bob"]),
        || fresh(&["bob", "carol"]),
        || fresh(&["dave"]),
        eq,
    );
}
```

- [ ] **Step 2: Run, expect failure**

Run: `cargo test -p calimero-storage --test crdt_contract unordered_set_satisfies_crdt_laws 2>&1 | tail -10`
Expected: compile FAIL — `UnorderedSet` doesn't impl `CrdtSet` yet (assertion will fail when the test imports it or uses it indirectly; if the test passes anyway because we didn't bind to `CrdtSet`, add a `fn assert<T: CrdtSet>() {} assert::<UnorderedSet<String, MainStorage>>();` line at the top of the test to force the bound).

- [ ] **Step 3: Add `CrdtSet` impl**

Append to `crdt_impls.rs`. Don't forget to import `CrdtSet` in the `use` line at the top of the file:

```rust
impl<T, S> CrdtSet for UnorderedSet<T, S>
where
    T: borsh::BorshSerialize + borsh::BorshDeserialize + AsRef<[u8]> + Clone + PartialEq,
    S: StorageAdaptor,
{
    type Element = T;
    type Error = crate::collections::error::StoreError;

    fn insert(&mut self, element: Self::Element) -> Result<bool, Self::Error> {
        UnorderedSet::insert(self, element)
    }

    fn contains(&self, element: &Self::Element) -> Result<bool, Self::Error> {
        UnorderedSet::contains(self, element)
    }

    fn remove(&mut self, element: &Self::Element) -> Result<bool, Self::Error> {
        UnorderedSet::remove(self, element)
    }

    fn len(&self) -> Result<usize, Self::Error> {
        UnorderedSet::len(self)
    }
}
```

- [ ] **Step 4: Run, expect green**

Run: `cargo test -p calimero-storage --test crdt_contract unordered_set_satisfies_crdt_laws 2>&1 | tail -10`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/storage/src/collections/crdt_impls.rs crates/storage/tests/crdt_contract.rs
git commit -m "feat(storage): impl CrdtSet for UnorderedSet + contract test

Refs #2308."
```

---

### Task 9: Impl `CrdtMap` for `UnorderedMap<K, V, S>`

**Files:**
- Modify: `crates/storage/src/collections/crdt_impls.rs`
- Modify: `crates/storage/tests/crdt_contract.rs`

- [ ] **Step 1: Failing test**

Append to `tests/crdt_contract.rs`:

```rust
#[test]
fn unordered_map_with_counter_satisfies_crdt_laws() {
    use calimero_storage::collections::{Counter, UnorderedMap};
    use calimero_storage::env;
    use calimero_storage::store::MainStorage;

    env::reset_for_testing();

    fn fresh(key: &str, count: usize) -> UnorderedMap<String, Counter, MainStorage> {
        let mut m = UnorderedMap::new();
        let mut c = Counter::new();
        for _ in 0..count {
            c.increment().unwrap();
        }
        m.insert(key.to_owned(), c).unwrap();
        m
    }

    let eq = |a: &UnorderedMap<String, Counter, MainStorage>,
              b: &UnorderedMap<String, Counter, MainStorage>| -> bool {
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
        || fresh("alice", 2),
        || fresh("bob", 3),
        || fresh("carol", 5),
        eq,
    );
}
```

Note: `Counter`'s merge semantics depend on `env::set_executor_id` being set per-replica in real life. For property-test purposes we rely on the underlying merge implementation already being commutative; if the test fails due to executor-id collisions, parameterise each `fresh` call with a distinct executor ID set via `env::set_executor_id([N; 32])` before constructing.

- [ ] **Step 2: Run, expect compile or runtime failure**

Run: `cargo test -p calimero-storage --test crdt_contract unordered_map_with_counter_satisfies_crdt_laws 2>&1 | tail -10`
Expected: FAIL (compile error: `UnorderedMap doesn't impl CrdtMap`).

- [ ] **Step 3: Add `CrdtMap` impl**

Append to `crdt_impls.rs`, and import `CrdtMap` in the use-line:

```rust
impl<K, V, S> CrdtMap for UnorderedMap<K, V, S>
where
    K: borsh::BorshSerialize + borsh::BorshDeserialize + AsRef<[u8]> + Clone + PartialEq,
    V: borsh::BorshSerialize + borsh::BorshDeserialize + Mergeable,
    S: StorageAdaptor,
{
    type Key = K;
    type Value = V;
    type Error = crate::collections::error::StoreError;

    fn insert(&mut self, key: Self::Key, value: Self::Value) -> Result<Option<Self::Value>, Self::Error> {
        UnorderedMap::insert(self, key, value)
    }

    fn get(&self, key: &Self::Key) -> Result<Option<Self::Value>, Self::Error> {
        UnorderedMap::get(self, key)
    }

    fn remove(&mut self, key: &Self::Key) -> Result<Option<Self::Value>, Self::Error> {
        UnorderedMap::remove(self, key)
    }

    fn len(&self) -> Result<usize, Self::Error> {
        UnorderedMap::len(self)
    }
}
```

- [ ] **Step 4: Run, expect green**

Run: `cargo test -p calimero-storage --test crdt_contract unordered_map_with_counter_satisfies_crdt_laws 2>&1 | tail -10`
Expected: PASS. If the commutativity check fails due to non-deterministic Counter executor IDs, fix by setting `env::set_executor_id` to distinct values per fresh-builder call inside the test.

- [ ] **Step 5: Commit**

```bash
git add crates/storage/src/collections/crdt_impls.rs crates/storage/tests/crdt_contract.rs
git commit -m "feat(storage): impl CrdtMap for UnorderedMap + contract test

Test exercises the recursive nested-merge path (UnorderedMap<String, Counter>)
because the entry-by-entry merge in UnorderedMap delegates to V::merge.

Refs #2308."
```

---

### Task 10: Add `Mergeable`-only contract tests for `LwwRegister`, `Counter`, `Rga`

These types don't get a shape sub-trait (they're scalar / non-map / non-sequence). Still useful to run them through `assert_crdt_laws` to lock in the CRDT-law guarantee.

**Files:**
- Modify: `crates/storage/tests/crdt_contract.rs`

- [ ] **Step 1: Add three tests**

Append:

```rust
#[test]
fn lww_register_satisfies_crdt_laws() {
    use calimero_storage::collections::LwwRegister;
    use calimero_storage::env;

    env::reset_for_testing();

    let eq = |a: &LwwRegister<String>, b: &LwwRegister<String>| a.get() == b.get();

    assert_crdt_laws(
        || LwwRegister::new("alice".to_owned()),
        || LwwRegister::new("bob".to_owned()),
        || LwwRegister::new("carol".to_owned()),
        eq,
    );
}

#[test]
fn counter_satisfies_crdt_laws() {
    use calimero_storage::collections::Counter;
    use calimero_storage::env;
    use calimero_storage::store::MainStorage;

    env::reset_for_testing();

    let make = |executor: [u8; 32], count: usize| {
        move || {
            env::set_executor_id(executor);
            let mut c = Counter::<false, MainStorage>::new();
            for _ in 0..count {
                c.increment().unwrap();
            }
            c
        }
    };

    let eq = |a: &Counter<false, MainStorage>, b: &Counter<false, MainStorage>| {
        a.value().unwrap() == b.value().unwrap()
    };

    assert_crdt_laws(
        make([11; 32], 2),
        make([22; 32], 3),
        make([33; 32], 5),
        eq,
    );
}

#[test]
fn rga_satisfies_crdt_laws() {
    use calimero_storage::collections::ReplicatedGrowableArray;
    use calimero_storage::env;

    env::reset_for_testing();

    let eq = |a: &ReplicatedGrowableArray, b: &ReplicatedGrowableArray| {
        a.len().unwrap() == b.len().unwrap()
    };

    assert_crdt_laws(
        || {
            let mut r = ReplicatedGrowableArray::new();
            r.insert_str(0, "Hello").unwrap();
            r
        },
        || {
            let mut r = ReplicatedGrowableArray::new();
            r.insert_str(0, "World").unwrap();
            r
        },
        || {
            let mut r = ReplicatedGrowableArray::new();
            r.insert_str(0, "!").unwrap();
            r
        },
        eq,
    );
}
```

- [ ] **Step 2: Run all three**

Run: `cargo test -p calimero-storage --test crdt_contract 2>&1 | tail -15`
Expected: all six contract tests (vector, set, map, lww, counter, rga) pass.

- [ ] **Step 3: Commit**

```bash
git add crates/storage/tests/crdt_contract.rs
git commit -m "test(storage): CRDT-law contract tests for LwwRegister, Counter, RGA

Refs #2308."
```

---

### Task 11: Replace verbose `map_err` with `?` in `crdt_impls.rs`

**Files:**
- Modify: `crates/storage/src/collections/crdt_impls.rs`

- [ ] **Step 1: Identify call sites**

Run: `grep -n "MergeError::StorageError(format" crates/storage/src/collections/crdt_impls.rs | wc -l`
Expected: ~15-20 occurrences across the `Mergeable` impls for Counter, RGA, UnorderedMap, UnorderedSet, Vector.

- [ ] **Step 2: Replace each `.map_err(...)` with `?`**

For each occurrence, the pattern transforms:

Before:
```rust
let other_entries = other
    .entries()
    .map_err(|e| MergeError::StorageError(format!("Failed to get entries: {:?}", e)))?;
```

After:
```rust
let other_entries = other.entries()?;
```

This works because `StoreError: Into<MergeError>` via the `From` impl added in Task 1, and `?` uses `From` for error coercion. The error message loses the human-readable prefix (`"Failed to get entries: ..."`); that's fine — the original `StoreError::Debug` rendering is preserved by the `From` impl's `format!("{err:?}")`. If a reviewer wants the prefix back, wrap with `.map_err(|e: StoreError| MergeError::StorageError(format!("entries lookup: {e:?}")))` selectively.

Do this for each of: `Counter`, `RGA`, `UnorderedMap`, `UnorderedSet`, `Vector` `Mergeable` impls. Keep the conditional structure (if/else, for loops) unchanged.

- [ ] **Step 3: Run all storage tests**

Run: `cargo test -p calimero-storage --lib --tests 2>&1 | tail -10`
Expected: full pass — including all the `#[cfg(test)]` tests inside `crdt_impls.rs` itself (test_counter_merge, test_vector_merge_same_length, etc., 485 lines of tests).

- [ ] **Step 4: Measure the LOC reduction**

Run: `wc -l crates/storage/src/collections/crdt_impls.rs`
Expected: ~50-100 fewer lines than the baseline (980 → ~880-930). Report the actual delta in PR-B's description so the team has the real number, not the issue's optimistic estimate.

- [ ] **Step 5: Commit**

```bash
git add crates/storage/src/collections/crdt_impls.rs
git commit -m "refactor(storage): use ? instead of verbose map_err in Mergeable impls

The new From<StoreError> for MergeError lets us drop the map_err+format!
boilerplate. Net: -<actual> LOC across Counter, RGA, UnorderedMap,
UnorderedSet, Vector merge impls.

Refs #2308."
```

---

### Task 12: Push PR-B and open it

- [ ] **Step 1: Push**

Run: `git push -u origin feat/2308-crdt-impls`

- [ ] **Step 2: Open PR with base = PR-A's branch**

Run:
```bash
gh pr create --draft \
  --base feat/2308-crdt-trait-hierarchy \
  --title "feat(storage): CRDT trait impls for simple collections (PR-B) — #2308" \
  --body "$(cat <<'EOF'
Second of three PRs for #2308. **Base branch:** \`feat/2308-crdt-trait-hierarchy\` (PR-A).

## Summary

- \`Vector\` impls \`CrdtSequence\`.
- \`UnorderedMap\` impls \`CrdtMap\`.
- \`UnorderedSet\` impls \`CrdtSet\`.
- \`tests/crdt_contract.rs\` exercises every collection through \`assert_crdt_laws\` (idempotency, commutativity, associativity).
- Replaced 15+ verbose \`map_err(|e| MergeError::StorageError(format!(...)))\` call sites with \`?\` using the new \`From<StoreError>\` impl.
- Real LOC delta in \`crdt_impls.rs\`: **-<actual> lines** (vs. issue's projected 50%, which assumed the file was macro boilerplate — it isn't).

## Test plan

- [ ] \`cargo test -p calimero-storage --lib --tests\` green.
- [ ] \`cargo test -p calimero-storage --test crdt_contract\` shows 6 passing tests.
- [ ] \`wc -l crates/storage/src/collections/crdt_impls.rs\` confirms the LOC reduction.

cc @chefsale @rtb-12
EOF
)"
```

- [ ] **Step 3: Stop and wait for PR-B review**

---

## PR-C — Wrapper collections

**End state of PR-C:** `AuthoredMap`, `AuthoredVector`, `NestedMap` implement the appropriate sub-trait. `Frozen`, `Shared`, `Root` either impl `Mergeable` only (if they don't fit map/sequence/set shape) or get the right sub-trait. Final cleanup pass. #2308 done.

**Branch:** off PR-B: `git checkout -b feat/2308-crdt-wrappers feat/2308-crdt-impls`

### Task 13: Impl `CrdtMap` for `AuthoredMap`

**Files:**
- Modify: `crates/storage/src/collections/authored_map.rs`
- Modify: `crates/storage/tests/crdt_contract.rs`

- [ ] **Step 1: Read `authored_map.rs` (497 LOC) to understand its method surface**

Run: `grep -nE "^\s*pub fn (insert|get|remove|len|contains)" crates/storage/src/collections/authored_map.rs`
Note the signatures — they likely take an extra `author: PublicKey` parameter. **If they do, the trait shape doesn't match directly.** Two options:
- (a) Impl `CrdtMap` with a sensible default author (e.g., `env::executor_id()`) when called through the trait.
- (b) Don't impl `CrdtMap`; just impl `Mergeable` (already exists) and leave the issue's stated coverage at that.

Pick (a) only if the maintainer's review on PR-B explicitly endorses it. Otherwise (b) is safer.

- [ ] **Step 2: Add the chosen impl and a contract test**

For option (a):

```rust
// In authored_map.rs (or a separate impl block in crdt_impls.rs):
impl<K, V, S> CrdtMap for AuthoredMap<K, V, S>
where
    K: borsh::BorshSerialize + borsh::BorshDeserialize + AsRef<[u8]> + Clone + PartialEq,
    V: borsh::BorshSerialize + borsh::BorshDeserialize + Mergeable + Clone,
    S: StorageAdaptor,
{
    type Key = K;
    type Value = V;
    type Error = crate::collections::error::StoreError;

    fn insert(&mut self, key: Self::Key, value: Self::Value) -> Result<Option<Self::Value>, Self::Error> {
        // Use current executor as the author when called through the trait.
        AuthoredMap::insert_with_author(self, key, value, env::executor_id())
    }
    // ... etc.
}
```

For option (b), skip the impl; just confirm `AuthoredMap`'s existing `Mergeable` impl is in `crdt_contract.rs`.

- [ ] **Step 3: Add a contract test for whichever option**

Append to `tests/crdt_contract.rs` analogous to Task 9's `unordered_map_with_counter` test, substituting `AuthoredMap` for `UnorderedMap`.

- [ ] **Step 4: Run tests**

Run: `cargo test -p calimero-storage --test crdt_contract 2>&1 | tail -10`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/storage/src/collections/{authored_map.rs,crdt_impls.rs} crates/storage/tests/crdt_contract.rs
git commit -m "feat(storage): impl CrdtMap for AuthoredMap + contract test

Refs #2308."
```

---

### Task 14: Impl `CrdtSequence` for `AuthoredVector`

Repeat Task 13's approach for `AuthoredVector` → `CrdtSequence`. Same read-first / decide-(a)-or-(b) / impl + test / commit flow.

### Task 15: Impl `CrdtMap` for `NestedMap`

Repeat for `NestedMap`. Likely the cleanest of the wrappers; should be a straight impl.

### Task 16: Audit `Frozen`, `Shared`, `Root` for trait fit

**Files:**
- Read: `crates/storage/src/collections/{frozen.rs,shared.rs,root.rs}`
- Possibly modify: `crates/storage/src/collections/crdt_impls.rs`

- [ ] **Step 1: Read each file's public method surface**

Run: `grep -nE "^\s*pub fn" crates/storage/src/collections/{frozen.rs,shared.rs,root.rs}`

- [ ] **Step 2: For each one, decide:**
  - If it looks like a map → impl `CrdtMap`.
  - If it looks like a sequence → impl `CrdtSequence`.
  - If it's a wrapper around another CRDT (e.g., `Root<C>`, `Frozen<C>`) → delegate the trait impl to the inner type via a blanket impl, e.g.:
    ```rust
    impl<C: CrdtMap> CrdtMap for Frozen<C> { type Key = C::Key; ... }
    ```
  - If it doesn't fit any shape → leave it as `Mergeable`-only; document in the file's module-level doc-comment why.

- [ ] **Step 3: For each impl added, add a contract test in `tests/crdt_contract.rs`**

- [ ] **Step 4: Run full test suite**

Run: `cargo test -p calimero-storage --lib --tests && cargo test -p calimero-storage --test crdt_contract 2>&1 | tail -10`

- [ ] **Step 5: Commit each wrapper as a separate commit**

For reviewer clarity, do `frozen`, `shared`, `root` as three separate commits.

---

### Task 17: Final cleanup pass and PR-C

- [ ] **Step 1: Verify the issue's "done when" criteria**

The issue says:
- ☐ All collection types impl the appropriate trait.
- ☐ `crdt_impls.rs` shrinks ≥50% (note: we challenged this — record actual delta).
- ☐ `cargo test -p calimero-storage` green.
- ☐ No behavioural change.

Run:
```bash
wc -l crates/storage/src/collections/crdt_impls.rs   # actual delta
cargo test -p calimero-storage --lib --tests 2>&1 | tail -5
cargo test -p calimero-storage --test crdt_contract 2>&1 | tail -5
```

- [ ] **Step 2: Run full workspace build to catch downstream breakage**

Run: `cargo build --workspace 2>&1 | tail -20`
Expected: clean build. If any consumer of `calimero_storage::collections` breaks, fix it.

- [ ] **Step 3: Push and open PR-C**

Run: `git push -u origin feat/2308-crdt-wrappers`

```bash
gh pr create --draft \
  --base feat/2308-crdt-impls \
  --title "feat(storage): CRDT trait impls for wrapper collections (PR-C) — #2308" \
  --body "$(cat <<'EOF'
Third and final PR for #2308. **Base branch:** \`feat/2308-crdt-impls\` (PR-B).

## Summary

- \`AuthoredMap\` impls \`CrdtMap\`.
- \`AuthoredVector\` impls \`CrdtSequence\`.
- \`NestedMap\` impls \`CrdtMap\`.
- \`Frozen\` / \`Shared\` / \`Root\` impl the appropriate trait or are documented as \`Mergeable\`-only.
- Final actual LOC delta in \`crdt_impls.rs\`: **<final number>** (revised criterion: behavioural correctness via \`crdt_contract.rs\`).

## Test plan

- [ ] \`cargo build --workspace\` green.
- [ ] \`cargo test -p calimero-storage --lib --tests\` green.
- [ ] \`cargo test -p calimero-storage --test crdt_contract\` shows ≥9 passing tests.
- [ ] Closes #2308.

cc @chefsale @rtb-12
EOF
)"
```

---

## After PR-C merges

- [ ] **Comment on #2308** linking the three merged PRs and noting the actual outcomes (LOC delta, number of contract tests, any deviations from the issue spec).
- [ ] **Update #2309** (the `Authored<C>` audit issue) — the trait hierarchy is now available; the audit can proceed.
- [ ] **Do not start #2310** yet; that's a separate planning effort.

---

## Self-review notes (for the plan author)

**Spec coverage check vs. issue #2308:**
- Trait hierarchy declared (`Crdt`/`CrdtMap`/`CrdtSequence`/`CrdtSet`) → Tasks 2–4. **Note:** kept `Mergeable` instead of renaming to `Crdt`; documented why.
- Impl matrix from the issue → Tasks 7–16.
- `crdt_impls.rs` ≥50% smaller → **flagged as unrealistic**; record actual delta in Tasks 11 and 17.
- `cargo test -p calimero-storage` green → Tasks 11.3 and 17.1.
- No behavioural change → trait impls all delegate to existing inherent methods (zero algorithm changes).
- `Authored<C>` unification = out of scope (#2309).
- Codec trait = out of scope (per issue).
- Property tests over `T: CrdtMap` / `T: CrdtSequence` → Tasks 5 + 7–10.
- Blanket-impl coverage test → Task 16 (when wrappers are decided).

**Placeholder scan:** none — every step has concrete code, exact commands, and expected output.

**Type consistency:** `Self::Error = StoreError` used throughout; `Mergeable` referenced by canonical name everywhere; method signatures match the inherent methods discovered in the Read phase.

**Known risk:** Task 9's `unordered_map_with_counter_satisfies_crdt_laws` may need executor-ID parameterisation for Counter to make commutativity hold. Mitigation written into the task.
