//! Built-in `migration_check` invariant helpers.
//!
//! These are small, cheap, high-signal predicates an app author composes inside
//! their [`#[app::migration_check]`](crate::app::migration_check) body to
//! health-check a produced v2 root **before** it is committed. A `false` verdict
//! lets the runtime logically abort the migration, leaving the still-untouched
//! v1 root intact (see PR-6d / spec §3 decision 10).
//!
//! They are **pure** functions over already-deserialized application values —
//! they make no host call (no `read_raw`, no `env::*`), so they run identically
//! in the wasm migration-check export and in native unit tests. The author has
//! already deserialized `old` and `new` (the macro does it for them), then calls
//! these on the in-memory collections:
//!
//! ```ignore
//! #[app::migration_check]
//! fn check(old: AppV1, new: AppV2) -> bool {
//!     use calimero_sdk::migration_check::*;
//!     entity_count_parity(&old.users, &new.users, 0)
//!         && no_orphaned_refs(
//!             new.posts.iter().map(|p| p.author_id),
//!             new.users.iter().map(|u| u.id),
//!         )
//!         && conservation(old.total_supply(), new.total_supply())
//! }
//! ```
//!
//! Each helper is generic so it composes over whatever shape the app uses —
//! a `Vec`, a `HashMap`, a `HashSet`, a summed scalar. [`entity_count_parity`]
//! counts a collection (not a bare iterator); the ref/key helpers take any
//! [`IntoIterator`], so a CRDT collection's iterator works there directly.

use std::collections::HashSet;
use std::hash::Hash;

/// Something whose number of entities can be counted.
///
/// Implemented for the common standard collections (`Vec`, slices, `HashMap` /
/// `BTreeMap`, `HashSet` / `BTreeSet`) and shared references to them, so an
/// author can pass `&old.users` directly. It is **not** implemented for bare
/// iterators — pass the collection itself, or `&iter.collect::<Vec<_>>()` when
/// you only have an iterator.
///
/// ```
/// use calimero_sdk::migration_check::entity_count_parity;
///
/// // Pass the collection directly...
/// let users = vec![1u32, 2, 3];
/// assert!(entity_count_parity(&users, &users, 0));
///
/// // ...or collect an iterator into a `Vec` first.
/// let mapped: Vec<_> = users.iter().map(|n| n * 2).collect();
/// assert!(entity_count_parity(&users, &mapped, 0));
/// ```
pub trait Count {
    /// The number of entities.
    fn count(&self) -> usize;
}

impl<T> Count for [T] {
    fn count(&self) -> usize {
        self.len()
    }
}

impl<T> Count for Vec<T> {
    fn count(&self) -> usize {
        self.len()
    }
}

impl<K, V> Count for std::collections::HashMap<K, V> {
    fn count(&self) -> usize {
        self.len()
    }
}

impl<K, V> Count for std::collections::BTreeMap<K, V> {
    fn count(&self) -> usize {
        self.len()
    }
}

impl<T> Count for HashSet<T> {
    fn count(&self) -> usize {
        self.len()
    }
}

impl<T> Count for std::collections::BTreeSet<T> {
    fn count(&self) -> usize {
        self.len()
    }
}

impl<T: Count + ?Sized> Count for &T {
    fn count(&self) -> usize {
        (**self).count()
    }
}

/// Returns `true` iff `old` and `new` hold the same number of entities, within
/// an allowed absolute `delta`.
///
/// A faithful 1:1 carry passes with `delta == 0`. A migration that silently
/// drops (or duplicates) entries fails — the cheapest, highest-signal lossiness
/// guard. Use a non-zero `delta` when the migration is *expected* to add or
/// remove a bounded number of entities (e.g. seeding one summary row).
///
/// ```
/// use calimero_sdk::migration_check::entity_count_parity;
///
/// let old = vec![1, 2, 3];
/// let new = vec![10, 20, 3];
/// assert!(entity_count_parity(&old, &new, 0));
///
/// let lossy = vec![10, 20];
/// assert!(!entity_count_parity(&old, &lossy, 0));
/// assert!(entity_count_parity(&old, &lossy, 1)); // within tolerance
/// ```
#[must_use]
pub fn entity_count_parity<C1: Count, C2: Count>(old: C1, new: C2, delta: usize) -> bool {
    old.count().abs_diff(new.count()) <= delta
}

/// Returns `true` iff every referenced child id is present in the key set —
/// i.e. there are no dangling foreign-key references in the produced v2 root.
///
/// `refs` yields the ids referenced by entities (e.g. each post's `author_id`);
/// `keys` yields the ids that actually exist (e.g. every user's `id`). A
/// migration that rewrites entities but loses a referenced parent leaves a
/// dangling pointer — this catches it before commit.
///
/// ```
/// use calimero_sdk::migration_check::no_orphaned_refs;
///
/// let user_ids = [1u32, 2, 3];
/// let post_authors = [1u32, 2, 2];
/// assert!(no_orphaned_refs(post_authors.iter().copied(), user_ids.iter().copied()));
///
/// let dangling = [1u32, 9];
/// assert!(!no_orphaned_refs(dangling.iter().copied(), user_ids.iter().copied()));
/// ```
#[must_use]
pub fn no_orphaned_refs<R, K>(refs: R, keys: K) -> bool
where
    R: IntoIterator,
    K: IntoIterator<Item = R::Item>,
    R::Item: Eq + Hash,
{
    let key_set: HashSet<R::Item> = keys.into_iter().collect();
    refs.into_iter().all(|r| key_set.contains(&r))
}

/// Returns `true` iff an app-computed conserved quantity is preserved across the
/// migration.
///
/// Wraps an equality of a summed/aggregated invariant the author computes on
/// each side (e.g. total token supply, summed balances, a row count the schema
/// guarantees). Off-by-one or rounding drift introduced by a lossy transform
/// fails here.
///
/// ```
/// use calimero_sdk::migration_check::conservation;
///
/// let old_total: u64 = 100;
/// let new_total: u64 = 100;
/// assert!(conservation(old_total, new_total));
/// assert!(!conservation(old_total, new_total + 1));
/// ```
#[must_use]
pub fn conservation<T: PartialEq>(old_total: T, new_total: T) -> bool {
    old_total == new_total
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    // A faithful migration: a v1 map carried 1:1 into a v2 map of the same size,
    // referential integrity intact, total conserved.
    #[derive(Clone)]
    struct AppV1 {
        users: HashMap<u32, u64>, // id -> balance
        posts: Vec<(u32, u32)>,   // (post_id, author_id)
    }

    impl AppV1 {
        fn seed() -> Self {
            let mut users = HashMap::new();
            let _ = users.insert(1, 40);
            let _ = users.insert(2, 60);
            AppV1 {
                users,
                posts: vec![(10, 1), (11, 2), (12, 2)],
            }
        }

        fn total_balance(&self) -> u64 {
            self.users.values().copied().sum()
        }
    }

    // ---- entity_count_parity --------------------------------------------

    #[test]
    fn entity_count_parity_passes_on_faithful_carry() {
        let old = AppV1::seed();
        // faithful: same users carried over.
        let new = old.clone();
        assert!(entity_count_parity(&old.users, &new.users, 0));
    }

    #[test]
    fn entity_count_parity_detects_dropped_entry() {
        let old = AppV1::seed();
        let mut new = old.clone();
        let _ = new.users.remove(&2); // drop one user
        assert!(
            !entity_count_parity(&old.users, &new.users, 0),
            "dropping an entry must fail strict parity"
        );
        assert!(
            entity_count_parity(&old.users, &new.users, 1),
            "a tolerated single-entry delta should pass"
        );
    }

    #[test]
    fn entity_count_parity_works_for_slices_and_iterators() {
        let old = [1, 2, 3];
        let new = vec![9, 8, 7];
        assert!(entity_count_parity(&old[..], &new, 0));
        // An iterator has no `Count` impl; collect it into a `Vec` first.
        let mapped: Vec<_> = old.iter().map(|n| n * 2).collect();
        assert!(entity_count_parity(&old[..], &mapped, 0));
    }

    // ---- no_orphaned_refs -----------------------------------------------

    #[test]
    fn no_orphaned_refs_passes_when_all_resolve() {
        let app = AppV1::seed();
        let refs = app.posts.iter().map(|(_, author)| *author);
        let keys = app.users.keys().copied();
        assert!(no_orphaned_refs(refs, keys));
    }

    #[test]
    fn no_orphaned_refs_detects_dangling_reference() {
        let mut app = AppV1::seed();
        // a v2 transform that dropped user 2 but kept a post pointing at it.
        let _ = app.users.remove(&2);
        let refs = app.posts.iter().map(|(_, author)| *author);
        let keys = app.users.keys().copied();
        assert!(
            !no_orphaned_refs(refs, keys),
            "a post referencing a removed user must fail"
        );
    }

    // ---- conservation ----------------------------------------------------

    #[test]
    fn conservation_passes_when_total_preserved() {
        let old = AppV1::seed();
        let new = old.clone();
        assert!(conservation(old.total_balance(), new.total_balance()));
    }

    #[test]
    fn conservation_detects_broken_invariant() {
        let old = AppV1::seed();
        let mut new = old.clone();
        // a lossy transform that shaved one off a balance.
        if let Some(b) = new.users.get_mut(&1) {
            *b -= 1;
        }
        assert!(
            !conservation(old.total_balance(), new.total_balance()),
            "an off-by-one in the conserved total must fail"
        );
    }

    // ---- composition: the way an author writes their check ---------------

    #[test]
    fn helpers_compose_into_a_single_predicate() {
        let old = AppV1::seed();

        let faithful = old.clone();
        let pass = entity_count_parity(&old.users, &faithful.users, 0)
            && no_orphaned_refs(
                faithful.posts.iter().map(|(_, a)| *a),
                faithful.users.keys().copied(),
            )
            && conservation(old.total_balance(), faithful.total_balance());
        assert!(pass, "a faithful migration must satisfy every invariant");

        let mut lossy = old.clone();
        let _ = lossy.users.remove(&2);
        let fail = entity_count_parity(&old.users, &lossy.users, 0)
            && no_orphaned_refs(
                lossy.posts.iter().map(|(_, a)| *a),
                lossy.users.keys().copied(),
            )
            && conservation(old.total_balance(), lossy.total_balance());
        assert!(!fail, "a lossy migration must fail at least one invariant");
    }
}
