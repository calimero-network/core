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

use calimero_primitives::identity::PublicKey;
use core::fmt::{self, Debug, Display, Formatter};
use std::collections::BTreeMap;
use std::io::{ErrorKind, Read};
use std::ops::{Deref, DerefMut};

use borsh::{BorshDeserialize, BorshSerialize};

use crate::address::Id;
use crate::env::time_now;

/// Identifies the specific CRDT type for entity metadata.
///
/// Used to enable proper CRDT merge dispatch during state synchronization.
/// Without this, state sync falls back to Last-Write-Wins (LWW), which causes
/// data loss for concurrent updates on Counters, Maps, Sets, etc.
#[derive(BorshDeserialize, BorshSerialize, Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum CrdtType {
    /// Last-Write-Wins Register
    LwwRegister,
    /// Grow-only Counter
    Counter,
    /// Replicated Growable Array (text CRDT)
    Rga,
    /// Unordered Map (add-wins set semantics for keys)
    UnorderedMap,
    /// Unordered Set (add-wins semantics)
    UnorderedSet,
    /// Vector (ordered list with operational transformation)
    Vector,
    /// Custom user-defined CRDT (requires WASM callback for merge)
    Custom {
        /// Type name identifier for the custom CRDT
        type_name: String,
    },
}

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
#[derive(BorshDeserialize, BorshSerialize, Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct ChildInfo {
    id: Id,
    pub(crate) merkle_hash: [u8; 32],
    /// Metadata of the child.
    pub metadata: Metadata,
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
                storage_type: StorageType::Public,
                crdt_type: None,
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
                storage_type: StorageType::Public,
                crdt_type: None,
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

    /// Helper to set the storage domain to `User`.
    pub fn set_user_domain(&mut self, owner: PublicKey) {
        self.metadata.storage_type = StorageType::User {
            owner,
            signature_data: None, // Will be signed later
        };
        self.update(); // Mark as dirty
    }

    /// Helper to set the storage domain to `Frozen.`
    pub fn set_frozen_domain(&mut self) {
        self.metadata.storage_type = StorageType::Frozen;
        self.update(); // Mark as dirty
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

/// Data for a user-owned, signed action.
#[derive(BorshDeserialize, BorshSerialize, Copy, Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct SignatureData {
    /// Ed25519 signature.
    pub signature: [u8; 64],
    /// Nonce (counter/timestamp) to avoid replaying attacks.
    pub nonce: u64,
}

/// Defines the type of storage and its associated authorization rules.
/// Enum to define the storage domain and its associated data.
#[derive(BorshDeserialize, BorshSerialize, Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum StorageType {
    /// Public data, accessible to all members of context.
    Public,
    /// Verifiable, user-signed, synchronized storage.
    User {
        /// The owner of the data where this storage type is applied.
        owner: PublicKey,
        /// A signature and nonce for the data. The signature should be done by the `owner`.
        signature_data: Option<SignatureData>,
    },
    /// Data that can be set only once, can'be modified or deleted.
    Frozen,
}

// Default to `Public` for backward compatibility
impl Default for StorageType {
    fn default() -> Self {
        Self::Public
    }
}

/// System metadata (timestamps in u64 nanoseconds).
#[derive(BorshSerialize, Clone, Debug, Default, Eq, Ord, PartialEq, PartialOrd)]
pub struct Metadata {
    /// Timestamp of creation time in u64 nanoseconds.
    pub created_at: u64,
    /// Timestamp of update time in u64 nanoseconds.
    pub updated_at: UpdatedAt,

    /// Storage type represents the Public/Frozen/User storage type. Each of the types has
    /// different characteristics of handling in the node.
    /// See `StorageType`.
    pub storage_type: StorageType,

    /// CRDT type identifier for proper merge dispatch during state synchronization.
    ///
    /// When `None`, state sync falls back to Last-Write-Wins (LWW).
    /// When `Some(crdt_type)`, enables proper CRDT merge for Counters, Maps, Sets, etc.
    ///
    /// Backward compatible: custom deserializer handles missing field from old data.
    pub crdt_type: Option<CrdtType>,
}

// Custom deserialization to handle backward compatibility with existing stored data.
//
// Before this field was added, Metadata was serialized with only 3 fields:
// [created_at][updated_at][storage_type]
//
// After adding crdt_type, new data serializes as:
// [created_at][updated_at][storage_type][crdt_type]
//
// When deserializing old data, Borsh will encounter UnexpectedEof when trying to
// read crdt_type (because it doesn't exist in old data). We catch this error and
// default to None, maintaining backward compatibility.
impl BorshDeserialize for Metadata {
    fn deserialize_reader<R: Read>(reader: &mut R) -> borsh::io::Result<Self> {
        // Always deserialize the original 3 fields
        let created_at = u64::deserialize_reader(reader)?;
        let updated_at = UpdatedAt::deserialize_reader(reader)?;
        let storage_type = StorageType::deserialize_reader(reader)?;

        // Try to deserialize crdt_type (new field)
        // If the data is old (doesn't have this field), UnexpectedEof will be raised
        let crdt_type = match Option::<CrdtType>::deserialize_reader(reader) {
            Ok(crdt_type) => crdt_type,
            Err(e) => {
                // Only treat "no more data" errors as "field not present" (old data format)
                // Propagate all other errors (corruption, I/O errors, etc.)
                match e.kind() {
                    ErrorKind::UnexpectedEof => None, // Old data - no crdt_type field
                    ErrorKind::InvalidData if e.to_string().contains("Unexpected length") => {
                        None // Old data detected via insufficient bytes
                    }
                    _ => return Err(e), // Real error - propagate
                }
            }
        };

        Ok(Metadata {
            created_at,
            updated_at,
            storage_type,
            crdt_type,
        })
    }
}

impl Metadata {
    /// Creates new metadata with the provided timestamps.
    #[must_use]
    pub fn new(created_at: u64, updated_at: u64) -> Self {
        Self {
            created_at,
            updated_at: updated_at.into(),
            storage_type: StorageType::default(),
            crdt_type: None,
        }
    }

    /// Creates new metadata with CRDT type.
    ///
    /// # Example
    /// ```
    /// # use calimero_storage::entities::{Metadata, CrdtType};
    /// let metadata = Metadata::with_crdt_type(1000, 2000, CrdtType::Counter);
    /// assert_eq!(metadata.crdt_type, Some(CrdtType::Counter));
    /// ```
    #[must_use]
    pub fn with_crdt_type(created_at: u64, updated_at: u64, crdt_type: CrdtType) -> Self {
        Self {
            created_at,
            updated_at: updated_at.into(),
            storage_type: StorageType::default(),
            crdt_type: Some(crdt_type),
        }
    }

    /// Checks if this metadata has a built-in CRDT type (not Custom).
    ///
    /// Built-in CRDTs (Counter, LwwRegister, etc.) are merged in the storage layer.
    /// Custom CRDTs require WASM callback for merge.
    ///
    /// # Example
    /// ```
    /// # use calimero_storage::entities::{Metadata, CrdtType};
    /// let builtin = Metadata::with_crdt_type(1000, 2000, CrdtType::Counter);
    /// assert!(builtin.is_builtin_crdt());
    ///
    /// let custom = Metadata::with_crdt_type(1000, 2000, CrdtType::Custom { type_name: "MyCRDT".to_string() });
    /// assert!(!custom.is_builtin_crdt());
    ///
    /// let none = Metadata::new(1000, 2000);
    /// assert!(!none.is_builtin_crdt());
    /// ```
    #[must_use]
    pub fn is_builtin_crdt(&self) -> bool {
        matches!(
            self.crdt_type,
            Some(CrdtType::Counter)
                | Some(CrdtType::LwwRegister)
                | Some(CrdtType::Rga)
                | Some(CrdtType::UnorderedMap)
                | Some(CrdtType::UnorderedSet)
                | Some(CrdtType::Vector)
        )
    }

    /// Updates the `updated_at` timestamp.
    pub fn set_updated_at(&mut self, timestamp: u64) {
        self.updated_at = timestamp.into();
    }

    /// Returns the creation timestamp.
    #[must_use]
    pub const fn created_at(&self) -> u64 {
        self.created_at
    }

    /// Returns the last update timestamp.
    #[must_use]
    pub fn updated_at(&self) -> u64 {
        *self.updated_at
    }
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
