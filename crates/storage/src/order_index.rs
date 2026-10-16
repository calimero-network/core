//! Where each child of a collection sits in [`ChildInfo`]'s order, kept
//! node-local so that reading one position does not read every child.
//!
//! # Why this exists
//!
//! A [`Vector`](crate::collections::Vector) addresses its entries by position,
//! and a position is a child's rank in `(created_at, order, id)` order. The
//! child trie is keyed by id, which pushes mint at random, so it cannot answer
//! "which child is 19,980th": `nth` loaded every child's index row and sorted
//! them, once per call. A contract that reads the newest page of a long list
//! then paid gas linear in the list's history, and mero-chat stopped being able
//! to open a channel at about 16,000 messages, although it only ever asks for
//! the newest 20.
//!
//! # Shape
//!
//! A counted B+ tree over the children's sort keys, one row per node, in the
//! node-local index plane beside [`admitted_count`](crate::admitted_count):
//! never synced, never hashed, and written even by a read-only call. A leaf
//! holds up to [`LEAF_MAX`] keys in order; an inner node holds up to
//! [`INNER_MAX`] branches, each with its subtree's size and last key. So `nth`
//! and `position_of` descend by counts and read one row per level plus the
//! header: four rows for a hundred thousand children.
//!
//! The header names the trie root the tree is exact at and the domain it
//! indexes for, as `admitted_count`'s row does. [`before_change`] and
//! [`Pending::finish`] keep it current across every link and unlink through
//! [`ChildTrie`](crate::child_trie::ChildTrie), local or applied from a peer.
//! A path that changes the trie without passing through here (snapshot install
//! writes trie rows directly, and an older binary knows nothing of the tree)
//! leaves the header naming an older root, so the next positional read loads
//! the children once and builds the tree again.
//!
//! A linked child's sort key never moves: `created_at` and `order` are written
//! when the child is linked, a re-link keeps the stored metadata, and a value
//! write rewrites only `updated_at`. So a replaced child moves nothing here.
//!
//! Nothing here reaches the wire or the root hash, so replicas with and without
//! the tree, or on versions that do not write it, agree on every answer.

use borsh::{BorshDeserialize, BorshSerialize};

use crate::address::Id;
use crate::child_trie::ChildTrie;
use crate::domain::Domain;
use crate::entities::ChildInfo;
use crate::hash_meter::{Digest, Sha256};
use crate::store::StorageAdaptor;

/// Domain separator of the header's row id, versioned with the layout.
const HEADER_DOMAIN: &[u8] = b"calimero:order-index:v1:header";

/// Domain separator of a node's row id, versioned with the layout.
const NODE_DOMAIN: &[u8] = b"calimero:order-index:v1:node";

/// Most keys a leaf holds before it splits in two. A key is 48 bytes, so a full
/// leaf is a ~6 KiB row.
pub(crate) const LEAF_MAX: usize = 128;

/// Most branches an inner node holds before it splits in two.
pub(crate) const INNER_MAX: usize = 64;

/// A child's place in [`ChildInfo`]'s order. The field order is the comparison
/// order, and it must stay `ChildInfo::cmp`'s: `created_at`, then `order`, then
/// `id`.
#[derive(BorshDeserialize, BorshSerialize, Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct SortKey {
    created_at: u64,
    order: u64,
    id: Id,
}

impl SortKey {
    pub(crate) const fn of(child: &ChildInfo) -> Self {
        Self {
            created_at: child.metadata.created_at,
            order: child.metadata.order,
            id: child.id(),
        }
    }
}

/// One subtree as its parent node records it.
#[derive(BorshDeserialize, BorshSerialize, Clone, Copy, Debug)]
struct Branch {
    node: u32,
    count: u64,
    last: SortKey,
}

#[derive(BorshDeserialize, BorshSerialize, Clone, Debug)]
enum Node {
    Leaf(Vec<SortKey>),
    Inner(Vec<Branch>),
}

impl Node {
    /// The branch that stands for this node, stored as `node`. `None` for an
    /// empty node, which has no last key and is never linked from a parent.
    fn branch(&self, node: u32) -> Option<Branch> {
        match self {
            Self::Leaf(keys) => keys.last().map(|last| Branch {
                node,
                count: keys.len() as u64,
                last: *last,
            }),
            Self::Inner(branches) => branches.last().map(|last| Branch {
                node,
                count: branches.iter().map(|branch| branch.count).sum(),
                last: last.last,
            }),
        }
    }
}

/// The stored header.
#[derive(BorshDeserialize, BorshSerialize)]
struct Header {
    /// The child-trie root the tree is exact at. `None` after a change the tree
    /// could not carry: never current, so the next read rebuilds.
    root: Option<[u8; 32]>,
    /// The domain whose admitted children the tree holds: the collection's,
    /// which admission depends on and apply never sees.
    domain: Domain,
    /// The node at the top of the tree.
    top: u32,
    /// The number the next new node takes. Every node in use is below it, so a
    /// rebuild knows which rows to clear.
    next_node: u32,
}

fn header_id(parent: Id) -> Id {
    let mut hasher = Sha256::new();
    hasher.update(HEADER_DOMAIN);
    hasher.update(parent.as_bytes());
    Id::new(hasher.finalize().into())
}

fn node_id(parent: Id, node: u32) -> Id {
    let mut hasher = Sha256::new();
    hasher.update(NODE_DOMAIN);
    hasher.update(parent.as_bytes());
    hasher.update(node.to_le_bytes());
    Id::new(hasher.finalize().into())
}

fn read_header<S: StorageAdaptor>(parent: Id) -> Option<Header> {
    if !S::index_supported() {
        return None;
    }
    let bytes = S::index_meta_get(header_id(parent))?;
    // An undecodable header is a stale one: the next read rebuilds the tree.
    Header::try_from_slice(&bytes).ok()
}

fn write_header<S: StorageAdaptor>(parent: Id, header: &Header) {
    if let Ok(bytes) = borsh::to_vec(header) {
        let _persisted = S::index_meta_put(header_id(parent), &bytes);
    }
}

fn read_node<S: StorageAdaptor>(parent: Id, node: u32) -> Option<Node> {
    let bytes = S::index_meta_get(node_id(parent, node))?;
    Node::try_from_slice(&bytes).ok()
}

fn write_node<S: StorageAdaptor>(parent: Id, node: u32, body: &Node) {
    if let Ok(bytes) = borsh::to_vec(body) {
        let _persisted = S::index_meta_put(node_id(parent, node), &bytes);
    }
}

fn clear_node<S: StorageAdaptor>(parent: Id, node: u32) {
    let _persisted = S::index_meta_clear(node_id(parent, node));
}

/// The tree of a collection whose header is current, for positional reads.
pub(crate) struct Tree {
    parent: Id,
    top: u32,
}

impl Tree {
    /// The id of the child at `index`, or `None` past the end.
    ///
    /// `Err(())` when a node the header points at is missing or undecodable:
    /// the caller loads the children instead.
    pub(crate) fn nth<S: StorageAdaptor>(&self, index: usize) -> Result<Option<Id>, ()> {
        let mut remaining = index as u64;
        let mut node = self.top;
        loop {
            match read_node::<S>(self.parent, node).ok_or(())? {
                Node::Leaf(keys) => {
                    let at = usize::try_from(remaining).map_err(|_| ())?;
                    return Ok(keys.get(at).map(|key| key.id));
                }
                Node::Inner(branches) => {
                    let Some(branch) = branches.iter().find(|branch| {
                        let inside = remaining < branch.count;
                        if !inside {
                            remaining -= branch.count;
                        }
                        inside
                    }) else {
                        return Ok(None);
                    };
                    node = branch.node;
                }
            }
        }
    }

    /// The position of the child with sort key `key`, or `None` when the tree
    /// does not hold it.
    ///
    /// `Err(())` as for [`nth`](Self::nth).
    pub(crate) fn position_of<S: StorageAdaptor>(&self, key: SortKey) -> Result<Option<usize>, ()> {
        let mut before = 0_u64;
        let mut node = self.top;
        loop {
            match read_node::<S>(self.parent, node).ok_or(())? {
                Node::Leaf(keys) => {
                    let Ok(at) = keys.binary_search(&key) else {
                        return Ok(None);
                    };
                    let position = before + at as u64;
                    return usize::try_from(position).map(Some).map_err(|_| ());
                }
                Node::Inner(branches) => {
                    let Some(branch) = branches.iter().find(|branch| {
                        let inside = key <= branch.last;
                        if !inside {
                            before += branch.count;
                        }
                        inside
                    }) else {
                        return Ok(None);
                    };
                    node = branch.node;
                }
            }
        }
    }
}

/// The tree of `parent`'s children under `domain`, or the trie root to
/// [`record`] it at once the caller has loaded the children.
///
/// One trie row read and the header.
pub(crate) fn current<S: StorageAdaptor>(parent: Id, domain: &Domain) -> Result<Tree, [u8; 32]> {
    let root = ChildTrie::<S>::new(parent).root();
    match read_header::<S>(parent) {
        Some(header) if header.root == Some(root) && header.domain == *domain => Ok(Tree {
            parent,
            top: header.top,
        }),
        _ => Err(root),
    }
}

/// Builds the tree of `children`, the ones `parent`'s collection admits in
/// [`ChildInfo`]'s order, loaded by a caller that read `root` BEFORE loading
/// them (see `admitted_count::record` for why that root and not the current).
pub(crate) fn record<S: StorageAdaptor>(
    parent: Id,
    domain: &Domain,
    root: [u8; 32],
    children: &[ChildInfo],
) {
    if !S::index_supported() {
        return;
    }
    // The rows of a tree this replaces would otherwise stay behind for good:
    // nothing else knows their numbers.
    if let Some(old) = read_header::<S>(parent) {
        for node in 0..old.next_node {
            clear_node::<S>(parent, node);
        }
    }

    let mut next_node = 0_u32;
    let mut level: Vec<Branch> = Vec::new();
    for chunk in children.chunks(LEAF_MAX) {
        let leaf = Node::Leaf(chunk.iter().map(SortKey::of).collect());
        level.extend(leaf.branch(next_node));
        write_node::<S>(parent, next_node, &leaf);
        next_node += 1;
    }
    let top = loop {
        match level.as_slice() {
            [] => {
                write_node::<S>(parent, next_node, &Node::Leaf(Vec::new()));
                next_node += 1;
                break next_node - 1;
            }
            [only] => break only.node,
            _ => {}
        }
        let mut above = Vec::new();
        for chunk in level.chunks(INNER_MAX) {
            let inner = Node::Inner(chunk.to_vec());
            above.extend(inner.branch(next_node));
            write_node::<S>(parent, next_node, &inner);
            next_node += 1;
        }
        level = above;
    };

    write_header::<S>(
        parent,
        &Header {
            root: Some(root),
            domain: domain.clone(),
            top,
            next_node,
        },
    );
}

/// `parent`'s header, read before a change to its trie, when it is current.
///
/// Read first, so a collection never read by position pays for none of this.
pub(crate) fn before_change<S: StorageAdaptor>(parent: Id) -> Option<Pending> {
    before_change_with::<S>(parent, || ChildTrie::<S>::new(parent).root())
}

/// [`before_change`] for a caller holding the trie's root row already: `root`
/// is asked only when there is a header to compare it with.
pub(crate) fn before_change_with<S: StorageAdaptor>(
    parent: Id,
    root: impl FnOnce() -> [u8; 32],
) -> Option<Pending> {
    let header = read_header::<S>(parent)?;
    (header.root == Some(root())).then_some(Pending { parent, header })
}

/// [`before_change`] for a caller that read the trie's root, as it stood before
/// the change, on its way to making it.
pub(crate) fn before_change_at<S: StorageAdaptor>(parent: Id, root: [u8; 32]) -> Option<Pending> {
    let header = read_header::<S>(parent)?;
    (header.root == Some(root)).then_some(Pending { parent, header })
}

/// A current header, waiting for the change it was read before.
pub(crate) struct Pending {
    parent: Id,
    header: Header,
}

impl Pending {
    /// Carries the tree across the change: `unlinked` is the child it removed,
    /// `linked` the one it added (neither, for a child replaced in place, whose
    /// sort key does not move), and the trie's root is now `new_root`.
    pub(crate) fn finish<S: StorageAdaptor>(
        mut self,
        new_root: [u8; 32],
        unlinked: Option<&ChildInfo>,
        linked: Option<&ChildInfo>,
    ) {
        let parent = self.parent;
        let admitted = |child: Option<&ChildInfo>| {
            child
                .filter(|child| self.header.domain.admits(&child.metadata.storage_type))
                .map(SortKey::of)
        };
        let (removed, added) = (admitted(unlinked), admitted(linked));

        let moved = removed
            .map_or(Some(()), |key| self.remove::<S>(key))
            .and_then(|()| added.map_or(Some(()), |key| self.insert::<S>(key)));
        // A key that was not where the tree said, or a node that could not be
        // read: the tree was not exact. Leave it stale so the next read
        // rebuilds it; the header keeps `next_node`, so that rebuild clears
        // every node this change wrote.
        self.header.root = moved.map(|()| new_root);
        write_header::<S>(parent, &self.header);
    }

    fn insert<S: StorageAdaptor>(&mut self, key: SortKey) -> Option<()> {
        let top = self.header.top;
        let branches = self.insert_into::<S>(top, key)?;
        if let [single] = branches.as_slice() {
            self.header.top = single.node;
            return Some(());
        }
        // The top split: a new node above it holds both halves.
        let node = self.allocate();
        write_node::<S>(self.parent, node, &Node::Inner(branches));
        self.header.top = node;
        Some(())
    }

    /// Inserts `key` beneath `node` and returns the branches that now stand for
    /// it: one, or two when it split. `None` when the tree was not exact.
    fn insert_into<S: StorageAdaptor>(&mut self, node: u32, key: SortKey) -> Option<Vec<Branch>> {
        let body = match read_node::<S>(self.parent, node)? {
            Node::Leaf(mut keys) => {
                let at = keys.binary_search(&key).err()?;
                keys.insert(at, key);
                if keys.len() <= LEAF_MAX {
                    Node::Leaf(keys)
                } else {
                    let right = Node::Leaf(keys.split_off(keys.len() / 2));
                    return Some(self.split::<S>(node, Node::Leaf(keys), right));
                }
            }
            Node::Inner(mut branches) => {
                // The first subtree whose last key is not before `key` (so a
                // key the tree holds already is found, and refused, where it
                // is), or the last subtree when `key` is past them all.
                let at = branches
                    .iter()
                    .position(|branch| key <= branch.last)
                    .unwrap_or(branches.len().checked_sub(1)?);
                let replaced = self.insert_into::<S>(branches[at].node, key)?;
                drop(branches.splice(at..=at, replaced));
                if branches.len() <= INNER_MAX {
                    Node::Inner(branches)
                } else {
                    let right = Node::Inner(branches.split_off(branches.len() / 2));
                    return Some(self.split::<S>(node, Node::Inner(branches), right));
                }
            }
        };
        write_node::<S>(self.parent, node, &body);
        Some(vec![body.branch(node)?])
    }

    /// Stores `left` as `node` and `right` as a new node beside it.
    fn split<S: StorageAdaptor>(&mut self, node: u32, left: Node, right: Node) -> Vec<Branch> {
        let sibling = self.allocate();
        write_node::<S>(self.parent, node, &left);
        write_node::<S>(self.parent, sibling, &right);
        left.branch(node)
            .into_iter()
            .chain(right.branch(sibling))
            .collect()
    }

    fn remove<S: StorageAdaptor>(&mut self, key: SortKey) -> Option<()> {
        let top = self.header.top;
        match self.remove_from::<S>(top, key)? {
            // The tree is now empty: keep the top as an empty leaf.
            None => write_node::<S>(self.parent, top, &Node::Leaf(Vec::new())),
            Some(_) => {
                // A top with one branch is a level that holds nothing up.
                while let Node::Inner(branches) = read_node::<S>(self.parent, self.header.top)? {
                    let [only] = branches.as_slice() else {
                        break;
                    };
                    clear_node::<S>(self.parent, self.header.top);
                    self.header.top = only.node;
                }
            }
        }
        Some(())
    }

    /// Removes `key` beneath `node` and returns the branch that now stands for
    /// it: `Some(None)` when it emptied and was cleared, `None` when the tree
    /// was not exact.
    fn remove_from<S: StorageAdaptor>(
        &mut self,
        node: u32,
        key: SortKey,
    ) -> Option<Option<Branch>> {
        let body = match read_node::<S>(self.parent, node)? {
            Node::Leaf(mut keys) => {
                let at = keys.binary_search(&key).ok()?;
                let _removed = keys.remove(at);
                Node::Leaf(keys)
            }
            Node::Inner(mut branches) => {
                let at = branches.iter().position(|branch| key <= branch.last)?;
                match self.remove_from::<S>(branches[at].node, key)? {
                    Some(branch) => branches[at] = branch,
                    None => {
                        let _emptied = branches.remove(at);
                    }
                }
                Node::Inner(branches)
            }
        };
        let Some(branch) = body.branch(node) else {
            clear_node::<S>(self.parent, node);
            return Some(None);
        };
        write_node::<S>(self.parent, node, &body);
        Some(Some(branch))
    }

    fn allocate(&mut self) -> u32 {
        let node = self.header.next_node;
        self.header.next_node += 1;
        node
    }
}

/// Drops `parent`'s tree with its trie.
pub(crate) fn forget<S: StorageAdaptor>(parent: Id) {
    if !S::index_supported() {
        return;
    }
    if let Some(header) = read_header::<S>(parent) {
        for node in 0..header.next_node {
            clear_node::<S>(parent, node);
        }
    }
    let _persisted = S::index_meta_clear(header_id(parent));
}

#[cfg(test)]
mod tests;
