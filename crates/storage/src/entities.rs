#![allow(clippy::multiple_inherent_impl, reason = "Used for test-only methods")]

//! Core entity types for the storage system.
//!
//! - [`Element`] - Storage metadata (ID, path, timestamps, hashes)
//! - [`Data`] - Trait for storable user types
//! - [`AtomicUnit`] - Marker for persistable entities
//! - [`Collection`] - Trait for parent-child relationships
//!
//! See [README](../README.md) for design details and examples.

#[cfg(test)]
#[path = "tests/entities.rs"]
mod tests;

use core::fmt::{self, Debug, Display, Formatter};
use std::collections::BTreeMap;
use std::ops::{Deref, DerefMut};

use borsh::{BorshDeserialize, BorshSerialize};

use crate::address::Id;
use crate::env::time_now;

/// Marker trait for atomic, persistable entities.
///
/// Implemented via `#[derive(AtomicUnit)]` macro.
///
/// # Example
/// ```
/// # use borsh::{BorshSerialize, BorshDeserialize};
/// # use calimero_storage::entities::Element;
/// # use calimero_storage_macros::AtomicUnit;
/// #[derive(AtomicUnit, BorshSerialize, BorshDeserialize)]
/// struct Page {
///     title: String,
///     #[storage]
///     storage: Element,
/// }
/// ```
pub trait AtomicUnit: Data {}

/// Trait for parent-child relationships.
///
/// Implemented via `#[derive(Collection)]` macro.
///
/// # Example
/// ```
/// # use borsh::{BorshSerialize, BorshDeserialize};
/// # use calimero_storage_macros::{AtomicUnit, Collection};
/// # use calimero_storage::entities::Element;
/// #[derive(AtomicUnit, BorshSerialize, BorshDeserialize)]
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
/// struct Page {
///     content: String,
///     #[storage]
///     storage: Element,
/// }
/// ```
pub trait Collection {
    /// Child type.
    type Child: Data;
}

/// Base trait for storable user data. Requires an associated [`Element`].
pub trait Data: BorshDeserialize + BorshSerialize {
    /// Collection metadata for children.
    fn collections(&self) -> BTreeMap<String, Vec<ChildInfo>>;

    /// Storage element reference.
    fn element(&self) -> &Element;

    /// Mutable storage element.
    fn element_mut(&mut self) -> &mut Element;

    /// Unique ID (delegates to element).
    #[must_use]
    fn id(&self) -> Id {
        self.element().id()
    }
}

/// Child element metadata stored in parent's index.
#[derive(BorshDeserialize, BorshSerialize, Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct ChildInfo {
    id: Id,
    pub(crate) merkle_hash: [u8; 32],
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
    /// Creates a new ChildInfo.
    #[must_use]
    pub const fn new(id: Id, merkle_hash: [u8; 32], metadata: Metadata) -> Self {
        Self {
            id,
            merkle_hash,
            metadata,
        }
    }

    /// Returns the entity ID.
    #[must_use]
    pub const fn id(&self) -> Id {
        self.id
    }

    /// Returns the Merkle hash.
    #[must_use]
    pub const fn merkle_hash(&self) -> [u8; 32] {
        self.merkle_hash
    }

    /// Returns the creation timestamp.
    #[must_use]
    pub const fn created_at(&self) -> u64 {
        self.metadata.created_at
    }

    /// Returns the last update timestamp.
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

/// Storage metadata for entities (ID, timestamps, dirty flag, Merkle hash).
#[derive(BorshDeserialize, BorshSerialize, Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
#[non_exhaustive]
pub struct Element {
    pub(crate) id: Id,
    #[borsh(skip)]
    pub(crate) is_dirty: bool,
    #[borsh(skip)]
    pub(crate) merkle_hash: [u8; 32],
    #[borsh(skip)]
    pub(crate) metadata: Metadata,
}

impl Element {
    /// Creates a new element (marked dirty, empty hash until saved).
    #[must_use]
    pub fn new(id: Option<Id>) -> Self {
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
        }
    }

    /// Creates the root element.
    #[must_use]
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
        }
    }

    /// Returns the creation timestamp.
    #[must_use]
    pub const fn created_at(&self) -> u64 {
        self.metadata.created_at
    }

    /// Returns the entity ID.
    #[must_use]
    pub const fn id(&self) -> Id {
        self.id
    }

    /// Checks if the entity has unsaved changes.
    #[must_use]
    pub const fn is_dirty(&self) -> bool {
        self.is_dirty
    }

    /// Returns the Merkle hash.
    #[must_use]
    pub const fn merkle_hash(&self) -> [u8; 32] {
        self.merkle_hash
    }

    /// Returns the entity metadata.
    #[must_use]
    pub const fn metadata(&self) -> &Metadata {
        &self.metadata
    }

    /// Marks dirty and updates timestamp.
    pub fn update(&mut self) {
        self.is_dirty = true;
        *self.metadata.updated_at = time_now();
    }

    /// Returns the last update timestamp.
    #[must_use]
    pub fn updated_at(&self) -> u64 {
        *self.metadata.updated_at
    }

    /// Sets the updated timestamp.
    ///
    /// Helper to avoid Law of Demeter violations.
    /// Instead of `element.metadata.updated_at = time`, use `element.set_updated_at(time)`.
    pub fn set_updated_at(&mut self, timestamp: u64) {
        *self.metadata.updated_at = timestamp;
    }

    /// Returns mutable reference to updated_at for direct manipulation.
    ///
    /// Use sparingly - prefer `set_updated_at()` for Law of Demeter compliance.
    #[must_use]
    pub fn updated_at_mut(&mut self) -> &mut u64 {
        &mut *self.metadata.updated_at
    }
}

#[cfg(test)]
impl Element {
    /// Test-only: Sets element ID.
    pub fn set_id(&mut self, id: Id) {
        self.id = id;
    }
}

impl Display for Element {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "Element {}", self.id)
    }
}

/// System metadata (timestamps in u64 nanoseconds).
#[derive(
    BorshDeserialize, BorshSerialize, Copy, Clone, Debug, Default, Eq, Ord, PartialEq, PartialOrd,
)]
#[non_exhaustive]
pub struct Metadata {
    pub(crate) created_at: u64,
    pub(crate) updated_at: UpdatedAt,
}

/// Update timestamp (PartialEq always true for CRDT semantics).
#[derive(BorshDeserialize, BorshSerialize, Copy, Clone, Debug, Default, Eq, Ord, PartialOrd)]
pub struct UpdatedAt(u64);

impl PartialEq for UpdatedAt {
    fn eq(&self, _other: &Self) -> bool {
        true
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
