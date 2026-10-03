//! How many children a guarded collection admits, kept node-local so that
//! counting them does not read them.
//!
//! # Why this exists
//!
//! A collection in a guarded domain (an `AuthoredVector`, an owned map,
//! `UserStorage`, a collection nested in an owned entry or in a cell) reads only
//! the entries its [`Domain`] admits. Apply cannot hold a peer to that, because
//! the domain belongs to the collection's type and is never stored, so a patched
//! peer can link an entry the collection does not admit: a `Public` one, or an
//! owned one under other rules. The child trie's own count includes those, so
//! `len` could not use it, and it loaded every child instead. A contract that
//! counts on every write (mero-chat derives a message id from the channel's
//! length) then paid gas linear in the channel's history, and a channel stopped
//! taking messages at about 3,200 of them.
//!
//! # Shape
//!
//! One row per collection that has been counted, in the node-local index plane
//! beside `SortedMap`'s validity markers: never synced, never hashed, and
//! written even by a read-only call, so a replica that only reads still counts
//! in constant time from its second read on. The row holds the domain it
//! counts for, the two counts `len` and `keyed_len` need, and the trie root it
//! is exact at.
//!
//! [`before_change`] and [`Pending::finish`] keep it current: every link and
//! unlink through [`ChildTrie`](crate::child_trie::ChildTrie), local or applied
//! from a peer, moves the counts by what the changed child contributes and the
//! root with the trie. A child contributes by the stamp in its own index row,
//! which is what the collection's reads see too. It is trusted only while the root it names is the trie's root, so a
//! path that changes the trie without passing through here (snapshot install
//! writes trie rows directly, and an older binary knows nothing of the row)
//! makes it stale rather than wrong, and the next count loads the children once
//! and records it again.
//!
//! Nothing here reaches the wire or the root hash, so replicas with and without
//! the row, or on versions that do not write it, agree on every answer.

use borsh::{BorshDeserialize, BorshSerialize};

use crate::address::Id;
use crate::child_trie::{ChildTrie, EMPTY};
use crate::collections::is_keyed_entry_of;
use crate::domain::Domain;
use crate::entities::ChildInfo;
use crate::hash_meter::{Digest, Sha256};
use crate::store::StorageAdaptor;

/// Domain separator of the row's id, which is versioned with the row's layout.
const ROW_DOMAIN: &[u8] = b"calimero:admitted-count:v1";

/// Children a collection admits, as `len` and `keyed_len` count them.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct Counts {
    /// Children the domain admits.
    pub(crate) admitted: usize,
    /// Of those, the ones at the keyed owned id kind `parent` gives its
    /// entries: what an owned map or `UserStorage` reads and counts.
    pub(crate) keyed: usize,
}

impl Counts {
    /// What `child` contributes to the counts of a collection at `parent`.
    fn of(domain: &Domain, parent: Id, child: &ChildInfo) -> Self {
        let admitted = domain.admits(&child.metadata.storage_type);
        Self {
            admitted: usize::from(admitted),
            keyed: usize::from(admitted && is_keyed_entry_of(parent, child.id())),
        }
    }

    /// Counts a set of admitted children.
    pub(crate) fn of_admitted<'a>(parent: Id, ids: impl IntoIterator<Item = &'a Id>) -> Self {
        ids.into_iter().fold(Self::default(), |counts, id| Self {
            admitted: counts.admitted + 1,
            keyed: counts.keyed + usize::from(is_keyed_entry_of(parent, *id)),
        })
    }
}

/// The stored row.
#[derive(BorshSerialize, BorshDeserialize)]
struct Tally {
    /// The child-trie root the counts are exact at.
    root: [u8; 32],
    /// The domain the counts are for: the collection's, which admission
    /// depends on and apply never sees.
    domain: Domain,
    admitted: u64,
    keyed: u64,
}

/// Id of `parent`'s row. Derived rather than `parent` itself, which a
/// `SortedMap` or `IndexedMap` at that id already uses for its marker.
fn row_id(parent: Id) -> Id {
    let mut hasher = Sha256::new();
    hasher.update(ROW_DOMAIN);
    hasher.update(parent.as_bytes());
    Id::new(hasher.finalize().into())
}

fn read<S: StorageAdaptor>(parent: Id) -> Option<Tally> {
    if !S::index_supported() {
        return None;
    }
    let bytes = S::index_meta_get(row_id(parent))?;
    // An undecodable row is a stale one: the next count rebuilds it.
    Tally::try_from_slice(&bytes).ok()
}

fn write<S: StorageAdaptor>(parent: Id, tally: &Tally) {
    if let Ok(bytes) = borsh::to_vec(tally) {
        let _persisted = S::index_meta_put(row_id(parent), &bytes);
    }
}

/// The counts of `parent`'s children under `domain`, or the trie root to
/// [`record`] them at once the caller has loaded the children.
///
/// Known without loading the children for an empty trie, and when the row is
/// current for this domain: one trie row read, plus the row.
pub(crate) fn current<S: StorageAdaptor>(parent: Id, domain: &Domain) -> Result<Counts, [u8; 32]> {
    let root = ChildTrie::<S>::new(parent).root();
    if root == EMPTY {
        return Ok(Counts::default());
    }
    match read::<S>(parent) {
        Some(tally) if tally.root == root && tally.domain == *domain => Ok(Counts {
            admitted: usize::try_from(tally.admitted).unwrap_or(usize::MAX),
            keyed: usize::try_from(tally.keyed).unwrap_or(usize::MAX),
        }),
        _ => Err(root),
    }
}

/// Records `counts`, computed by loading `parent`'s children, as exact at
/// `root`: the one [`current`] read BEFORE they were loaded. The root now could
/// be a later one, which a link racing the load (a host-side apply) moved it to
/// without these counts seeing it.
pub(crate) fn record<S: StorageAdaptor>(
    parent: Id,
    domain: &Domain,
    root: [u8; 32],
    counts: Counts,
) {
    if !S::index_supported() {
        return;
    }
    write::<S>(
        parent,
        &Tally {
            root,
            domain: domain.clone(),
            admitted: counts.admitted as u64,
            keyed: counts.keyed as u64,
        },
    );
}

/// `parent`'s row, read before a change to its trie, when it is current.
///
/// Read first, so a collection that has never been counted pays for none of
/// this: no row, no root read, no classification.
pub(crate) fn before_change<S: StorageAdaptor>(parent: Id) -> Option<Pending> {
    before_change_with::<S>(parent, || ChildTrie::<S>::new(parent).root())
}

/// [`before_change`] for a caller holding the trie's root row already, from a
/// descent on its way to the change: `root` gives the root as it stands, and is
/// asked only when the row is there to compare.
pub(crate) fn before_change_with<S: StorageAdaptor>(
    parent: Id,
    root: impl FnOnce() -> [u8; 32],
) -> Option<Pending> {
    let tally = read::<S>(parent)?;
    (root() == tally.root).then_some(Pending { parent, tally })
}

/// [`before_change`] for a caller that read the trie's root, as it stood before
/// the change, on its way to making it, so the root row is not read again.
pub(crate) fn before_change_at<S: StorageAdaptor>(parent: Id, root: [u8; 32]) -> Option<Pending> {
    let tally = read::<S>(parent)?;
    (root == tally.root).then_some(Pending { parent, tally })
}

/// A current row, waiting for the change it was read before.
pub(crate) struct Pending {
    parent: Id,
    tally: Tally,
}

impl Pending {
    /// Carries the row across the change: `unlinked` is the child it removed,
    /// `linked` the one it added (neither, for a child replaced in place), and
    /// the trie's root is now `new_root`.
    ///
    /// A replaced child moves nothing, because what a collection admits is
    /// decided by the stamp in the child's own index row, and nothing rewrites
    /// a linked child's stamp across that line: a re-link keeps the stored
    /// metadata, a signature patch keeps the owner, rules and anchor that
    /// [`Domain::admits`] compares, and the one setter that does rewrite a stamp
    /// (`Index::set_storage_type`, used only by tests) drops the parent's row
    /// itself.
    pub(crate) fn finish<S: StorageAdaptor>(
        mut self,
        new_root: [u8; 32],
        unlinked: Option<&ChildInfo>,
        linked: Option<&ChildInfo>,
    ) {
        let (parent, domain) = (self.parent, &self.tally.domain);
        let contribution = |child: Option<&ChildInfo>| {
            child.map_or_else(Counts::default, |child| Counts::of(domain, parent, child))
        };
        let (removed, added) = (contribution(unlinked), contribution(linked));
        let moved = |count: u64, removed: usize, added: usize| {
            count
                .checked_sub(removed as u64)
                .map(|count| count + added as u64)
        };
        let (Some(admitted), Some(keyed)) = (
            moved(self.tally.admitted, removed.admitted, added.admitted),
            moved(self.tally.keyed, removed.keyed, added.keyed),
        ) else {
            // A count that would fall below zero was not exact: leave the row
            // stale, so the next count rebuilds it rather than trusting it.
            return;
        };
        self.tally.root = new_root;
        self.tally.admitted = admitted;
        self.tally.keyed = keyed;
        write::<S>(parent, &self.tally);
    }
}

/// Drops `parent`'s row with its trie.
pub(crate) fn forget<S: StorageAdaptor>(parent: Id) {
    if S::index_supported() {
        let _persisted = S::index_meta_clear(row_id(parent));
    }
}
