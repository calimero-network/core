//! A single size-capped in-memory cache abstraction shared by every hot cache
//! `ContextManager` keeps (`contexts`, `applications`, `modules`,
//! `namespace_dags`).
//!
//! Before this module each cache hand-rolled its own cap + eviction: two
//! free functions (`evict_idle_context_if_full`, `evict_application_if_full`),
//! an inline `pop_first` in the execute handler, and — for `namespace_dags` —
//! nothing at all (it grew unbounded; see the `namespace_dags` issue this
//! consolidation closes). The three live implementations only ever differed on
//! one axis: whether an entry may be evicted while a caller holds a live handle
//! to it. That axis is captured by [`Evictable`]; everything else (the cap, the
//! fetch-before-evict insert discipline, the "all entries live → defer and
//! stay over-cap" behaviour) lives here once.
//!
//! The datastore is always authoritative, so an evicted entry costs at most a
//! re-fetch on next access — the cap is a safety valve against unbounded growth
//! on long-running nodes, not a correctness boundary.

use std::collections::{btree_map, BTreeMap};
use std::fmt::Debug;

/// Whether a cached value may be dropped right now.
///
/// The default (`true`) suits a *plain* cache whose values are pure clones of
/// datastore state — evicting one is always safe. A cache whose entries hand
/// out a live handle (e.g. an `Arc<Mutex<_>>` guard held across an async
/// operation) overrides this so the entry is only evictable once no caller
/// holds that handle; see the `ContextMeta` / namespace-DAG impls in `lib.rs`
/// for why evicting a live entry would let two operations serialize on
/// different mutexes and corrupt state.
pub(crate) trait Evictable {
    /// `true` when nothing outside the cache references this entry and it is
    /// safe to evict. Always `true` for plain caches.
    fn is_idle(&self) -> bool {
        true
    }
}

/// A `BTreeMap`-backed cache bounded to `cap` entries.
///
/// Eviction runs only just before inserting a *new* key (re-touching an
/// existing key never evicts) and removes the lowest-keyed [`Evictable::is_idle`]
/// entry. Victim selection is by key order, not true LRU — deliberately:
/// `ContextId`/`ApplicationId` keys are hashes, so "lowest key" is effectively
/// arbitrary, and a wrong guess only costs a re-fetch. Upgrading to LRU is a
/// possible follow-up if profiling ever shows churn on a hot entry.
#[derive(Debug)]
pub(crate) struct BoundedCache<K, V> {
    map: BTreeMap<K, V>,
    cap: usize,
    /// Stable label for tracing (`contexts`, `applications`, …).
    name: &'static str,
}

impl<K: Ord + Clone + Debug, V: Evictable> BoundedCache<K, V> {
    /// A new empty cache holding at most `cap` entries, tagged `name` in logs.
    pub(crate) fn new(cap: usize, name: &'static str) -> Self {
        Self {
            map: BTreeMap::new(),
            cap,
            name,
        }
    }

    pub(crate) fn len(&self) -> usize {
        self.map.len()
    }

    pub(crate) fn contains_key(&self, key: &K) -> bool {
        self.map.contains_key(key)
    }

    pub(crate) fn get(&self, key: &K) -> Option<&V> {
        self.map.get(key)
    }

    pub(crate) fn get_mut(&mut self, key: &K) -> Option<&mut V> {
        self.map.get_mut(key)
    }

    pub(crate) fn remove(&mut self, key: &K) -> Option<V> {
        self.map.remove(key)
    }

    /// Raw entry access, for the one caller (`create_context`) that needs a
    /// `VacantEntry` to satisfy a borrow-checker workaround.
    ///
    /// Caps automatically: a *new* key evicts one idle entry first (so this
    /// path can't grow the cache past cap), while re-accessing an existing key
    /// never evicts — matching [`Self::insert_new`] / [`Self::get_or_insert_with`].
    /// The cap is therefore enforced here, not by a caller convention.
    pub(crate) fn entry(&mut self, key: K) -> btree_map::Entry<'_, K, V> {
        if !self.map.contains_key(&key) {
            self.evict_if_full();
        }
        self.map.entry(key)
    }

    /// Evict one idle entry if the cache is at capacity; a no-op below it.
    ///
    /// If every entry is live nothing is evicted and the cache is allowed to
    /// sit over `cap` until in-flight work finishes — a legitimate designed
    /// state for lock-gated caches (it self-corrects as each later insert frees
    /// one slot). Plain caches always find a victim, so they never reach this.
    pub(crate) fn evict_if_full(&mut self) {
        if self.map.len() < self.cap {
            return;
        }

        let victim = self
            .map
            .iter()
            .find(|(_, value)| value.is_idle())
            .map(|(key, _)| key.clone());

        let Some(key) = victim else {
            // Over-cap with every entry live. Surface a *significant* overage
            // (sustained high concurrency) at warn; a small one is routine.
            let len = self.map.len();
            if len > self.cap + self.cap / 10 {
                tracing::warn!(
                    cache = self.name,
                    cap = self.cap,
                    len,
                    "cache significantly over capacity and all entries live; \
                     deferring eviction (sustained high concurrency?)"
                );
            } else {
                tracing::debug!(
                    cache = self.name,
                    cap = self.cap,
                    len,
                    "cache at capacity but all entries live; deferring eviction"
                );
            }
            return;
        };

        let _ = self.map.remove(&key);
        tracing::debug!(
            cache = self.name,
            ?key,
            "evicted idle cache entry (at capacity)"
        );
    }

    /// Insert `value` at `key`, capping the cache. Replacing an existing key
    /// never evicts (it isn't a new entry); inserting a new key evicts one idle
    /// entry first when at capacity. Returns the previous value, if any.
    ///
    /// Use this when the key may already be present (e.g. overwriting a
    /// recompiled module); prefer [`Self::insert_new`] when novelty is
    /// guaranteed and a returned `&mut V` is convenient.
    pub(crate) fn insert(&mut self, key: K, value: V) -> Option<V> {
        if !self.map.contains_key(&key) {
            self.evict_if_full();
        }
        self.map.insert(key, value)
    }

    /// Insert a brand-new key, evicting one idle entry first if at capacity.
    ///
    /// The caller must have already confirmed `key` is absent (so a re-touch
    /// never evicts, and the key about to be inserted is never itself the
    /// victim). Returns a mutable reference to the inserted value.
    pub(crate) fn insert_new(&mut self, key: K, value: V) -> &mut V {
        debug_assert!(
            !self.map.contains_key(&key),
            "insert_new called with a key already present; caller must guard on absence"
        );
        self.evict_if_full();
        self.map.entry(key).or_insert(value)
    }

    /// Return the value for `key`, computing and inserting it (capped) on a
    /// miss. `make` runs only on a miss, after any eviction.
    pub(crate) fn get_or_insert_with(&mut self, key: K, make: impl FnOnce() -> V) -> &mut V {
        // Hit: return the existing value. Miss: evict-if-full, then insert via
        // the entry API so the value is materialised in a single lookup with
        // the key moved (not cloned). `evict_if_full` runs before `entry()`, so
        // the entry it returns is always vacant for `key`.
        if self.map.contains_key(&key) {
            return self.map.get_mut(&key).expect("checked present");
        }
        self.evict_if_full();
        self.map.entry(key).or_insert_with(make)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;

    /// A value carrying an `Arc` so a test can hold a clone to keep it "live"
    /// (`strong_count > 1`), exercising the lock-gated eviction path.
    #[derive(Clone)]
    struct Lockable(Arc<()>);

    impl Lockable {
        fn idle() -> Self {
            Self(Arc::new(()))
        }
    }

    impl Evictable for Lockable {
        fn is_idle(&self) -> bool {
            Arc::strong_count(&self.0) == 1
        }
    }

    /// A plain value that is always evictable (the default impl).
    #[derive(Clone)]
    struct Plain(u32);
    impl Evictable for Plain {}

    #[test]
    fn no_eviction_below_cap() {
        let mut cache: BoundedCache<u32, Plain> = BoundedCache::new(4, "plain");
        for i in 0..3 {
            let _ = cache.insert_new(i, Plain(i));
        }
        assert_eq!(cache.len(), 3);
    }

    #[test]
    fn plain_cache_evicts_lowest_key_at_cap() {
        let mut cache: BoundedCache<u32, Plain> = BoundedCache::new(4, "plain");
        for i in 0..4 {
            let _ = cache.insert_new(i, Plain(i));
        }
        // At cap; inserting a 5th evicts the lowest key (0).
        let _ = cache.insert_new(99, Plain(99));
        assert_eq!(cache.len(), 4);
        assert!(!cache.contains_key(&0));
        assert!(cache.contains_key(&99));
    }

    #[test]
    fn lock_gated_cache_never_evicts_a_live_entry() {
        let mut cache: BoundedCache<u32, Lockable> = BoundedCache::new(4, "locked");

        // Key 0 is idle; keys 1..4 are live (held by `guards`).
        let _ = cache.insert_new(0, Lockable::idle());
        let mut guards = Vec::new();
        for i in 1..4 {
            let v = Lockable::idle();
            guards.push(v.clone()); // bump strong_count → live
            let _ = cache.insert_new(i, v);
        }

        // At cap; the next insert must evict the single idle entry (0), never
        // a live one.
        let _ = cache.insert_new(50, Lockable::idle());
        assert_eq!(cache.len(), 4);
        assert!(
            !cache.contains_key(&0),
            "idle entry should have been evicted"
        );
        for i in 1..4 {
            assert!(cache.contains_key(&i), "live entry {i} was wrongly evicted");
        }
        drop(guards);
    }

    #[test]
    fn lock_gated_cache_defers_when_all_entries_live() {
        let mut cache: BoundedCache<u32, Lockable> = BoundedCache::new(4, "locked");
        let mut guards = Vec::new();
        for i in 0..4 {
            let v = Lockable::idle();
            guards.push(v.clone());
            let _ = cache.insert_new(i, v);
        }
        // Cap reached, nothing evictable: evict_if_full is a no-op rather than
        // dropping a live entry.
        cache.evict_if_full();
        assert_eq!(cache.len(), 4);
        drop(guards);
    }

    #[test]
    fn get_or_insert_with_runs_make_only_on_miss() {
        let mut cache: BoundedCache<u32, Plain> = BoundedCache::new(4, "plain");
        let _ = cache.get_or_insert_with(7, || Plain(7));
        // Hit: closure must not run (would panic if it did).
        let v = cache.get_or_insert_with(7, || panic!("make ran on a hit"));
        assert_eq!(v.0, 7);
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn retouching_existing_key_never_evicts() {
        let mut cache: BoundedCache<u32, Plain> = BoundedCache::new(4, "plain");
        for i in 0..4 {
            let _ = cache.insert_new(i, Plain(i));
        }
        // Re-touch an existing key at cap: no eviction, no growth.
        let _ = cache.get_or_insert_with(0, || panic!("make ran on a hit"));
        assert_eq!(cache.len(), 4);
        for i in 0..4 {
            assert!(cache.contains_key(&i));
        }
    }

    #[test]
    fn entry_caps_on_new_key_but_not_on_existing() {
        let mut cache: BoundedCache<u32, Plain> = BoundedCache::new(4, "plain");
        for i in 0..4 {
            let _ = cache.insert_new(i, Plain(i));
        }
        // entry() for a NEW key at cap must evict first (key 0, the lowest),
        // holding at cap — the cap is enforced by entry() itself.
        if let btree_map::Entry::Vacant(e) = cache.entry(99) {
            let _ = e.insert(Plain(99));
        }
        assert_eq!(cache.len(), 4);
        assert!(cache.contains_key(&99));
        assert!(!cache.contains_key(&0), "lowest idle key should be evicted");

        // entry() for an EXISTING key must NOT evict (re-access, not growth).
        let _ = cache.entry(99);
        assert_eq!(cache.len(), 4);
        assert!(cache.contains_key(&99));
    }
}
