//! Entities for the storage system.
//!
//! This module contains the entities that are used to represent the elements in
//! the storage system. These are the building blocks that are used to construct
//! the hierarchy of the storage, and to apply updates to the elements.
//!

#[cfg(test)]
#[path = "tests/entities.rs"]
mod tests;

use core::fmt::{self, Display, Formatter};
use std::time::{SystemTime, UNIX_EPOCH};

use borsh::{BorshDeserialize, BorshSerialize};

use crate::address::{Id, Path};

/// The primary data for an [`Element`], that is, the data that the consumer
/// application has stored in the [`Element`].
#[derive(BorshDeserialize, BorshSerialize, Clone, Debug, Eq, Hash, PartialEq, PartialOrd, Ord)]
#[non_exhaustive]
pub struct Data;

/// Represents an [`Element`] in the storage.
///
/// This is a simple model of an [`Element`] in the storage system, with a
/// unique identifier and a path to the [`Element`] in the hierarchy. Together,
/// these properties give the means of addressing that are necessary in order to
/// locate the [`Element`] and apply updates.
///
/// Note, this is modelled as a single entity called "Element" rather than
/// separating into separate "Node" and "Leaf" entities, to simplify the
/// handling via the storage [`Interface`](crate::interface::Interface). The
/// actual nature of the [`Element`] can be determined by inspection.
///
/// # Updates
///
/// When an [`Element`] is updated, the [`Element`] is marked as dirty, and the
/// [`updated_at()`](Element::updated_at()) timestamp is updated. This is used
/// to determine the freshness of the data, and to resolve conflicts when
/// multiple parties are updating the same [`Element`] concurrently, on a "last
/// write wins" basis.
///
/// An [`Element`] is considered to be an atomic unit, but this designation
/// applies only to it, and not its children. Its children are separate
/// entities, and are not part of the "state" of an [`Element`] for update
/// comparison purposes. However, they do matter for calculating the Merkle tree
/// hashes that represent the sum of all data at that part in the hierarchy and
/// below.
///
/// # Structure
///
/// TODO: Update when the `child_ids` field is replaced with an index.
///
/// At present the [`Element`] has a `child_ids` field, which memoises the IDs
/// of the children. This is because it is better than traversing the whole
/// database to find the children (!).
///
/// Having a `child_ids` field is however non-optimal — this should come from
/// outside the struct. We should be able to look up the children by path, and
/// so given that the path is the primary method of determining that an
/// [`Element`] is a child of another, this should be the mechanism relied upon.
/// Therefore, maintaining a list of child IDs is a second point of maintenance
/// and undesirable, as well as being restrictive for larger child sets.
///
/// An index would be a better approach, but this will be done later, as that is
/// purely an optimisation aspect and not a functional one, and it is better to
/// get the shape of the functionality correct, working, and testable first.
///
/// Given that we do need to obtain the children, and given also that indexes
/// are a later optimisation, the best interim approach is therefore to simply
/// store the child IDs against the struct.
///
/// This reasoning is, however, why we do not have a parent ID field. The path
/// should be sufficient to determine the parent, and so a parent ID is
/// redundant. It is also another point of maintenance and data to keep
/// consistent. For instance, moving an item in the tree should be as simple as
/// updating the path and recalculating affected [`Element`]s' hashes. If there
/// is a parent ID then that also has to be synced, and that does not give us
/// anything that we cannot already get via the path.
///
/// There are approaches that work with relational database when storing
/// hierarchies, such as BetterNestedSet to combine a NestedSet (with left and
/// right partitioning) with an AdjacencyList (parent IDs). However, operations
/// to update things like that are fast in relational databases but slow in
/// key-value stores. Therefore, this approach and other similar patterns are
/// not suitable for our use case.
///
#[derive(BorshDeserialize, BorshSerialize, Clone, Debug, Eq, Hash, PartialEq, PartialOrd, Ord)]
#[non_exhaustive]
pub struct Element {
    /// The unique identifier for the [`Element`].
    id: Id,

    /// The unique identifiers of the children of the [`Element`]. This is
    /// considered somewhat temporary, as there are efficiency gains to be made
    /// by storing this list elsewhere — but for now, it helps to get the data
    /// in place and usable, and establish a basis to test against and enhance.
    pub(crate) child_ids: Vec<Id>,

    /// The primary data for the [`Element`], that is, the data that the
    /// consumer application has stored in the [`Element`].
    data: Data,

    /// Whether the [`Element`] is dirty, i.e. has been modified since it was
    /// last saved.
    pub(crate) is_dirty: bool,

    /// The metadata for the [`Element`]. This represents a range of
    /// system-managed properties that are used to process the [`Element`], but
    /// are not part of the primary data.
    pub(crate) metadata: Metadata,

    /// The path to the [`Element`] in the hierarchy of the storage.
    path: Path,
}

impl Element {
    /// Creates a new [`Element`].
    ///
    /// When created, the [`Element`] does not yet exist in the storage system,
    /// and needs to be saved in order to be persisted. It is auto-assigned a
    /// unique ID, but the path in the data hierarchy must be provided.
    ///
    /// This method is intended for creating brand-new [`Element`]s, and not for
    /// creating [`Element`]s that have been received from other parties. The
    /// intended approach there is that these will be created through
    /// deserialisation.
    ///
    /// # Parameters
    ///
    /// * `path` - The path to the [`Element`] in the hierarchy of the storage.
    ///
    /// # Panics
    ///
    /// This method can technically panic if the system time goes backwards, to
    /// before the Unix epoch, which should never ever happen!
    ///
    #[must_use]
    pub fn new(path: &Path) -> Self {
        #[allow(clippy::cast_possible_truncation)] // Impossible to overflow in normal circumstances
        #[allow(clippy::expect_used)] // Effectively infallible here
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("Time went backwards to before the Unix epoch!")
            .as_nanos() as u64;
        Self {
            id: Id::new(),
            child_ids: Vec::new(),
            data: Data {},
            is_dirty: true,
            metadata: Metadata {
                created_at: timestamp,
                updated_at: timestamp,
            },
            path: path.clone(),
        }
    }

    /// The unique identifiers of the children of the [`Element`].
    ///
    /// This gets only the IDs of the children of the [`Element`], which are the
    /// [`Element`]s that are directly below this [`Element`] in the hierarchy.
    ///
    /// TODO: This method will likely move to the [`Interface`] when the index
    ///       is implemented.
    ///
    #[must_use]
    pub fn child_ids(&self) -> Vec<Id> {
        self.child_ids.clone()
    }

    /// The timestamp when the [`Element`] was first created.
    #[must_use]
    pub const fn created_at(&self) -> u64 {
        self.metadata.created_at
    }

    /// The primary data for the [`Element`].
    ///
    /// This gets the primary data for the [`Element`], that is, the data that
    /// the consumer application has stored in the [`Element`]. This is the data
    /// that the [`Element`] is primarily concerned with, and for the management
    /// of which the [`Element`] exists.
    ///
    #[must_use]
    pub const fn data(&self) -> &Data {
        &self.data
    }

    /// Whether the [`Element`] has children.
    ///
    /// This checks whether the [`Element`] has children, which are the
    /// [`Element`]s that are directly below this [`Element`] in the hierarchy.
    ///
    /// TODO: This method will likely move to the [`Interface`] when the index
    ///       is implemented.
    ///
    #[must_use]
    pub fn has_children(&self) -> bool {
        !self.child_ids.is_empty()
    }

    /// The unique identifier for the [`Element`].
    ///
    /// This is the unique identifier for the [`Element`], which can always be
    /// used to locate the [`Element`] in the storage system. It is generated
    /// when the [`Element`] is first created, and never changes. It is
    /// reflected onto all other systems and so is universally consistent.
    ///
    #[must_use]
    pub const fn id(&self) -> Id {
        self.id
    }

    /// Whether the [`Element`] is dirty.
    ///
    /// This checks whether the [`Element`] is dirty, i.e. has been modified
    /// since it was last saved. This is used to determine whether the
    /// [`Element`] needs to be saved again in order to persist the changes.
    ///
    #[must_use]
    pub const fn is_dirty(&self) -> bool {
        self.is_dirty
    }

    /// The metadata for the [`Element`].
    ///
    /// This gets the metadata for the [`Element`]. This represents a range of
    /// system-managed properties that are used to process the [`Element`], but
    /// are not part of the primary data. This is the data that the system uses
    /// to manage the [`Element`], and is not intended to be directly
    /// manipulated by the consumer application.
    ///
    #[must_use]
    pub const fn metadata(&self) -> &Metadata {
        &self.metadata
    }

    /// The path to the [`Element`] in the hierarchy.
    ///
    /// This is the path to the [`Element`] in the hierarchy of the storage
    /// system. It is an important primary method of accessing [`Element`]s, but
    /// they can potentially move, and so the [`Id`] is the fixed and consistent
    /// means of locating an [`Element`].
    ///
    #[must_use]
    pub fn path(&self) -> Path {
        self.path.clone()
    }

    /// Updates the data for the [`Element`].
    ///
    /// This updates the data for the [`Element`], and marks the [`Element`] as
    /// dirty. This is used to indicate that the [`Element`] has been modified
    /// since it was last saved, and that it needs to be saved again in order to
    /// persist the changes.
    ///
    /// It also updates the [`updated_at()`](Element::updated_at()) timestamp to
    /// reflect the time that the [`Element`] was last updated.
    ///
    /// # Parameters
    ///
    /// * `data` - The new data for the [`Element`].
    ///
    /// # Panics
    ///
    /// This method can technically panic if the system time goes backwards, to
    /// before the Unix epoch, which should never ever happen!
    ///
    #[allow(clippy::cast_possible_truncation)] // Impossible to overflow in normal circumstances
    #[allow(clippy::expect_used)] // Effectively infallible here
    pub fn update_data(&mut self, data: Data) {
        self.data = data;
        self.is_dirty = true;
        self.metadata.updated_at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("Time went backwards to before the Unix epoch!")
            .as_nanos() as u64;
    }

    /// The timestamp when the [`Element`] was last updated.
    #[must_use]
    pub const fn updated_at(&self) -> u64 {
        self.metadata.updated_at
    }
}

impl Display for Element {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "Element {}: {}", self.id, self.path)
    }
}

/// The metadata for an [`Element`].
///
/// This represents a range of system-managed properties that are used to
/// process the [`Element`], but are not part of the primary data.
///
/// # Timestamps
///
/// The timestamp fields, i.e. [`created_at()`](Element::created_at()) and
/// [`updated_at()`](Element::updated_at()), are stored using [`u64`] integer
/// values. This is because [Chrono](https://crates.io/crates/chrono) does not
/// support [Borsh](https://crates.io/crates/borsh) serialisation, and also
/// using a 64-bit integer is faster and more efficient (as Chrono uses 96 bits
/// internally).
///
/// Using a [`u64`] timestamp allows for 585 years from the Unix epoch, at
/// nanosecond precision. This is more than sufficient for our current needs.
///
#[derive(BorshDeserialize, BorshSerialize, Clone, Debug, Eq, Hash, PartialEq, PartialOrd, Ord)]
#[non_exhaustive]
pub struct Metadata {
    /// When the [`Element`] was first created. Note that this is a global
    /// creation time, and does not reflect the time that the [`Element`] was
    /// added to the local storage.
    created_at: u64,

    /// When the [`Element`] was last updated. This is the time that the
    /// [`Element`] was last modified in any way, and is used to determine the
    /// freshness of the data. It is critical for the "last write wins" strategy
    /// that is used to resolve conflicts.
    pub(crate) updated_at: u64,
}
