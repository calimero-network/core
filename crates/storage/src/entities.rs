#![allow(clippy::multiple_inherent_impl, reason = "Used for test-only methods")]

//! Core entity types for the storage system.
//!
//! Provides the building blocks for hierarchical data storage:
//! - [`Element`] - Storage metadata container (ID, path, timestamps, hashes)
//! - [`Data`] - Trait for user data containing an Element
//! - [`AtomicUnit`] - Marker trait for atomic, persistable entities
//! - [`Collection`] - Trait for parent-child relationships
//!
//! # Design Pattern
//!
//! User types implement [`Data`] by including an `Element` field:
//!
//! ```rust
//! # use borsh::{BorshSerialize, BorshDeserialize};
//! # use calimero_storage::entities::Element;
//! # use calimero_storage_macros::AtomicUnit;
//! #[derive(AtomicUnit, BorshSerialize, BorshDeserialize)]
//! #[type_id(1)]
//! struct Person {
//!     name: String,
//!     #[storage]
//!     storage: Element,  // Storage metadata
//! }
//! ```
//!
//! This separation keeps user data clean while enabling:
//! - Automatic ID generation
//! - Merkle hash tracking
//! - Timestamp management
//! - Hierarchy navigation
//!
//! See [Design Decisions](../README.md#design-decisions) in the README for architecture details.

#[cfg(test)]
#[path = "tests/entities.rs"]
mod tests;

use core::fmt::{self, Debug, Display, Formatter};
use std::collections::BTreeMap;
use std::ops::{Deref, DerefMut};

use borsh::{BorshDeserialize, BorshSerialize};

use crate::address::{Id, Path};
use crate::env::time_now;

/// Marker trait for atomic, persistable entities.
///
/// Types deriving this trait are self-contained units that can be stored
/// and synced independently. Automatically implemented by the `#[derive(AtomicUnit)]` macro.
///
/// # Example
/// ```
/// # use borsh::{BorshSerialize, BorshDeserialize};
/// # use calimero_storage::entities::Element;
/// # use calimero_storage_macros::AtomicUnit;
/// #[derive(AtomicUnit, BorshSerialize, BorshDeserialize)]
/// #[type_id(43)]
/// struct Page {
///     title: String,
///     #[storage]  // Required Element field
///     storage: Element,
/// }
/// ```
pub trait AtomicUnit: Data {}

/// Trait for logical groupings of child elements.
///
/// Collections don't have their own storage—they provide typed access to children.
///
/// # Example
/// ```
/// # use borsh::{BorshSerialize, BorshDeserialize};
/// # use calimero_storage_macros::{AtomicUnit, Collection};
/// # use calimero_storage::entities::Element;
/// #[derive(AtomicUnit, BorshSerialize, BorshDeserialize)]
/// #[type_id(42)]
/// struct Book {
///     title: String,
///     pages: Pages,
///     #[storage]
///     storage: Element,
/// }
///
/// #[derive(Collection)]
/// #[children(Page)]
/// struct Pages;
///
/// #[derive(AtomicUnit, BorshSerialize, BorshDeserialize)]
/// #[type_id(43)]
/// struct Page {
///     content: String,
///     #[storage]
///     storage: Element,
/// }
/// ```
pub trait Collection {
    /// The child type stored in this collection.
    type Child: Data;

    /// Collection name, used for indexing.
    fn name(&self) -> &str;
}

/// Base trait for user data that can be stored.
///
/// Requires an associated [`Element`] to hold storage metadata.
/// Provides convenience methods for accessing element properties.
pub trait Data: BorshDeserialize + BorshSerialize {
    /// Returns collection metadata for this entity's children.
    fn collections(&self) -> BTreeMap<String, Vec<ChildInfo>>;

    /// Returns the storage element containing metadata.
    fn element(&self) -> &Element;

    /// Returns mutable access to the storage element.
    fn element_mut(&mut self) -> &mut Element;

    /// Returns the unique identifier (delegates to [`Element::id()`]).
    #[must_use]
    fn id(&self) -> Id {
        self.element().id()
    }

    /// Returns the hierarchical path (delegates to [`Element::path()`]).
    #[must_use]
    fn path(&self) -> Path {
        self.element().path()
    }
}

/// Lightweight metadata for a child element.
///
/// Stored in parent's index to maintain child list and enable Merkle tree comparison
/// without loading full child data.
#[derive(BorshDeserialize, BorshSerialize, Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct ChildInfo {
    /// The unique identifier for the child [`Element`].
    id: Id,

    /// The Merkle hash of the child [`Element`]. This is a cryptographic hash
    /// of the significant data in the "scope" of the child [`Element`], and is
    /// used to determine whether the data has changed and is valid.
    pub(crate) merkle_hash: [u8; 32],

    /// The metadata for the child [`Element`].
    pub(crate) metadata: Metadata,
}

impl Ord for ChildInfo {
    fn cmp(&self, other: &Self) -> core::cmp::Ordering {
        self.created_at()
            .cmp(&other.created_at())
            .then_with(|| self.id.cmp(&other.id))
    }
}

impl PartialOrd for ChildInfo {
    fn partial_cmp(&self, other: &Self) -> Option<core::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl ChildInfo {
    /// Creates new child metadata.
    #[must_use]
    pub const fn new(id: Id, merkle_hash: [u8; 32], metadata: Metadata) -> Self {
        Self {
            id,
            merkle_hash,
            metadata,
        }
    }

    /// Returns the child's unique identifier.
    #[must_use]
    pub const fn id(&self) -> Id {
        self.id
    }

    /// Returns the child's Merkle hash.
    #[must_use]
    pub const fn merkle_hash(&self) -> [u8; 32] {
        self.merkle_hash
    }

    /// Returns creation timestamp.
    #[must_use]
    pub const fn created_at(&self) -> u64 {
        self.metadata.created_at
    }

    /// Returns last update timestamp.
    #[must_use]
    pub fn updated_at(&self) -> u64 {
        *self.metadata.updated_at
    }
}

impl Display for ChildInfo {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "ChildInfo {}: {}",
            self.id,
            hex::encode(self.merkle_hash)
        )
    }
}

/// Storage metadata container for entities.
///
/// Contains ID, path, timestamps, dirty flag, and Merkle hash. Elements can be
/// both nodes (with children) and leaves, determined by inspection.
///
/// # Update Model
///
/// Updates mark element as dirty and update timestamp. Last-write-wins based on
/// `updated_at` resolves conflicts. Children are separate entities—not part of
/// element's state for comparison, but included in Merkle hash calculation.
#[derive(BorshDeserialize, BorshSerialize, Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
#[non_exhaustive]
pub struct Element {
    /// The unique identifier for the [`Element`].
    pub(crate) id: Id,

    /// Whether the [`Element`] is dirty, i.e. has been modified since it was
    /// last saved.
    #[borsh(skip)]
    pub(crate) is_dirty: bool,

    /// The Merkle hash of the [`Element`]. This is a cryptographic hash of the
    /// significant data in the "scope" of the [`Element`], and is used to
    /// determine whether the data has changed and is valid. It is calculated by
    /// hashing the substantive data in the [`Element`], along with the hashes
    /// of the children of the [`Element`], thereby representing the state of
    /// the entire hierarchy below the [`Element`].
    #[borsh(skip)]
    pub(crate) merkle_hash: [u8; 32],

    /// The metadata for the [`Element`]. This represents a range of
    /// system-managed properties that are used to process the [`Element`], but
    /// are not part of the primary data.
    #[borsh(skip)]
    pub(crate) metadata: Metadata,

    /// The path to the [`Element`] in the hierarchy of the storage.
    path: Path,
}

impl Element {
    /// Creates a new element with auto-generated or specified ID.
    ///
    /// Element is marked dirty with empty hash until saved. Use for new local
    /// elements; remote elements should be deserialized.
    ///
    /// # Panics
    /// Panics if system time is before Unix epoch.
    #[must_use]
    pub fn new(path: &Path, id: Option<Id>) -> Self {
        let timestamp = time_now();
        let element_id = id.unwrap_or_else(Id::random);
        Self {
            id: element_id,
            is_dirty: true,
            metadata: Metadata {
                created_at: timestamp,
                updated_at: timestamp.into(),
            },
            merkle_hash: [0; 32],
            path: path.clone(),
        }
    }

    /// Creates the root element with special ID.
    #[must_use]
    #[expect(clippy::missing_panics_doc, reason = "This is expected to be valid")]
    pub fn root() -> Self {
        let timestamp = time_now();
        Self {
            id: Id::root(),
            is_dirty: true,
            metadata: Metadata {
                created_at: timestamp,
                updated_at: timestamp.into(),
            },
            merkle_hash: [0; 32],
            #[expect(clippy::unwrap_used, reason = "This is expected to be valid")]
            path: Path::new("::root").unwrap(),
        }
    }

    /// Returns creation timestamp.
    #[must_use]
    pub const fn created_at(&self) -> u64 {
        self.metadata.created_at
    }

    /// Returns the unique identifier (stable across element moves).
    #[must_use]
    pub const fn id(&self) -> Id {
        self.id
    }

    /// Returns whether element has unsaved changes.
    #[must_use]
    pub const fn is_dirty(&self) -> bool {
        self.is_dirty
    }

    /// Returns current Merkle hash (valid only if not dirty).
    #[must_use]
    pub const fn merkle_hash(&self) -> [u8; 32] {
        self.merkle_hash
    }

    /// Returns system-managed metadata (timestamps).
    #[must_use]
    pub const fn metadata(&self) -> &Metadata {
        &self.metadata
    }

    /// Returns hierarchical path (may change if element moves).
    #[must_use]
    pub fn path(&self) -> Path {
        self.path.clone()
    }

    /// Marks element as dirty and updates timestamp.
    ///
    /// Call after modifying data. Hash remains unchanged until save.
    ///
    /// # Panics
    /// Panics if system time is before Unix epoch.
    pub fn update(&mut self) {
        self.is_dirty = true;
        *self.metadata.updated_at = time_now();
    }

    /// Returns last update timestamp.
    #[must_use]
    pub fn updated_at(&self) -> u64 {
        *self.metadata.updated_at
    }
}

#[cfg(test)]
impl Element {
    /// Sets the ID of the [`Element`]. This is **ONLY** for use in tests.
    pub fn set_id(&mut self, id: Id) {
        self.id = id;
    }
}

impl Display for Element {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "Element {}: {}", self.id, self.path)
    }
}

/// System-managed metadata for elements.
///
/// Timestamps use `u64` nanoseconds since Unix epoch (585 years range).
/// More efficient than Chrono and supports Borsh serialization.
#[derive(
    BorshDeserialize, BorshSerialize, Copy, Clone, Debug, Default, Eq, Ord, PartialEq, PartialOrd,
)]
#[non_exhaustive]
pub struct Metadata {
    /// When the [`Element`] was first created. Note that this is a global
    /// creation time, and does not reflect the time that the [`Element`] was
    /// added to the local storage.
    pub(crate) created_at: u64,

    /// When the [`Element`] was last updated. This is the time that the
    /// [`Element`] was last modified in any way, and is used to determine the
    /// freshness of the data. It is critical for the "last write wins" strategy
    /// that is used to resolve conflicts.
    pub(crate) updated_at: UpdatedAt,
}

/// Wrapper for update timestamp with custom PartialEq (always true for CRDT semantics).
#[derive(BorshDeserialize, BorshSerialize, Copy, Clone, Debug, Default, Eq, Ord, PartialOrd)]
pub struct UpdatedAt(u64);

impl PartialEq for UpdatedAt {
    fn eq(&self, _other: &Self) -> bool {
        true // Always equal for structural comparison
    }
}

impl Deref for UpdatedAt {
    type Target = u64;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for UpdatedAt {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl From<u64> for UpdatedAt {
    fn from(value: u64) -> Self {
        Self(value)
    }
}
