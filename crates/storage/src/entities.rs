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

use crate::address::{Id, Path};

/// The primary data for an [`Element`], that is, the data that the consumer
/// application has stored in the [`Element`].
#[derive(Clone, Debug, Eq, Hash, PartialEq, PartialOrd, Ord)]
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
#[derive(Clone, Debug, Eq, Hash, PartialEq, PartialOrd, Ord)]
#[non_exhaustive]
pub struct Element {
    /// The unique identifier for the [`Element`].
    id: Id,

    /// The primary data for the [`Element`], that is, the data that the
    /// consumer application has stored in the [`Element`].
    data: Data,

    /// The metadata for the [`Element`]. This represents a range of
    /// system-managed properties that are used to process the [`Element`], but
    /// are not part of the primary data.
    metadata: Metadata,

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
    #[must_use]
    pub fn new(path: &Path) -> Self {
        Self {
            id: Id::new(),
            data: Data {},
            metadata: Metadata {},
            path: path.clone(),
        }
    }

    /// The children of the [`Element`].
    ///
    /// This gets the children of the [`Element`], which are the [`Element`]s
    /// that are directly below this [`Element`] in the hierarchy. This is a
    /// simple method that returns the children as a list, and does not provide
    /// any filtering or ordering.
    ///
    /// Notably, there is no real concept of ordering in the storage system, as
    /// the records are not ordered in any way. They are simply stored in the
    /// hierarchy, and so the order of the children is not guaranteed. Any
    /// required ordering must be done as required upon retrieval.
    ///
    #[must_use]
    pub fn children(&self) -> Vec<Self> {
        unimplemented!()
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
    #[must_use]
    pub fn has_children(&self) -> bool {
        unimplemented!()
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
#[derive(Clone, Debug, Eq, Hash, PartialEq, PartialOrd, Ord)]
#[non_exhaustive]
pub struct Metadata;
