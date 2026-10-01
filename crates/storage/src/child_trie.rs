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
//! A hex trie keyed by child id whose shape is decided by the child set alone:
//!
//! ```text
//! subtree(prefix p) = bucket(sorted children)        if at most BUCKET_MAX children start with p
//!                   = node(16 slots over nibble |p|)  otherwise
//! ```
//!
//! A parent with a few children (the common case: a record holding a handful of
//! nested collections) costs one row. A parent with thousands costs roughly one
//! row per `BUCKET_MAX / 2` children, and a write touches one row per level,
//! about `log16(n / BUCKET_MAX) + 1` rows. The fixed-depth trie this replaced
//! paid four node rows plus a bucket for every child of a small parent.
//!
//! A bucket stores only what the fold needs: each child's id and hash. The
//! child's metadata, enumeration order included, is read from its own index
//! row when a caller asks for a [`ChildInfo`].
//!
//! # Why keyed by id, and not an append-order accumulator
//!
//! The root must be a function of the child *set*, never of the order the
//! children arrived. Two replicas learn about the same children in different
//! orders all the time; if the root depended on that order they would never
//! converge. An MMR-style accumulator is append-ordered and cannot be used here
//! for exactly that reason. A trie keyed by child id is canonical: the position
//! of a child is determined by its id alone.

use borsh::BorshDeserialize;
use sha2::{Digest, Sha256};

use crate::address::Id;
use crate::admitted_count;
use crate::entities::{ChildInfo, Metadata};
use crate::index::EntityIndex;
use crate::store::{Key, MainStorage, StorageAdaptor};

/// Most children a subtree holds as one bucket row before it splits.
///
/// The shape of the trie is a function of this and the child set alone (see the
/// module docs), so every replica must use the same value: changing it changes
/// every root hash.
pub const BUCKET_MAX: usize = 16;

/// Hash of an absent subtree, and of a parent with no children.
///
/// "Absent" and "present but empty" are deliberately the same thing to the
/// fold, which is what makes the root a function of the child SET rather than
/// of the write history that produced it.
pub const EMPTY: [u8; 32] = [0; 32];

const DOMAIN_NODE: &[u8] = b"childtrie:v2:node";
const DOMAIN_BUCKET: &[u8] = b"childtrie:v2:bucket";
const DOMAIN_ADDR: &[u8] = b"childtrie:v2:addr";

const TAG_BUCKET: u8 = 0xB2;
const TAG_NODE: u8 = 0xA2;

/// One child as its parent's trie records it.
///
/// Only its id and hash are stored: that is all the fold needs. The child's
/// [`Metadata`] lives in its own index row and is read from there when a caller
/// asks for a [`ChildInfo`]; a copy in the bucket would double it and go stale
/// whenever the child is updated in place.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Slot {
    /// The child's id.
    pub id: Id,
    /// The child's full hash, as folded into this parent.
    pub hash: [u8; 32],
    /// The child's `created_at`, carried while linking and never stored: a
    /// slot read back from a bucket has 0 here.
    pub created_at: u64,
    /// The writer-assigned position, carried while linking (it advances the
    /// parent's `next_order`) and never stored: 0 when read back.
    pub order: u64,
}

impl Slot {
    fn of(child: &ChildInfo) -> Self {
        Self {
            id: child.id(),
            hash: child.merkle_hash(),
            created_at: child.metadata.created_at,
            order: child.metadata.order,
        }
    }
}

/// An interior node: occupied slots only, ascending by nibble.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TrieNode {
    /// `(nibble, subtree hash)`, sorted by nibble and at most 16 long.
    pub slots: Vec<(u8, [u8; 32])>,
    /// Children beneath this node. Always more than [`BUCKET_MAX`], or it would
    /// be a bucket.
    ///
    /// Maintained rather than derived, so `len()` is one row read. Deliberately
    /// not folded into the hash: it is book-keeping, and hashing it would give a
    /// counting slip the power to fork the root.
    pub count: u64,
}

impl TrieNode {
    fn set(&mut self, nibble: u8, hash: [u8; 32]) {
        match self.slots.binary_search_by_key(&nibble, |(n, _)| *n) {
            Ok(i) if hash == EMPTY => {
                let _removed = self.slots.remove(i);
            }
            Ok(i) => self.slots[i].1 = hash,
            Err(i) if hash != EMPTY => self.slots.insert(i, (nibble, hash)),
            Err(_) => {}
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

/// A leaf bucket: every child under this prefix, sorted by id.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TrieBucket {
    /// Children, sorted by id so the fold is canonical.
    pub entries: Vec<Slot>,
}

impl TrieBucket {
    fn hash(&self) -> [u8; 32] {
        if self.entries.is_empty() {
            return EMPTY;
        }
        let mut hasher = Sha256::new();
        hasher.update(DOMAIN_BUCKET);
        for slot in &self.entries {
            hasher.update(slot.id.as_bytes());
            hasher.update(slot.hash);
        }
        hasher.finalize().into()
    }
}

/// What a trie row holds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Body {
    /// More than [`BUCKET_MAX`] children beneath: 16-way fan-out.
    Node(TrieNode),
    /// At most [`BUCKET_MAX`] children beneath, held inline.
    Bucket(TrieBucket),
}

impl Body {
    fn hash(&self) -> [u8; 32] {
        match self {
            Self::Node(node) => node.hash(),
            Self::Bucket(bucket) => bucket.hash(),
        }
    }

    fn count(&self) -> u64 {
        match self {
            Self::Node(node) => node.count,
            Self::Bucket(bucket) => bucket.entries.len() as u64,
        }
    }
}

/// One stored trie row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrieRow {
    /// One past the highest position ever handed out under this parent — a
    /// high-water mark, kept on the root row only (zero elsewhere).
    ///
    /// The count cannot serve as the next position: it FALLS when a child is
    /// removed, and the next append would reuse a position still in use. So the
    /// mark only ever rises. Like `count`, it is kept out of the hash.
    pub next_order: u64,
    /// The node or bucket.
    pub body: Body,
}

fn put_varint(out: &mut Vec<u8>, mut value: u64) {
    loop {
        let byte = (value & 0x7f) as u8;
        value >>= 7;
        if value == 0 {
            out.push(byte);
            return;
        }
        out.push(byte | 0x80);
    }
}

fn take_varint(bytes: &mut &[u8]) -> Option<u64> {
    let mut value: u64 = 0;
    for shift in (0..64).step_by(7) {
        let (&byte, rest) = bytes.split_first()?;
        *bytes = rest;
        value |= u64::from(byte & 0x7f).checked_shl(shift)?;
        if byte & 0x80 == 0 {
            // Reject non-minimal encodings, so one row has one byte string.
            if byte == 0 && shift != 0 {
                return None;
            }
            return Some(value);
        }
    }
    None
}

fn take_32(bytes: &mut &[u8]) -> Option<[u8; 32]> {
    if bytes.len() < 32 {
        return None;
    }
    let (head, rest) = bytes.split_at(32);
    *bytes = rest;
    head.try_into().ok()
}

impl TrieRow {
    /// Compact encoding: varints for counts and positions, a 16-bit occupancy
    /// map for a node's slots.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        match &self.body {
            Body::Bucket(bucket) => {
                out.push(TAG_BUCKET);
                put_varint(&mut out, self.next_order);
                put_varint(&mut out, bucket.entries.len() as u64);
                // Only what the fold needs: enumeration order comes from each
                // child's own index row (see `hydrate`).
                for slot in &bucket.entries {
                    out.extend_from_slice(slot.id.as_bytes());
                    out.extend_from_slice(&slot.hash);
                }
            }
            Body::Node(node) => {
                out.push(TAG_NODE);
                put_varint(&mut out, self.next_order);
                put_varint(&mut out, node.count);
                let mut map: u16 = 0;
                for (nibble, _) in &node.slots {
                    map |= 1 << nibble;
                }
                out.extend_from_slice(&map.to_be_bytes());
                for (_, hash) in &node.slots {
                    out.extend_from_slice(hash);
                }
            }
        }
        out
    }

    /// Inverse of [`encode`](Self::encode). `None` for anything it would not
    /// have produced, trailing bytes included.
    #[must_use]
    pub fn decode(mut bytes: &[u8]) -> Option<Self> {
        let bytes = &mut bytes;
        let (&tag, rest) = bytes.split_first()?;
        *bytes = rest;
        let next_order = take_varint(bytes)?;
        let body = match tag {
            TAG_BUCKET => {
                let n = usize::try_from(take_varint(bytes)?).ok()?;
                if n > bytes.len() / 64 {
                    return None;
                }
                let mut entries = Vec::with_capacity(n);
                for _ in 0..n {
                    let id = Id::new(take_32(bytes)?);
                    let hash = take_32(bytes)?;
                    entries.push(Slot {
                        id,
                        hash,
                        created_at: 0,
                        order: 0,
                    });
                }
                Body::Bucket(TrieBucket { entries })
            }
            TAG_NODE => {
                let count = take_varint(bytes)?;
                if bytes.len() < 2 {
                    return None;
                }
                let map = u16::from_be_bytes([bytes[0], bytes[1]]);
                *bytes = &bytes[2..];
                let mut slots = Vec::with_capacity(map.count_ones() as usize);
                for nibble in 0..16_u8 {
                    if map & (1 << nibble) != 0 {
                        slots.push((nibble, take_32(bytes)?));
                    }
                }
                Body::Node(TrieNode { slots, count })
            }
            _ => return None,
        };
        bytes.is_empty().then_some(Self { next_order, body })
    }
}

/// The `i`th nibble of `id`, high nibble first.
fn nibble(id: Id, i: usize) -> u8 {
    nibble_of(id.as_bytes(), i)
}

/// The `i`th nibble of `bytes`, high nibble first.
fn nibble_of(bytes: &[u8], i: usize) -> u8 {
    let byte = bytes[i / 2];
    if i.is_multiple_of(2) {
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

/// Row access, so the adaptor path and the caller-supplied-rows path used by
/// snapshot sync run the SAME walk. Two copies of one walk is a latent fork: a
/// receiver that rebuilds a different root from the sender's children.
trait Rows {
    fn get(&self, key: Key) -> Option<Vec<u8>>;
    fn put(&mut self, key: Key, value: &[u8]);
    fn del(&mut self, key: Key);
}

struct Adaptor<S>(core::marker::PhantomData<S>);

impl<S: StorageAdaptor> Rows for Adaptor<S> {
    fn get(&self, key: Key) -> Option<Vec<u8>> {
        S::storage_read(key)
    }
    fn put(&mut self, key: Key, value: &[u8]) {
        let _ignored = S::storage_write(key, value);
    }
    fn del(&mut self, key: Key) {
        let _ignored = S::storage_remove(key);
    }
}

struct Closures<R, W> {
    read: R,
    write: W,
}

impl<R: Fn(Key) -> Option<Vec<u8>>, W: FnMut(Key, &[u8])> Rows for Closures<R, W> {
    fn get(&self, key: Key) -> Option<Vec<u8>> {
        (self.read)(key)
    }
    fn put(&mut self, key: Key, value: &[u8]) {
        (self.write)(key, value);
    }
    fn del(&mut self, _key: Key) {
        // Only removal deletes rows, and removal is not offered through
        // caller-supplied rows: snapshot sync only ever links.
        unreachable!("caller-supplied trie rows are insert-only");
    }
}

/// Reads one row. Absent and undecodable are different, and only one of them
/// means "no children here", so the second is logged loudly.
fn read_row(rows: &impl Rows, parent: Id, path: &[u8]) -> Option<TrieRow> {
    let bytes = rows.get(Key::ChildTrie(addr(parent, path)))?;
    let row = TrieRow::decode(&bytes);
    if row.is_none() {
        tracing::warn!(
            ?parent,
            ?path,
            "child-trie row present but undecodable; treating the subtree as empty, \
             which UNDERSTATES this parent's children and its hash"
        );
    }
    row
}

fn write_row(rows: &mut impl Rows, parent: Id, path: &[u8], next_order: u64, body: Body) {
    let row = TrieRow { next_order, body };
    rows.put(Key::ChildTrie(addr(parent, path)), &row.encode());
}

/// Writes `entries` (sorted by id, all sharing `path`) as the canonical subtree
/// at `path`, splitting as far as the rule requires. Returns its hash.
fn build(
    rows: &mut impl Rows,
    parent: Id,
    path: &mut Vec<u8>,
    entries: Vec<Slot>,
    next_order: u64,
) -> [u8; 32] {
    if entries.len() <= BUCKET_MAX {
        let body = Body::Bucket(TrieBucket { entries });
        let hash = body.hash();
        write_row(rows, parent, path, next_order, body);
        return hash;
    }
    let depth = path.len();
    let count = entries.len() as u64;
    let mut groups: [Vec<Slot>; 16] = Default::default();
    for slot in entries {
        groups[nibble(slot.id, depth) as usize].push(slot);
    }
    let mut node = TrieNode {
        slots: Vec::new(),
        count,
    };
    for (nib, group) in groups.into_iter().enumerate() {
        if group.is_empty() {
            continue;
        }
        path.push(nib as u8);
        let hash = build(rows, parent, path, group, 0);
        let _popped = path.pop();
        node.slots.push((nib as u8, hash));
    }
    let hash = node.hash();
    write_row(rows, parent, path, next_order, Body::Node(node));
    hash
}

/// Every slot beneath `path`, optionally deleting the rows as it goes.
fn collect(
    rows: &mut impl Rows,
    parent: Id,
    path: &mut Vec<u8>,
    out: &mut Vec<Slot>,
    delete: bool,
) {
    let Some(row) = read_row(rows, parent, path) else {
        return;
    };
    if delete {
        rows.del(Key::ChildTrie(addr(parent, path)));
    }
    match row.body {
        Body::Bucket(bucket) => out.extend(bucket.entries),
        Body::Node(node) => {
            for (nib, _) in node.slots {
                path.push(nib);
                collect(rows, parent, path, out, delete);
                let _popped = path.pop();
            }
        }
    }
}

/// Inserts or replaces `slot` in the subtree at `path`. Returns the subtree's
/// new hash and whether a child was added (as opposed to replaced).
fn insert_at(rows: &mut impl Rows, parent: Id, path: &mut Vec<u8>, slot: Slot) -> ([u8; 32], bool) {
    let row = read_row(rows, parent, path);
    let is_root = path.is_empty();
    let next_order = match (&row, is_root) {
        (Some(row), true) => row.next_order.max(slot.order.saturating_add(1)),
        (None, true) => slot.order.saturating_add(1),
        (_, false) => 0,
    };
    match row.map(|row| row.body) {
        None => {
            let body = Body::Bucket(TrieBucket {
                entries: vec![slot],
            });
            let hash = body.hash();
            write_row(rows, parent, path, next_order, body);
            (hash, true)
        }
        Some(Body::Bucket(mut bucket)) => {
            let added = match bucket.entries.binary_search_by_key(&slot.id, |s| s.id) {
                Ok(i) => {
                    bucket.entries[i] = slot;
                    false
                }
                Err(i) => {
                    bucket.entries.insert(i, slot);
                    true
                }
            };
            (build(rows, parent, path, bucket.entries, next_order), added)
        }
        Some(Body::Node(mut node)) => {
            let nib = nibble(slot.id, path.len());
            path.push(nib);
            let (below, added) = insert_at(rows, parent, path, slot);
            let _popped = path.pop();
            node.set(nib, below);
            node.count += u64::from(added);
            let hash = node.hash();
            write_row(rows, parent, path, next_order, Body::Node(node));
            (hash, added)
        }
    }
}

/// Removes `id` from the subtree at `path`, merging a node back into a bucket
/// once it holds [`BUCKET_MAX`] or fewer. `None` when `id` is not there.
fn remove_at(rows: &mut impl Rows, parent: Id, path: &mut Vec<u8>, id: Id) -> Option<[u8; 32]> {
    let row = read_row(rows, parent, path)?;
    let key = Key::ChildTrie(addr(parent, path));
    match row.body {
        Body::Bucket(mut bucket) => {
            let i = bucket.entries.binary_search_by_key(&id, |s| s.id).ok()?;
            let _removed = bucket.entries.remove(i);
            if bucket.entries.is_empty() {
                // Also resets the root's position mark, which is harmless:
                // with no children left there is nothing to collide with.
                rows.del(key);
                return Some(EMPTY);
            }
            let body = Body::Bucket(bucket);
            let hash = body.hash();
            write_row(rows, parent, path, row.next_order, body);
            Some(hash)
        }
        Body::Node(mut node) => {
            let nib = nibble(id, path.len());
            path.push(nib);
            let below = remove_at(rows, parent, path, id);
            let _popped = path.pop();
            let below = below?;
            node.set(nib, below);
            node.count = node.count.saturating_sub(1);
            if node.count as usize > BUCKET_MAX {
                let hash = node.hash();
                write_row(rows, parent, path, row.next_order, Body::Node(node));
                return Some(hash);
            }
            // At or under the threshold: this subtree is a bucket by rule.
            let mut entries = Vec::new();
            for (nib, _) in &node.slots {
                path.push(*nib);
                collect(rows, parent, path, &mut entries, true);
                let _popped = path.pop();
            }
            entries.sort_by_key(|s| s.id);
            debug_assert_eq!(entries.len() as u64, node.count, "child-trie count drift");
            Some(build(rows, parent, path, entries, row.next_order))
        }
    }
}

/// The bucket slot for `id`, descending from the root.
fn find(rows: &impl Rows, parent: Id, id: Id) -> Option<Slot> {
    let mut path = Vec::new();
    loop {
        match read_row(rows, parent, &path)?.body {
            Body::Bucket(bucket) => {
                let i = bucket.entries.binary_search_by_key(&id, |s| s.id).ok()?;
                return Some(bucket.entries[i]);
            }
            Body::Node(node) => {
                let nib = nibble(id, path.len());
                node.slots.binary_search_by_key(&nib, |(n, _)| *n).ok()?;
                path.push(nib);
            }
        }
    }
}

/// Every slot whose id starts with `prefix`: descends as far as the prefix
/// reaches, then gathers that one subtree.
fn with_prefix(rows: &mut impl Rows, parent: Id, prefix: &[u8]) -> Vec<Slot> {
    let mut path = Vec::new();
    let nibbles = prefix.len() * 2;
    while path.len() < nibbles {
        let Some(row) = read_row(rows, parent, &path) else {
            return Vec::new();
        };
        let Body::Node(node) = row.body else {
            break;
        };
        let nib = nibble_of(prefix, path.len());
        if node.slots.binary_search_by_key(&nib, |(n, _)| *n).is_err() {
            return Vec::new();
        }
        path.push(nib);
    }
    let mut out = Vec::new();
    collect(rows, parent, &mut path, &mut out, false);
    out.retain(|slot| slot.id.as_bytes().starts_with(prefix));
    out.sort_by_key(|s| s.id);
    out
}

/// The slots with id at or above `from`, ascending by id, a whole row at a
/// time until `at_least` are collected, and the id to resume from: the lowest
/// id the next unread row can hold, `None` when no row is left.
///
/// A row is only read when its subtree can hold an id at or above `from`, so a
/// page costs the rows under the children it returns, never the parent's size.
/// The resume point is a bound on ids, not a position in the trie, so it stays
/// put however the trie splits and merges while children come and go.
fn slots_from(
    rows: &impl Rows,
    parent: Id,
    path: &mut Vec<u8>,
    from: Option<&Id>,
    at_least: usize,
    out: &mut Vec<Slot>,
) -> Option<Id> {
    if out.len() >= at_least {
        return Some(lowest_under(path));
    }
    let row = read_row(rows, parent, path)?;
    match row.body {
        Body::Bucket(bucket) => {
            out.extend(
                bucket
                    .entries
                    .into_iter()
                    .filter(|slot| from.is_none_or(|from| slot.id >= *from)),
            );
            None
        }
        Body::Node(node) => {
            let depth = path.len();
            for (nib, _) in node.slots {
                // Still on `from`'s spine: skip what lies below it, keep the
                // bound for the one branch it runs through.
                let bound = match from.map(|from| (from, nibble(*from, depth))) {
                    Some((_, at)) if nib < at => continue,
                    Some((from, at)) if nib == at => Some(from),
                    _ => None,
                };
                path.push(nib);
                let resume = slots_from(rows, parent, path, bound, at_least, out);
                let _popped = path.pop();
                if resume.is_some() {
                    return resume;
                }
            }
            None
        }
    }
}

/// The lowest id a subtree at `path` can hold: the path, zero-filled.
fn lowest_under(path: &[u8]) -> Id {
    let mut bytes = [0_u8; 32];
    for (i, nib) in path.iter().enumerate() {
        bytes[i / 2] |= if i % 2 == 0 { nib << 4 } else { *nib };
    }
    Id::new(bytes)
}

/// A [`ChildInfo`] for `slot`, with the child's metadata read from its own
/// index row.
///
/// Falls back to bare metadata when that row is absent — a snapshot can link a
/// child before installing it, and unit tests link bare ids — so such a child
/// sorts by id alone until its row lands.
fn hydrate(read: impl Fn(Key) -> Option<Vec<u8>>, slot: Slot) -> ChildInfo {
    let metadata = read(Key::Index(slot.id))
        .and_then(|bytes| EntityIndex::try_from_slice(&bytes).ok())
        .map(|index| index.metadata)
        .unwrap_or_else(|| Metadata {
            created_at: slot.created_at,
            order: slot.order,
            ..Metadata::default()
        });
    ChildInfo::new(slot.id, slot.hash, metadata)
}

/// Per-parent child trie.
///
/// Its shape is a pure function of the child set: the subtree under a prefix is
/// a single bucket row while it holds at most [`BUCKET_MAX`] children, and a
/// 16-way node over the next nibble otherwise. So a parent with a handful of
/// children costs one row, a large one costs about one row per `BUCKET_MAX / 2`
/// children, and two replicas holding the same children hold the same rows and
/// the same root, whatever order they learned about them in.
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

    fn rows() -> Adaptor<S> {
        Adaptor(core::marker::PhantomData)
    }

    /// Insert or replace `child`. Returns the trie's new root hash.
    pub fn insert(&self, child: ChildInfo) -> [u8; 32] {
        let tally = admitted_count::before_change::<S>(self.parent);
        let slot = Slot::of(&child);
        let (root, added) = insert_at(&mut Self::rows(), self.parent, &mut Vec::new(), slot);
        if let Some(tally) = tally {
            // A replaced child contributes what it did before: what a
            // collection admits is decided by the stamp in the child's index
            // row, and nothing rewrites a linked child's stamp across that
            // line (see `admitted_count`).
            let linked = added.then(|| hydrate(S::storage_read, slot));
            tally.finish::<S>(root, None, linked.as_ref());
        }
        root
    }

    /// Remove `child_id`. Returns the new root hash.
    pub fn remove(&self, child_id: Id) -> [u8; 32] {
        let tally = admitted_count::before_change::<S>(self.parent);
        // Read before it is unlinked: once it is gone there is no way to tell
        // what it contributed.
        let unlinked = tally.as_ref().and_then(|_| self.get(child_id));
        let Some(root) = remove_at(&mut Self::rows(), self.parent, &mut Vec::new(), child_id)
        else {
            return self.root();
        };
        if let Some(tally) = tally {
            tally.finish::<S>(root, unlinked.as_ref(), None);
        }
        root
    }

    /// Look up one child without materialising the rest.
    #[must_use]
    pub fn get(&self, child_id: Id) -> Option<ChildInfo> {
        find(&Self::rows(), self.parent, child_id).map(|slot| hydrate(S::storage_read, slot))
    }

    /// Whether `child_id` is linked here, without reading its index row.
    #[must_use]
    pub fn contains(&self, child_id: Id) -> bool {
        find(&Self::rows(), self.parent, child_id).is_some()
    }

    /// The children whose id starts with `prefix`, ascending by id.
    ///
    /// Reads only the one subtree the prefix selects, however deep the trie is.
    #[must_use]
    pub fn children_with_prefix(&self, prefix: &[u8]) -> Vec<ChildInfo> {
        with_prefix(&mut Self::rows(), self.parent, prefix)
            .into_iter()
            .map(|slot| hydrate(S::storage_read, slot))
            .collect()
    }

    /// The children with id at or above `from`, ascending by id, a whole trie
    /// row at a time until `at_least` are collected, and the id to resume from
    /// (`None` when none is left). See [`slots_from`] for what a page reads.
    #[must_use]
    pub fn children_from(&self, from: Id, at_least: usize) -> (Vec<ChildInfo>, Option<Id>) {
        let mut out = Vec::new();
        let resume = slots_from(
            &Self::rows(),
            self.parent,
            &mut Vec::new(),
            Some(&from),
            // A page of none would resume where it started.
            at_least.max(1),
            &mut out,
        );
        let children = out
            .into_iter()
            .map(|slot| hydrate(S::storage_read, slot))
            .collect();
        (children, resume)
    }

    /// [`Self::children_from`] for ids alone: no child's index row is read,
    /// and a page holds fewer than `at_least + BUCKET_MAX` ids.
    #[must_use]
    pub fn child_ids_from(&self, from: Id, at_least: usize) -> (Vec<Id>, Option<Id>) {
        let mut out = Vec::new();
        let resume = slots_from(
            &Self::rows(),
            self.parent,
            &mut Vec::new(),
            Some(&from),
            at_least.max(1),
            &mut out,
        );
        (out.into_iter().map(|slot| slot.id).collect(), resume)
    }

    /// Number of children, without enumerating them. One row read.
    #[must_use]
    pub fn len(&self) -> u64 {
        read_row(&Self::rows(), self.parent, &[]).map_or(0, |row| row.body.count())
    }

    /// The next position to hand a newly linked child.
    ///
    /// A high-water mark rather than `len()`, so a position freed by a removal
    /// is never reissued to a later append. See [`TrieRow::next_order`].
    #[must_use]
    pub fn next_order(&self) -> u64 {
        read_row(&Self::rows(), self.parent, &[]).map_or(0, |row| row.next_order)
    }

    /// Whether the parent has no children.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The trie's root hash — a function of the child set, not of insert order.
    #[must_use]
    pub fn root(&self) -> [u8; 32] {
        read_row(&Self::rows(), self.parent, &[]).map_or(EMPTY, |row| row.body.hash())
    }

    /// Every child's id, ascending, without reading any index row. For callers
    /// that need only ids (hash comparison, cascades). Ascending by id is the
    /// one order every replica agrees on: `created_at` is a local observation.
    #[must_use]
    pub fn child_ids(&self) -> Vec<Id> {
        let mut out = Vec::new();
        collect(
            &mut Self::rows(),
            self.parent,
            &mut Vec::new(),
            &mut out,
            false,
        );
        out.into_iter().map(|slot| slot.id).collect()
    }

    /// Every child, in [`ChildInfo`]'s own order — `(created_at, order, id)`.
    ///
    /// That order is load-bearing, NOT cosmetic: `Vector::get(idx)` walks a
    /// collection's children in it. The trie's layout and its hash are keyed by
    /// id; enumeration order is a separate concern, applied on the way out.
    #[must_use]
    pub fn children(&self) -> Vec<ChildInfo> {
        let mut out = Vec::new();
        collect(
            &mut Self::rows(),
            self.parent,
            &mut Vec::new(),
            &mut out,
            false,
        );
        let mut out: Vec<ChildInfo> = out
            .into_iter()
            .map(|slot| hydrate(S::storage_read, slot))
            .collect();
        out.sort();
        out
    }

    /// Link `child` under `parent` using caller-supplied row access.
    ///
    /// For snapshot sync, which installs entities by writing their rows straight
    /// to the store. Pass the child's SHIPPED `full_hash`: the trie is a pure
    /// function of the `{(id, full_hash)}` set, so rebuilding it from the
    /// sender's values reproduces the sender's root exactly, in any order.
    ///
    /// Runs the same walk as [`insert`](Self::insert), so the rows it writes are
    /// byte-identical to the ones `insert` would.
    pub fn insert_with<R, W>(parent: Id, child: ChildInfo, read: R, write: W)
    where
        R: Fn(Key) -> Option<Vec<u8>>,
        W: FnMut(Key, &[u8]),
    {
        let mut rows = Closures { read, write };
        let _hash = insert_at(&mut rows, parent, &mut Vec::new(), Slot::of(&child));
    }

    /// Remove every row of this trie.
    ///
    /// A deleted entity's trie must go with it. Its rows live in their own
    /// keyspace, and collection ids are deterministic, so deleting and later
    /// re-creating the same field would otherwise find the OLD children folded
    /// into the new incarnation's hash.
    pub fn drop_all(&self) {
        collect(
            &mut Self::rows(),
            self.parent,
            &mut Vec::new(),
            &mut Vec::new(),
            true,
        );
        admitted_count::forget::<S>(self.parent);
    }

    /// A parent's trie root, read through a caller-supplied reader.
    ///
    /// `None` when the root row is present but undecodable: that is not an
    /// empty subtree, and a caller comparing roots must not read it as one.
    pub fn root_with<F>(parent: Id, read: F) -> Option<[u8; 32]>
    where
        F: Fn(Key) -> Option<Vec<u8>>,
    {
        match read(Key::ChildTrie(addr(parent, &[]))) {
            None => Some(EMPTY),
            Some(bytes) => TrieRow::decode(&bytes).map(|row| row.body.hash()),
        }
    }

    /// Enumerate a parent's children using a caller-supplied reader, in the
    /// same order as [`children`](Self::children).
    pub fn children_with<F>(parent: Id, read: F) -> Vec<ChildInfo>
    where
        F: Fn(Key) -> Option<Vec<u8>>,
    {
        let mut rows = Closures {
            read: &read,
            write: |_: Key, _: &[u8]| {},
        };
        let mut slots = Vec::new();
        collect(&mut rows, parent, &mut Vec::new(), &mut slots, false);
        let mut out: Vec<ChildInfo> = slots.into_iter().map(|slot| hydrate(&read, slot)).collect();
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

        let root_b = TrieRow::decode(
            rows.get(&Key::ChildTrie(addr(parent, &[])))
                .expect("insert_with wrote no root node"),
        )
        .expect("root node decodes")
        .body;

        assert_eq!(trie.root(), root_b.hash(), "roots must agree");
        assert_eq!(
            ChildTrie::<crate::store::MainStorage>::root_with(parent, |k| rows.get(&k).cloned()),
            Some(trie.root()),
            "a root read through the caller's rows must agree"
        );
        assert_eq!(trie.len(), root_b.count(), "counts must agree");
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
        let root = TrieRow::decode(
            rows.get(&Key::ChildTrie(addr(other, &[])))
                .expect("root node written"),
        )
        .expect("root node decodes")
        .body;
        let enumerated =
            ChildTrie::<crate::store::MainStorage>::children_with(other, |k| rows.get(&k).cloned());

        assert_eq!(
            root.count() as usize,
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

    /// Paging ids from the start, each page resuming where the last stopped,
    /// visits every child exactly once, ascending, in pages under
    /// `at_least + BUCKET_MAX`.
    #[test]
    fn id_pages_cover_every_child_once_in_order() {
        let trie = ChildTrie::<crate::store::MainStorage>::new(parent(8));
        for i in 0..=u8::MAX {
            let _root = trie.insert(child(i, i));
        }
        let mut seen = Vec::new();
        let mut from = Some(Id::new([0; 32]));
        while let Some(at) = from {
            let (page, next) = trie.child_ids_from(at, 40);
            assert!(page.len() < 40 + BUCKET_MAX, "page of {}", page.len());
            seen.extend(page);
            from = next;
        }
        assert_eq!(seen, trie.child_ids());
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
        static STORE: RefCell<BTreeMap<[u8; crate::store::KEY_LEN], Vec<u8>>> = const { RefCell::new(BTreeMap::new()) };
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

        // Rows per link grow with the trie's depth, log16(n / BUCKET_MAX) + 1,
        // never with n itself. A split can add a level's worth once.
        for (n, _, rows) in &measured {
            let depth = ((*n as f64 / BUCKET_MAX as f64).log(16.0).ceil() as usize) + 1;
            assert!(
                *rows <= depth + 17,
                "rows per link must track depth, not n: n={n} wrote {rows} rows"
            );
        }

        // Bytes grow by at most one node (16 slots) per level, so going from
        // 10k to 50k children, well under one extra level, stays nearly flat.
        let (_, at_10k, _) = measured[2];
        let (_, at_50k, _) = measured[3];
        let growth = at_50k as f64 / at_10k as f64;
        println!("10k -> 50k growth: {growth:.3}x");
        assert!(
            growth < 1.5,
            "cost must track depth, not n; 10k={at_10k} 50k={at_50k}"
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
