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
//! [`relink`] keeps it current: every link and unlink through
//! [`ChildTrie`](crate::child_trie::ChildTrie), local or applied from a peer,
//! moves the counts by what the changed child contributes and the root with the
//! trie. It is trusted only while the root it names is the trie's root, so a
//! path that changes the trie without passing through here (snapshot install
//! writes trie rows directly, and an older binary knows nothing of the row)
//! makes it stale rather than wrong, and the next count loads the children once
//! and records it again.
//!
//! Nothing here reaches the wire or the root hash, so replicas with and without
//! the row, or on versions that do not write it, agree on every answer.

use borsh::{BorshDeserialize, BorshSerialize};
use sha2::{Digest, Sha256};

use crate::address::Id;
use crate::child_trie::{ChildTrie, EMPTY};
use crate::collections::is_keyed_entry_of;
use crate::domain::Domain;
use crate::entities::ChildInfo;
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

/// Carries `parent`'s row across one change to its trie: `before` is the child
/// the change replaced or removed, `after` the one it linked, and the trie's
/// root moved from `old_root` to `new_root`. A row that was not current before
/// the change is left as it is, still stale.
pub(crate) fn relink<S: StorageAdaptor>(
    parent: Id,
    old_root: [u8; 32],
    new_root: [u8; 32],
    before: Option<&ChildInfo>,
    after: Option<&ChildInfo>,
) {
    let Some(mut tally) = read::<S>(parent) else {
        return;
    };
    if tally.root != old_root {
        return;
    }
    let contribution = |child: Option<&ChildInfo>| {
        child.map_or_else(Counts::default, |child| {
            Counts::of(&tally.domain, parent, child)
        })
    };
    let (removed, added) = (contribution(before), contribution(after));
    let moved = |count: u64, removed: usize, added: usize| {
        count
            .checked_sub(removed as u64)
            .map(|count| count + added as u64)
    };
    let (Some(admitted), Some(keyed)) = (
        moved(tally.admitted, removed.admitted, added.admitted),
        moved(tally.keyed, removed.keyed, added.keyed),
    ) else {
        // A count that would fall below zero was not exact: leave the row
        // stale, so the next count rebuilds it rather than trusting it.
        return;
    };
    tally.root = new_root;
    tally.admitted = admitted;
    tally.keyed = keyed;
    write::<S>(parent, &tally);
}

/// Drops `parent`'s row with its trie.
pub(crate) fn forget<S: StorageAdaptor>(parent: Id) {
    if S::index_supported() {
        let _persisted = S::index_meta_clear(row_id(parent));
    }
}
