//! `ChildTrie`: a parent's children, stored as a hash trie instead of one blob.
//!
//! # Why this exists
//!
//! [`EntityIndex`](crate::index::EntityIndex) used to hold a parent's whole child
//! list inline as `Vec<ChildInfo>`, and the parent's hash was a SHA-256 fold over
//! every child. Linking one child therefore read, rewrote and re-hashed all of
//! them. Measured at 200 children that is a 16,927-byte row and 200 hash inputs
//! per link; at the ~1,600 children a chat context reached, ~135 KB rewritten
//! four times per message sent. Past a few hundred children a single write
//! exhausts the runtime gas limit and the parent becomes permanently unwritable
//! (core#3602).
//!
//! The hash chain itself was never wrong — it is a correct recursive Merkle
//! construction. What was missing is any structure *between* a parent and its
//! children: fan-out was unbounded and depth was whatever the app's nesting
//! happened to be, so none of the logarithmic properties a Merkle tree is
//! actually used for held. Updates folded every sibling, diffs scanned every
//! sibling, and an inclusion proof needed all of them.
//!
//! # Shape
//!
//! A sparse hex trie keyed by child id, [`DEPTH`] nibbles deep, with bucketed
//! leaves:
//!
//! ```text
//! level 0   node(path=[])          slots: nibble -> subtree hash
//! level 1   node(path=[n0])        slots: nibble -> subtree hash
//! ...
//! level D   bucket(path=[n0..nD])  the ChildInfos whose id starts with that path
//! ```
//!
//! Writing one child touches `DEPTH + 1` rows, whatever the parent's size.
//!
//! Nodes are **sparse** — a node stores only its occupied slots — so a parent
//! with three children costs three small rows, not a fixed 16-way fan-out. That
//! keeps the common case (a record with a handful of nested collections) cheaper
//! than the blob it replaces, rather than trading small-parent cost for
//! large-parent cost.
//!
//! # Why keyed by id, and not an append-order accumulator
//!
//! The root must be a function of the child *set*, never of the order the
//! children arrived. Two replicas learn about the same children in different
//! orders all the time; if the root depended on that order they would never
//! converge. An MMR-style accumulator is append-ordered and cannot be used here
//! for exactly that reason. A trie keyed by child id is canonical: the position
//! of a child is determined by its id alone.

use borsh::{to_vec, BorshDeserialize, BorshSerialize};
use sha2::{Digest, Sha256};

use crate::address::Id;
use crate::entities::ChildInfo;
use crate::store::{Key, MainStorage, StorageAdaptor};

/// Nibbles of child id consumed before reaching a bucket.
///
/// Four gives 65,536 buckets, so a parent holding 10,000 children still has
/// buckets averaging well under one entry, and a write touches five rows.
/// Raising it makes large parents flatter at the cost of a deeper walk on every
/// write; lowering it makes buckets longer, and a bucket is the one part that is
/// folded linearly.
pub const DEPTH: usize = 4;

/// Hash of an absent subtree. Distinct from the hash of an empty node so that
/// "no children here" and "a node that exists but holds nothing" cannot collide.
pub const EMPTY: [u8; 32] = [0; 32];

const DOMAIN_NODE: &[u8] = b"childtrie:v1:node";
const DOMAIN_BUCKET: &[u8] = b"childtrie:v1:bucket";
const DOMAIN_ADDR: &[u8] = b"childtrie:v1:addr";

/// An interior node: occupied slots only, ascending by nibble.
#[derive(Clone, Debug, Default, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct TrieNode {
    /// `(nibble, subtree hash)`, sorted by nibble and at most 16 long.
    pub slots: Vec<(u8, [u8; 32])>,
}

impl TrieNode {
    fn set(&mut self, nibble: u8, hash: [u8; 32]) {
        match self.slots.binary_search_by_key(&nibble, |(n, _)| *n) {
            Ok(i) => {
                if hash == EMPTY {
                    let _ignored = self.slots.remove(i);
                } else {
                    self.slots[i].1 = hash;
                }
            }
            Err(i) => {
                if hash != EMPTY {
                    self.slots.insert(i, (nibble, hash));
                }
            }
        }
    }

    fn hash(&self) -> [u8; 32] {
        if self.slots.is_empty() {
            return EMPTY;
        }
        let mut hasher = Sha256::new();
        hasher.update(DOMAIN_NODE);
        for (nibble, hash) in &self.slots {
            hasher.update([*nibble]);
            hasher.update(hash);
        }
        hasher.finalize().into()
    }
}

/// A leaf bucket: the children whose id shares this path prefix.
#[derive(Clone, Debug, Default, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct TrieBucket {
    /// Children, sorted by id so the fold is canonical.
    pub entries: Vec<ChildInfo>,
}

impl TrieBucket {
    fn hash(&self) -> [u8; 32] {
        if self.entries.is_empty() {
            return EMPTY;
        }
        let mut hasher = Sha256::new();
        hasher.update(DOMAIN_BUCKET);
        for child in &self.entries {
            hasher.update(child.id().as_bytes());
            hasher.update(child.merkle_hash());
        }
        hasher.finalize().into()
    }
}

/// The `i`th nibble of `id`, high nibble first.
fn nibble(id: Id, i: usize) -> u8 {
    let byte = id.as_bytes()[i / 2];
    if i % 2 == 0 {
        byte >> 4
    } else {
        byte & 0x0f
    }
}

/// Storage address of the trie row for `parent` at `path`.
///
/// Domain-separated from entry and collection ids so a trie row can never
/// collide with an entity.
fn addr(parent: Id, path: &[u8]) -> Id {
    let mut hasher = Sha256::new();
    hasher.update(parent.as_bytes());
    hasher.update(DOMAIN_ADDR);
    hasher.update([path.len() as u8]);
    hasher.update(path);
    Id::new(hasher.finalize().into())
}

/// Per-parent child trie.
#[derive(Debug)]
pub struct ChildTrie<S: StorageAdaptor = MainStorage> {
    parent: Id,
    _phantom: core::marker::PhantomData<S>,
}

impl<S: StorageAdaptor> ChildTrie<S> {
    /// Bind to `parent`'s trie.
    #[must_use]
    pub const fn new(parent: Id) -> Self {
        Self {
            parent,
            _phantom: core::marker::PhantomData,
        }
    }

    fn read_node(&self, path: &[u8]) -> TrieNode {
        S::storage_read(Key::ChildTrie(addr(self.parent, path)))
            .and_then(|bytes| TrieNode::try_from_slice(&bytes).ok())
            .unwrap_or_default()
    }

    fn write_node(&self, path: &[u8], node: &TrieNode) {
        let key = Key::ChildTrie(addr(self.parent, path));
        if node.slots.is_empty() {
            let _ignored = S::storage_remove(key);
        } else if let Ok(bytes) = to_vec(node) {
            let _ignored = S::storage_write(key, &bytes);
        }
    }

    fn read_bucket(&self, path: &[u8]) -> TrieBucket {
        S::storage_read(Key::ChildTrie(addr(self.parent, path)))
            .and_then(|bytes| TrieBucket::try_from_slice(&bytes).ok())
            .unwrap_or_default()
    }

    fn write_bucket(&self, path: &[u8], bucket: &TrieBucket) {
        let key = Key::ChildTrie(addr(self.parent, path));
        if bucket.entries.is_empty() {
            let _ignored = S::storage_remove(key);
        } else if let Ok(bytes) = to_vec(bucket) {
            let _ignored = S::storage_write(key, &bytes);
        }
    }

    /// Recompute the spine above `path_of_child` after its bucket changed.
    ///
    /// Walks from the bucket back to the root, so the cost is `DEPTH` rows
    /// regardless of how many children the parent holds. This is the whole
    /// point of the structure.
    fn refresh_spine(&self, child: Id, bucket_hash: [u8; 32]) -> [u8; 32] {
        let mut below = bucket_hash;
        for level in (0..DEPTH).rev() {
            let path: Vec<u8> = (0..level).map(|i| nibble(child, i)).collect();
            let mut node = self.read_node(&path);
            node.set(nibble(child, level), below);
            self.write_node(&path, &node);
            below = node.hash();
        }
        below
    }

    /// Insert or replace `child`. Returns the trie's new root hash.
    pub fn insert(&self, child: ChildInfo) -> [u8; 32] {
        let id = child.id();
        let path: Vec<u8> = (0..DEPTH).map(|i| nibble(id, i)).collect();
        let mut bucket = self.read_bucket(&path);

        match bucket.entries.binary_search_by_key(&id, ChildInfo::id) {
            Ok(i) => bucket.entries[i] = child,
            Err(i) => bucket.entries.insert(i, child),
        }
        self.write_bucket(&path, &bucket);
        self.refresh_spine(id, bucket.hash())
    }

    /// Remove `child_id`. Returns the new root hash.
    pub fn remove(&self, child_id: Id) -> [u8; 32] {
        let path: Vec<u8> = (0..DEPTH).map(|i| nibble(child_id, i)).collect();
        let mut bucket = self.read_bucket(&path);
        if let Ok(i) = bucket
            .entries
            .binary_search_by_key(&child_id, ChildInfo::id)
        {
            let _removed = bucket.entries.remove(i);
            self.write_bucket(&path, &bucket);
            return self.refresh_spine(child_id, bucket.hash());
        }
        self.root()
    }

    /// Look up one child without materialising the rest.
    #[must_use]
    pub fn get(&self, child_id: Id) -> Option<ChildInfo> {
        let path: Vec<u8> = (0..DEPTH).map(|i| nibble(child_id, i)).collect();
        let bucket = self.read_bucket(&path);
        bucket
            .entries
            .binary_search_by_key(&child_id, ChildInfo::id)
            .ok()
            .map(|i| bucket.entries[i].clone())
    }

    /// The trie's root hash — a function of the child set, not of insert order.
    #[must_use]
    pub fn root(&self) -> [u8; 32] {
        self.read_node(&[]).hash()
    }

    /// Every child, in [`ChildInfo`]'s own order — `(created_at, id)`.
    ///
    /// That order is load-bearing, NOT cosmetic: `Vector::get(idx)` walks a
    /// collection's children in it, so a child's position is its insertion
    /// position. Enumerating by id instead would silently reorder every
    /// `Vector` in the system while every hash still matched.
    ///
    /// The trie's internal layout is keyed by id, and its hash folds buckets in
    /// id order — that is what makes the root canonical. Enumeration order is a
    /// separate concern, applied here on the way out.
    ///
    /// Linear in the number of children by nature; the structure exists to make
    /// *writes* independent of size, not to make a full enumeration cheaper.
    #[must_use]
    pub fn children(&self) -> Vec<ChildInfo> {
        let mut out = Vec::new();
        self.collect(&mut Vec::new(), &mut out);
        out.sort();
        out
    }

    fn collect(&self, path: &mut Vec<u8>, out: &mut Vec<ChildInfo>) {
        if path.len() == DEPTH {
            out.extend(self.read_bucket(path).entries);
            return;
        }
        let node = self.read_node(path);
        for (nib, _) in node.slots {
            path.push(nib);
            self.collect(path, out);
            let _popped = path.pop();
        }
    }

    /// Enumerate a parent's children using a caller-supplied reader.
    ///
    /// For callers that reach the store directly rather than through a
    /// [`StorageAdaptor`] — raw-DB diagnostic and projection paths that
    /// previously decoded the child list straight out of the parent's index row.
    /// Children moved into this keyspace, so those callers need a way to walk it
    /// without duplicating the addressing, which is what this provides.
    ///
    /// `read` is given the trie row's [`Key`] and returns its bytes, if present.
    pub fn children_with<F>(parent: Id, read: F) -> Vec<ChildInfo>
    where
        F: Fn(Key) -> Option<Vec<u8>>,
    {
        fn walk<F: Fn(Key) -> Option<Vec<u8>>>(
            parent: Id,
            path: &mut Vec<u8>,
            read: &F,
            out: &mut Vec<ChildInfo>,
        ) {
            let key = Key::ChildTrie(addr(parent, path));
            let Some(bytes) = read(key) else {
                return;
            };
            if path.len() == DEPTH {
                if let Ok(bucket) = TrieBucket::try_from_slice(&bytes) {
                    out.extend(bucket.entries);
                }
                return;
            }
            let Ok(node) = TrieNode::try_from_slice(&bytes) else {
                return;
            };
            for (nib, _) in node.slots {
                path.push(nib);
                walk(parent, path, read, out);
                let _popped = path.pop();
            }
        }

        let mut out = Vec::new();
        walk(parent, &mut Vec::new(), &read, &mut out);
        out.sort();
        out
    }

    /// Drop the whole trie.
    pub fn clear(&self) {
        for child in self.children() {
            let _root = self.remove(child.id());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entities::Metadata;

    fn child(seed: u8, hash_byte: u8) -> ChildInfo {
        // Ids in production are hashes, so spread these the same way — a trie
        // keyed by id relies on the key space being well distributed.
        let id = Id::new(Sha256::digest([seed]).into());
        ChildInfo::new(id, [hash_byte; 32], Metadata::default())
    }

    fn parent(n: u8) -> Id {
        Id::new(Sha256::digest([b'p', n]).into())
    }

    #[test]
    fn a_child_reads_back_after_insert() {
        let trie = ChildTrie::<crate::store::MainStorage>::new(parent(1));
        let c = child(7, 9);
        let _root = trie.insert(c.clone());

        assert_eq!(trie.get(c.id()).map(|g| g.merkle_hash()), Some([9; 32]));
        assert_eq!(trie.children().len(), 1);
    }

    #[test]
    fn the_root_does_not_depend_on_insertion_order() {
        // The load-bearing property. Replicas learn about children in different
        // orders constantly; a root that depended on order would never converge.
        // This is exactly why the structure is keyed by id rather than being an
        // append-ordered accumulator.
        let forward = ChildTrie::<crate::store::MainStorage>::new(parent(2));
        for i in 0..40_u8 {
            let _root = forward.insert(child(i, i));
        }

        let backward = ChildTrie::<crate::store::MainStorage>::new(parent(3));
        for i in (0..40_u8).rev() {
            let _root = backward.insert(child(i, i));
        }

        assert_eq!(
            forward.root(),
            backward.root(),
            "same child set inserted in opposite orders must give the same root"
        );
        assert_ne!(forward.root(), EMPTY);
    }

    #[test]
    fn the_root_changes_when_any_child_changes() {
        let trie = ChildTrie::<crate::store::MainStorage>::new(parent(4));
        for i in 0..10_u8 {
            let _root = trie.insert(child(i, i));
        }
        let before = trie.root();

        // Same child id, different subtree hash: the root must move, or a
        // change deep in the tree would be invisible to comparison.
        let _root = trie.insert(child(5, 200));
        assert_ne!(trie.root(), before);
    }

    #[test]
    fn removing_a_child_restores_the_previous_root() {
        let trie = ChildTrie::<crate::store::MainStorage>::new(parent(5));
        for i in 0..12_u8 {
            let _root = trie.insert(child(i, i));
        }
        let before = trie.root();

        let extra = child(99, 99);
        let _root = trie.insert(extra.clone());
        assert_ne!(trie.root(), before);

        let _root = trie.remove(extra.id());
        assert_eq!(
            trie.root(),
            before,
            "removing a child must undo its effect exactly; a root that drifted \
             would diverge from a replica that never saw the child"
        );
        assert_eq!(trie.children().len(), 12);
    }

    #[test]
    fn an_empty_trie_is_empty() {
        let trie = ChildTrie::<crate::store::MainStorage>::new(parent(6));
        assert_eq!(trie.root(), EMPTY);
        assert!(trie.children().is_empty());
        assert_eq!(trie.get(child(1, 1).id()), None);
    }

    #[test]
    fn every_child_is_enumerated() {
        let trie = ChildTrie::<crate::store::MainStorage>::new(parent(7));
        for i in 0..200_u8 {
            let _root = trie.insert(child(i, i));
        }
        let all = trie.children();
        assert_eq!(all.len(), 200);

        // Enumeration follows ChildInfo's own (created_at, id) order, which
        // Vector::get(idx) depends on — see `children`.
        for pair in all.windows(2) {
            assert!(
                pair[0] < pair[1],
                "children must come back in ChildInfo order"
            );
        }
    }

    #[test]
    fn reinserting_the_same_child_is_idempotent() {
        let trie = ChildTrie::<crate::store::MainStorage>::new(parent(8));
        let c = child(3, 3);
        let first = trie.insert(c.clone());
        let second = trie.insert(c);
        assert_eq!(first, second);
        assert_eq!(trie.children().len(), 1, "no duplicate entry");
    }
}

/// Write-cost instrumentation.
///
/// The trie exists to make a link cost the same at 10 children as at 10,000.
/// That claim is the reason for the whole structure, so it is measured rather
/// than asserted — and measured in bytes rewritten, which is what the gas meter
/// actually charges for.
#[cfg(test)]
mod cost {
    use super::*;
    use crate::entities::Metadata;
    use std::cell::RefCell;
    use std::collections::BTreeMap;

    thread_local! {
        static STORE: RefCell<BTreeMap<[u8; 32], Vec<u8>>> = RefCell::new(BTreeMap::new());
        static BYTES_WRITTEN: RefCell<usize> = const { RefCell::new(0) };
        static ROWS_WRITTEN: RefCell<usize> = const { RefCell::new(0) };
    }

    /// An adaptor that records what a write actually costs.
    #[derive(Debug)]
    struct Counting;

    impl StorageAdaptor for Counting {
        fn storage_read(key: Key) -> Option<Vec<u8>> {
            STORE.with(|s| s.borrow().get(&key.to_bytes()).cloned())
        }
        fn storage_write(key: Key, value: &[u8]) -> bool {
            BYTES_WRITTEN.with(|b| *b.borrow_mut() += value.len());
            ROWS_WRITTEN.with(|r| *r.borrow_mut() += 1);
            let _prev = STORE.with(|s| s.borrow_mut().insert(key.to_bytes(), value.to_vec()));
            true
        }
        fn storage_remove(key: Key) -> bool {
            STORE.with(|s| s.borrow_mut().remove(&key.to_bytes()).is_some())
        }
    }

    fn measure_insert_at(n: usize) -> (usize, usize) {
        STORE.with(|s| s.borrow_mut().clear());
        let parent = Id::new(Sha256::digest(b"cost").into());
        let trie = ChildTrie::<Counting>::new(parent);

        for i in 0..n {
            let id = Id::new(Sha256::digest(i.to_be_bytes()).into());
            let _root = trie.insert(ChildInfo::new(id, [1; 32], Metadata::default()));
        }

        // Measure only the next insert.
        BYTES_WRITTEN.with(|b| *b.borrow_mut() = 0);
        ROWS_WRITTEN.with(|r| *r.borrow_mut() = 0);
        let id = Id::new(Sha256::digest(n.to_be_bytes()).into());
        let _root = trie.insert(ChildInfo::new(id, [2; 32], Metadata::default()));

        (
            BYTES_WRITTEN.with(|b| *b.borrow()),
            ROWS_WRITTEN.with(|r| *r.borrow()),
        )
    }

    #[test]
    fn one_link_costs_a_bounded_amount_however_many_children_there_are() {
        let sizes = [10_usize, 1_000, 10_000, 50_000];
        let mut measured = Vec::new();
        for n in sizes {
            let (bytes, rows) = measure_insert_at(n);
            println!(
                "n={n:>6}: {bytes:>5} bytes, {rows} rows   (flat blob would be ~{} bytes)",
                n * 84
            );
            measured.push((n, bytes, rows));
        }

        // Rows per link is fixed by DEPTH, so it cannot depend on the child
        // count. This is the structural claim.
        let rows: Vec<usize> = measured.iter().map(|(_, _, r)| *r).collect();
        assert!(
            rows.windows(2).all(|w| w[0] == w[1]),
            "rows per link must be constant, got {rows:?}"
        );

        // Bytes rise as interior nodes fill their 16 slots, then stop: a node
        // cannot exceed 16 slots, so the ceiling is DEPTH*16*33 plus a bucket.
        // What matters is that it PLATEAUS rather than tracking n.
        let (_, at_10k, _) = measured[2];
        let (_, at_50k, _) = measured[3];
        let growth = at_50k as f64 / at_10k as f64;
        println!("10k -> 50k growth: {growth:.3}x");
        assert!(
            growth < 1.15,
            "cost must plateau once nodes saturate; 10k={at_10k} 50k={at_50k}"
        );

        // And the point of the exercise: at 50k the blob it replaces would
        // rewrite ~4.2 MB per link.
        assert!(
            at_50k < 4_000,
            "a link must stay in the low kilobytes, got {at_50k}"
        );
    }
}
