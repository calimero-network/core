//! In-memory awareness store: LWW-by-`(author, seq)` with TTL expiry.
//!
//! **Deliberately import-free of network, storage, and actix.** This is the
//! structural "never persisted" boundary — a type with no RocksDB access
//! cannot write to disk. Tasks 6 (inbound) and 7 (outbound) drive this.
//!
//! Time is always passed in as `now_ms`; no wall-clock calls exist here so
//! the type is fully deterministic and unit-testable in isolation.

use std::collections::{BTreeMap, HashMap};

use calimero_primitives::context::ContextId;
use calimero_primitives::identity::PublicKey;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// A single diff produced by a mutating operation on the store.
#[derive(Debug, PartialEq)]
pub enum Diff {
    Upsert { author: PublicKey, slice: Vec<u8> },
    Remove { author: PublicKey },
}

// ---------------------------------------------------------------------------
// Internal types
// ---------------------------------------------------------------------------

#[derive(Debug)]
struct Entry {
    slice: Vec<u8>,
    seq: u64,
    last_seen_ms: u64,
}

// ---------------------------------------------------------------------------
// AwarenessStore
// ---------------------------------------------------------------------------

/// Per-context map of `author -> {slice, seq, last_seen_ms}`.
#[derive(Debug)]
pub struct AwarenessStore {
    // Outer: context → per-author entries.
    // Inner: BTreeMap so PublicKey (which is Ord but not std::hash::Hash)
    // can be used as a key, and iteration order is deterministic (sorted by
    // author bytes), which `snapshot` exploits for free.
    inner: HashMap<ContextId, BTreeMap<PublicKey, Entry>>,
}

impl AwarenessStore {
    /// Create an empty store.
    pub fn new() -> Self {
        Self {
            inner: HashMap::new(),
        }
    }

    /// Apply an incoming awareness slice for `author` in `ctx`.
    ///
    /// LWW rule: if an entry with `seq >= incoming_seq` already exists the
    /// call is a no-op and returns `None`.
    ///
    /// On accept the entry is updated. Returns `Some(Diff::Upsert)` **only**
    /// if the live slice bytes changed; a same-bytes re-apply with a higher
    /// `seq` updates liveness but returns `None` (no visible diff).
    pub fn apply(
        &mut self,
        ctx: ContextId,
        author: PublicKey,
        seq: u64,
        slice: Vec<u8>,
        now_ms: u64,
    ) -> Option<Diff> {
        let per_ctx = self.inner.entry(ctx).or_default();

        if let Some(entry) = per_ctx.get_mut(&author) {
            // Stale or equal seq → no-op.
            if seq <= entry.seq {
                return None;
            }
            let slice_changed = entry.slice != slice;
            entry.seq = seq;
            entry.last_seen_ms = now_ms;
            if slice_changed {
                entry.slice = slice.clone();
                return Some(Diff::Upsert { author, slice });
            }
            // Same bytes, higher seq → liveness updated, no diff.
            return None;
        }

        // New entry.
        per_ctx.insert(
            author,
            Entry {
                slice: slice.clone(),
                seq,
                last_seen_ms: now_ms,
            },
        );
        Some(Diff::Upsert { author, slice })
    }

    /// Refresh liveness for `author` in `ctx` without changing the slice.
    ///
    /// Used as a heartbeat when the local slice has not changed. A missing
    /// entry is silently ignored (the peer may already have been swept).
    pub fn touch(&mut self, ctx: ContextId, author: PublicKey, now_ms: u64) {
        if let Some(per_ctx) = self.inner.get_mut(&ctx) {
            if let Some(entry) = per_ctx.get_mut(&author) {
                entry.last_seen_ms = now_ms;
            }
        }
    }

    /// Drop entries for `ctx` whose `last_seen_ms` is older than `ttl_ms`
    /// relative to `now_ms`. Returns one `Diff::Remove` per dropped entry.
    pub fn sweep(&mut self, ctx: ContextId, ttl_ms: u64, now_ms: u64) -> Vec<Diff> {
        let Some(per_ctx) = self.inner.get_mut(&ctx) else {
            return vec![];
        };

        let mut removals = Vec::new();
        per_ctx.retain(|author, entry| {
            if now_ms.saturating_sub(entry.last_seen_ms) >= ttl_ms {
                removals.push(Diff::Remove { author: *author });
                false
            } else {
                true
            }
        });
        removals
    }

    /// Snapshot of live slices for `ctx`, sorted by author bytes (stable order).
    ///
    /// Returns an empty `Vec` when the context has no entries.
    pub fn snapshot(&self, ctx: ContextId) -> Vec<(PublicKey, Vec<u8>)> {
        let Some(per_ctx) = self.inner.get(&ctx) else {
            return vec![];
        };
        // BTreeMap already iterates in sorted (author-bytes) order.
        per_ctx
            .iter()
            .map(|(author, entry)| (*author, entry.slice.clone()))
            .collect()
    }

    /// Explicitly remove `author` from `ctx` (e.g. on disconnect).
    ///
    /// Returns `Some(Diff::Remove)` if the author was present, `None` otherwise.
    pub fn remove_author(&mut self, ctx: ContextId, author: PublicKey) -> Option<Diff> {
        let per_ctx = self.inner.get_mut(&ctx)?;
        per_ctx.remove(&author).map(|_| Diff::Remove { author })
    }
}

impl Default for AwarenessStore {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx() -> ContextId {
        ContextId::from([1u8; 32])
    }
    fn pk(n: u8) -> PublicKey {
        PublicKey::from([n; 32])
    }

    #[test]
    fn apply_then_snapshot() {
        let mut s = AwarenessStore::new();
        assert_eq!(
            s.apply(ctx(), pk(1), 1, vec![1], 1000),
            Some(Diff::Upsert {
                author: pk(1),
                slice: vec![1]
            })
        );
        assert_eq!(s.snapshot(ctx()), vec![(pk(1), vec![1])]);
    }

    #[test]
    fn lww_ignores_stale_or_equal_seq() {
        let mut s = AwarenessStore::new();
        s.apply(ctx(), pk(1), 5, vec![5], 1000);
        assert!(s.apply(ctx(), pk(1), 4, vec![4], 1001).is_none()); // stale
        assert!(s.apply(ctx(), pk(1), 5, vec![9], 1002).is_none()); // equal seq
        assert_eq!(s.snapshot(ctx()), vec![(pk(1), vec![5])]);
    }

    #[test]
    fn sweep_expires_and_diffs() {
        let mut s = AwarenessStore::new();
        s.apply(ctx(), pk(1), 1, vec![1], 1000);
        assert!(s.sweep(ctx(), 7000, 5000).is_empty()); // still fresh
        assert_eq!(
            s.sweep(ctx(), 7000, 9000),
            vec![Diff::Remove { author: pk(1) }]
        ); // expired
        assert!(s.snapshot(ctx()).is_empty());
    }

    #[test]
    fn touch_extends_liveness() {
        let mut s = AwarenessStore::new();
        s.apply(ctx(), pk(1), 1, vec![1], 1000);
        s.touch(ctx(), pk(1), 6000);
        assert!(s.sweep(ctx(), 7000, 9000).is_empty()); // touched at 6000 → not expired at 9000
    }

    #[test]
    fn remove_author_diffs_once() {
        let mut s = AwarenessStore::new();
        s.apply(ctx(), pk(1), 1, vec![1], 1000);
        assert_eq!(
            s.remove_author(ctx(), pk(1)),
            Some(Diff::Remove { author: pk(1) })
        );
        assert!(s.remove_author(ctx(), pk(1)).is_none());
    }
}
