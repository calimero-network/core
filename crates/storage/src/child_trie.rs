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

/// Hash of an absent subtree.
///
/// An empty node hashes to this too, deliberately: `TrieNode::hash` returns it
/// when there are no slots, which is what lets `set` collapse a parent's slot
/// when its subtree empties instead of leaving a slot pointing at a node that
/// holds nothing. "Absent" and "present but empty" are therefore the same
/// thing to the fold — which is what makes the root a function of the child SET
/// rather than of the write history that produced it.
pub const EMPTY: [u8; 32] = [0; 32];

const DOMAIN_NODE: &[u8] = b"childtrie:v1:node";
const DOMAIN_BUCKET: &[u8] = b"childtrie:v1:bucket";
const DOMAIN_ADDR: &[u8] = b"childtrie:v1:addr";

/// An interior node: occupied slots only, ascending by nibble.
#[derive(Clone, Debug, Default, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct TrieNode {
    /// `(nibble, subtree hash)`, sorted by nibble and at most 16 long.
    pub slots: Vec<(u8, [u8; 32])>,
    /// Children beneath this node.
    ///
    /// Maintained along the spine an insert already walks, so `len()` is a
    /// single row read instead of a full enumeration. Without it, counting is
    /// O(n) — and a caller that counts on every write (deriving an id from the
    /// current length, say) reintroduces exactly the cost this type removes,
    /// with the trie doing nothing wrong.
    pub count: u64,
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

    /// Deliberately does NOT fold `count`: it is derived from the same slots,
    /// so hashing it would add nothing while giving a book-keeping slip the
    /// power to fork the root.
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
            debug_assert_eq!(node.count, 0, "a slotless node cannot hold children");
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
    fn refresh_spine(&self, child: Id, bucket_hash: [u8; 32], delta: i64) -> [u8; 32] {
        let mut below = bucket_hash;
        for level in (0..DEPTH).rev() {
            let path: Vec<u8> = (0..level).map(|i| nibble(child, i)).collect();
            let mut node = self.read_node(&path);
            node.set(nibble(child, level), below);
            // `saturating_add_signed` clamps rather than wrapping, which is the
            // right production behaviour — but a clamp here is a book-keeping
            // bug that nothing else can detect: `count` is deliberately not
            // folded into the hash, so drift does not fork the root, it just
            // makes `len()` quietly lie. A contract that derives an id from
            // `len()` then mints duplicate ids with no mismatch anywhere.
            debug_assert!(
                delta >= 0 || node.count >= delta.unsigned_abs(),
                "child-trie count underflow at level {level}: {} + {delta}",
                node.count
            );
            node.count = node.count.saturating_add_signed(delta);
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

        let delta = match bucket.entries.binary_search_by_key(&id, ChildInfo::id) {
            Ok(i) => {
                bucket.entries[i] = child;
                0
            }
            Err(i) => {
                bucket.entries.insert(i, child);
                1
            }
        };
        self.write_bucket(&path, &bucket);
        self.refresh_spine(id, bucket.hash(), delta)
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
            return self.refresh_spine(child_id, bucket.hash(), -1);
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

    /// Number of children, without enumerating them.
    ///
    /// One row read. The linear alternative — walking every bucket — is what a
    /// caller that counts on each write would pay per write.
    #[must_use]
    pub fn len(&self) -> u64 {
        self.read_node(&[]).count
    }

    /// Whether the parent has no children.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
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

    /// Link `child` under `parent` using caller-supplied row access.
    ///
    /// The writer-side counterpart to [`children_with`](Self::children_with),
    /// for callers that reach the store directly rather than through a
    /// [`StorageAdaptor`] — specifically snapshot sync, which installs entities
    /// by writing their `Entry` and `Index` rows straight to the store.
    ///
    /// It needs to exist because children stopped living inside the parent's
    /// index row. While they were inline, anything that shipped an index row
    /// shipped the parent→child links with it, and snapshot sync got them for
    /// free. Now they are their own keyspace, and a receiver that installs
    /// entities without also building the trie holds every entity but cannot
    /// enumerate them and computes a different root — permanently, because
    /// re-applying byte-identical entities never re-links anything.
    ///
    /// Pass the child's SHIPPED `full_hash`: the trie is a pure function of the
    /// `{(id, full_hash)}` set, so reconstructing it from the sender's values
    /// reproduces the sender's root exactly, with no re-hashing and no ordering
    /// requirement between parent and child.
    pub fn insert_with<R, W>(parent: Id, child: ChildInfo, read: R, mut write: W)
    where
        R: Fn(Key) -> Option<Vec<u8>>,
        W: FnMut(Key, &[u8]),
    {
        let id = child.id();
        let path: Vec<u8> = (0..DEPTH).map(|i| nibble(id, i)).collect();

        let mut bucket = read(Key::ChildTrie(addr(parent, &path)))
            .and_then(|bytes| TrieBucket::try_from_slice(&bytes).ok())
            .unwrap_or_default();

        let delta: i64 = match bucket.entries.binary_search_by_key(&id, ChildInfo::id) {
            Ok(i) => {
                bucket.entries[i] = child;
                0
            }
            Err(i) => {
                bucket.entries.insert(i, child);
                1
            }
        };
        if let Ok(bytes) = to_vec(&bucket) {
            write(Key::ChildTrie(addr(parent, &path)), &bytes);
        }

        // Same spine walk as `insert`, through the caller's rows.
        let mut below = bucket.hash();
        for level in (0..DEPTH).rev() {
            let node_path: Vec<u8> = (0..level).map(|i| nibble(id, i)).collect();
            let key = Key::ChildTrie(addr(parent, &node_path));
            let mut node = read(key)
                .and_then(|bytes| TrieNode::try_from_slice(&bytes).ok())
                .unwrap_or_default();
            node.set(nibble(id, level), below);
            debug_assert!(
                delta >= 0 || node.count >= delta.unsigned_abs(),
                "child-trie count underflow at level {level}: {} + {delta}",
                node.count
            );
            node.count = node.count.saturating_add_signed(delta);
            if let Ok(bytes) = to_vec(&node) {
                write(key, &bytes);
            }
            below = node.hash();
        }
    }

    /// Remove every row of this trie.
    ///
    /// A deleted entity's trie must go with it. Its rows live in their own
    /// keyspace, so nothing else reaches them: dropping the entity's `Entry`
    /// and tombstoning its `Index` leaves the trie behind, and because
    /// collection ids are deterministic (`compute_collection_id(parent,
    /// field_name)`), deleting and later re-creating the same field would find
    /// a trie still holding the OLD children — ids whose `Entry` rows are gone,
    /// folded into the parent's hash as a ghost root.
    ///
    /// Walks the node structure to find the occupied rows rather than
    /// removing children one at a time: this drops the whole trie, so there is
    /// no spine left to refresh and paying `DEPTH+1` writes per child to
    /// maintain one would be pure waste.
    pub fn drop_all(&self) {
        // A root row that is present but undecodable reads as an empty trie
        // through `read_node`, which would delete one row and orphan every
        // bucket beneath it — the exact ghost-children state this exists to
        // prevent, reached by the one input it cannot tell from "empty".
        if let Some(bytes) = S::storage_read(Key::ChildTrie(addr(self.parent, &[]))) {
            if TrieNode::try_from_slice(&bytes).is_err() {
                tracing::warn!(
                    parent = ?self.parent,
                    "child-trie root row present but undecodable; dropping what can be \
                     reached, rows beneath it may be orphaned"
                );
            }
        }

        let mut paths: Vec<Vec<u8>> = Vec::new();
        Self::collect_paths(self, &mut Vec::new(), &mut paths);
        for path in paths {
            let _ignored = S::storage_remove(Key::ChildTrie(addr(self.parent, &path)));
        }
    }

    fn collect_paths(&self, path: &mut Vec<u8>, out: &mut Vec<Vec<u8>>) {
        if path.len() == DEPTH {
            out.push(path.clone());
            return;
        }
        let node = self.read_node(path);
        if node.slots.is_empty() && !path.is_empty() {
            return;
        }
        out.push(path.clone());
        for (nib, _) in node.slots {
            path.push(nib);
            self.collect_paths(path, out);
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

    /// Same as [`child`], but with an explicit `created_at`/`updated_at`.
    fn child_created_at(seed: u8, hash_byte: u8, created_at: u64) -> ChildInfo {
        let id = Id::new(Sha256::digest([seed]).into());
        ChildInfo::new(id, [hash_byte; 32], Metadata::new(created_at, created_at))
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

    /// `insert_with` re-derives by hand what `insert` + `refresh_spine` do,
    /// because snapshot sync reaches the store directly rather than through a
    /// `StorageAdaptor`. Two implementations of one spine walk is a latent
    /// fork, and the consequence is precisely the bug the snapshot half of
    /// this work exists to fix: a receiver that reconstructs a DIFFERENT root
    /// from the sender, permanently, because re-applying byte-identical
    /// entities never re-links anything.
    ///
    /// So pin them against each other directly. Same parent, same child set,
    /// one through each path — then require the rows to be byte-identical and
    /// the roots, counts and enumerations to agree. Anything that changes one
    /// walk without the other fails here instead of in a divergent context.
    #[test]
    fn insert_with_writes_exactly_what_insert_writes() {
        use std::cell::RefCell;
        use std::collections::BTreeMap;

        let parent = parent(30);
        let children: Vec<ChildInfo> = (0..40_u8).map(|i| child(i, i)).collect();

        // Path A: the StorageAdaptor walk.
        let trie = ChildTrie::<crate::store::MainStorage>::new(parent);
        for c in &children {
            let _root = trie.insert(c.clone());
        }

        // Path B: the caller-supplied-rows walk, over its own store. Same
        // parent id, so both paths address rows identically — a row written by
        // one is directly comparable to the other's.
        let rows: RefCell<BTreeMap<Key, Vec<u8>>> = RefCell::new(BTreeMap::new());
        for c in &children {
            ChildTrie::<crate::store::MainStorage>::insert_with(
                parent,
                c.clone(),
                |k| rows.borrow().get(&k).cloned(),
                |k, v| {
                    let _prev = rows.borrow_mut().insert(k, v.to_vec());
                },
            );
        }

        let rows = rows.into_inner();
        assert!(!rows.is_empty(), "insert_with wrote nothing");

        for (key, value) in &rows {
            let via_adaptor = crate::store::MainStorage::storage_read(*key);
            assert_eq!(
                via_adaptor.as_ref(),
                Some(value),
                "row {key:?} differs between insert and insert_with"
            );
        }

        let root_b = TrieNode::try_from_slice(
            rows.get(&Key::ChildTrie(addr(parent, &[])))
                .expect("insert_with wrote no root node"),
        )
        .expect("root node decodes");

        assert_eq!(trie.root(), root_b.hash(), "roots must agree");
        assert_eq!(trie.len(), root_b.count, "counts must agree");
        assert_eq!(
            trie.children(),
            ChildTrie::<crate::store::MainStorage>::children_with(parent, |k| rows
                .get(&k)
                .cloned()),
            "enumerations must agree"
        );
    }

    /// A deleted entity's trie must not outlive it.
    ///
    /// Trie rows are their own keyspace, so dropping the entity's `Entry` and
    /// tombstoning its `Index` does not touch them, and tombstone GC never
    /// sees them (it requires a row to decode as a tombstoned `EntityIndex`).
    /// Collection ids are deterministic, so a field that is deleted and later
    /// re-created lands on the SAME trie — and would inherit the old children:
    /// ids whose data is gone, folded into the parent's hash as a ghost root,
    /// with nothing anywhere reporting an error.
    /// `count` went from derived to maintained, and nothing can detect drift.
    ///
    /// It is deliberately not folded into the hash — hashing a book-keeping
    /// value would give a slip the power to fork the root — so a wrong count
    /// does not diverge anything. `len()` simply lies. Follow that through:
    /// `Collection::len` reads this count, and the contract that motivated the
    /// whole change derives a message id from `len()` on every write. A count
    /// that drifts LOW mints duplicate ids: silent data loss, no hash mismatch,
    /// no warning, nothing to alert on.
    ///
    /// So pin the invariant directly, over a mixed sequence rather than a happy
    /// path — inserts, replacements (delta 0), removals, removals of absent
    /// ids (also delta 0), and re-inserts — checking after every step, through
    /// both the adaptor walk and the caller-supplied-rows walk that snapshot
    /// sync uses.
    #[test]
    fn the_count_never_drifts_from_the_number_of_children() {
        use std::cell::RefCell;
        use std::collections::BTreeMap;

        let id = parent(50);
        let trie = ChildTrie::<crate::store::MainStorage>::new(id);

        // A deterministic mixed workload: seed i decides the operation.
        let mut live: std::collections::BTreeSet<u8> = std::collections::BTreeSet::new();
        for step in 0..120_u8 {
            let seed = step.wrapping_mul(37).wrapping_add(11);
            match step % 5 {
                // a genuinely new child (three in five, so the workload grows)
                0..=2 => {
                    let _root = trie.insert(child(seed, step));
                    let _inserted = live.insert(seed);
                }
                // RE-insert one already present. This is the case that made an
                // earlier version of this test useless: the seed is a bijection
                // over `step`, so every insert was new and a replacement
                // counting as +1 passed unnoticed. Caught by mutation.
                3 => {
                    let victim = live.iter().next().copied().unwrap_or(seed);
                    let _root = trie.insert(child(victim, step));
                    let _inserted = live.insert(victim);
                }
                // remove a live one
                4 => {
                    let victim = live.iter().next().copied().unwrap_or(seed);
                    let _root = trie.remove(Id::new(Sha256::digest([victim]).into()));
                    let _removed = live.remove(&victim);
                }
                _ => unreachable!("step % 5 is exhaustive above"),
            }

            // Removing an id that was never inserted must also be delta 0.
            let _root = trie.remove(Id::new(Sha256::digest([seed ^ 0x5A]).into()));

            assert_eq!(
                trie.len() as usize,
                trie.children().len(),
                "count diverged from enumeration at step {step}"
            );
        }
        assert!(!trie.is_empty(), "the workload must leave children behind");

        // Same invariant through `insert_with`, which is where a
        // snapshot-built trie gets its counts and which nothing else covered.
        let rows: RefCell<BTreeMap<Key, Vec<u8>>> = RefCell::new(BTreeMap::new());
        let other = parent(51);
        for i in 0..40_u8 {
            ChildTrie::<crate::store::MainStorage>::insert_with(
                other,
                child(i, i),
                |k| rows.borrow().get(&k).cloned(),
                |k, v| {
                    let _prev = rows.borrow_mut().insert(k, v.to_vec());
                },
            );
            // Re-inserting the same child must not double-count.
            ChildTrie::<crate::store::MainStorage>::insert_with(
                other,
                child(i, i.wrapping_add(1)),
                |k| rows.borrow().get(&k).cloned(),
                |k, v| {
                    let _prev = rows.borrow_mut().insert(k, v.to_vec());
                },
            );
        }

        let rows = rows.into_inner();
        let root = TrieNode::try_from_slice(
            rows.get(&Key::ChildTrie(addr(other, &[])))
                .expect("root node written"),
        )
        .expect("root node decodes");
        let enumerated =
            ChildTrie::<crate::store::MainStorage>::children_with(other, |k| rows.get(&k).cloned());

        assert_eq!(
            root.count as usize,
            enumerated.len(),
            "insert_with's count must match what it can enumerate"
        );
        assert_eq!(
            enumerated.len(),
            40,
            "replacements must not inflate the count"
        );
    }

    #[test]
    fn dropping_a_trie_leaves_nothing_for_a_later_incarnation_to_inherit() {
        let id = parent(40);

        let trie = ChildTrie::<crate::store::MainStorage>::new(id);
        for i in 0..25_u8 {
            let _root = trie.insert(child(i, i));
        }
        assert_eq!(trie.len(), 25);
        assert_ne!(trie.root(), EMPTY);

        trie.drop_all();

        // A fresh handle on the same id — which is what re-creating a
        // deterministically-named collection produces.
        let reborn = ChildTrie::<crate::store::MainStorage>::new(id);
        assert_eq!(
            reborn.root(),
            EMPTY,
            "a re-created collection must start empty"
        );
        assert_eq!(reborn.len(), 0, "and must not inherit the old count");
        assert!(
            reborn.children().is_empty(),
            "and must not enumerate the previous incarnation's children"
        );
    }

    #[test]
    fn the_root_does_not_depend_on_when_each_child_was_created() {
        // Issue #2418. `created_at` is a LOCAL wall-clock observation: two peers
        // that independently create the same entity — canonically the `Root<T>`
        // opaque marker each one writes the first time an app touches its state
        // — stamp different times for identical bytes. Any parent hash that
        // admits `created_at` therefore diverges across peers holding the same
        // data, and sync compares hashes.
        //
        // The old inline-list fold defended this by sorting children by id
        // before hashing. The trie defends it twice over: buckets hold entries
        // id-sorted, and the bucket fold covers only id + merkle_hash, so no
        // part of `Metadata` reaches the root at all. This test is what makes
        // that second property a decision rather than an accident — folding
        // metadata into `TrieBucket::hash` would pass every other test here.
        let peer1 = ChildTrie::<crate::store::MainStorage>::new(parent(20));
        for (i, t) in [(1_u8, 100_u64), (2, 101), (3, 102)] {
            let _root = peer1.insert(child_created_at(i, i, t));
        }

        let peer2 = ChildTrie::<crate::store::MainStorage>::new(parent(21));
        for (i, t) in [(1_u8, 200_u64), (2, 201), (3, 202)] {
            let _root = peer2.insert(child_created_at(i, i, t));
        }

        assert_eq!(
            peer1.root(),
            peer2.root(),
            "same ids + same merkle hashes must give the same root however the \
             peers' local creation times differ"
        );
        assert_ne!(peer1.root(), EMPTY);
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
        static STORE: RefCell<BTreeMap<[u8; 32], Vec<u8>>> = const { RefCell::new(BTreeMap::new()) };
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

#[cfg(test)]
mod count_tests {
    use super::*;
    use crate::entities::Metadata;
    use crate::store::MainStorage;

    fn child(seed: u16) -> ChildInfo {
        let id = Id::new(Sha256::digest(seed.to_be_bytes()).into());
        ChildInfo::new(id, [1; 32], Metadata::default())
    }

    #[test]
    fn the_count_tracks_inserts_and_removals() {
        let trie = ChildTrie::<MainStorage>::new(Id::new(Sha256::digest(b"count").into()));
        assert_eq!(trie.len(), 0);
        assert!(trie.is_empty());

        for i in 0..300_u16 {
            let _root = trie.insert(child(i));
        }
        assert_eq!(trie.len(), 300);
        assert_eq!(
            trie.len() as usize,
            trie.children().len(),
            "count must match enumeration"
        );

        // Re-inserting the same child is an update, not a new child.
        let _root = trie.insert(child(7));
        assert_eq!(trie.len(), 300);

        for i in 0..50_u16 {
            let _root = trie.remove(child(i).id());
        }
        assert_eq!(trie.len(), 250);
        assert_eq!(trie.len() as usize, trie.children().len());

        // Removing something absent must not drift the count.
        let _root = trie.remove(child(9_999).id());
        assert_eq!(trie.len(), 250);
    }

    #[test]
    fn the_count_does_not_change_the_root() {
        // `count` is derived book-keeping. Folding it into the hash would let a
        // counting slip fork the root, so the two must be independent.
        let a = ChildTrie::<MainStorage>::new(Id::new(Sha256::digest(b"root-a").into()));
        let b = ChildTrie::<MainStorage>::new(Id::new(Sha256::digest(b"root-b").into()));
        for i in 0..20_u16 {
            let _root = a.insert(child(i));
        }
        for i in (0..20_u16).rev() {
            let _root = b.insert(child(i));
        }
        assert_eq!(a.root(), b.root());
        assert_eq!(a.len(), b.len());
    }
}
