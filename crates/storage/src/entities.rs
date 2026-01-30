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
                resolution: ResolutionStrategy::default(),
            },
            merkle_hash: [0; 32],
        }
    }

    /// Creates a new element with a specific resolution strategy.
    #[must_use]
    pub fn with_resolution(id: Option<Id>, resolution: ResolutionStrategy) -> Self {
        let timestamp = time_now();
        let element_id = id.unwrap_or_else(Id::random);
        Self {
            id: element_id,
            is_dirty: true,
            metadata: Metadata {
                created_at: timestamp,
                updated_at: timestamp.into(),
                storage_type: StorageType::Public,
                resolution,
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
                resolution: ResolutionStrategy::default(),
            },
            merkle_hash: [0; 32],
        }
    }

    /// Creates the root element with a specific resolution strategy.
    #[must_use]
    pub fn root_with_resolution(resolution: ResolutionStrategy) -> Self {
        let timestamp = time_now();
        Self {
            id: Id::root(),
            is_dirty: true,
            metadata: Metadata {
                created_at: timestamp,
                updated_at: timestamp.into(),
                storage_type: StorageType::Public,
                resolution,
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

    /// Returns the conflict resolution strategy.
    #[must_use]
    pub const fn resolution(&self) -> ResolutionStrategy {
        self.metadata.resolution
    }

    /// Sets the conflict resolution strategy.
    ///
    /// Call this before saving to change how conflicts are resolved during sync.
    pub fn set_resolution(&mut self, resolution: ResolutionStrategy) {
        self.metadata.resolution = resolution;
        self.is_dirty = true;
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

/// Strategy for resolving conflicts when two nodes have different values.
///
/// Used during tree synchronization to determine which value wins when
/// the same entity has been modified on multiple nodes.
#[derive(BorshDeserialize, BorshSerialize, Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum ResolutionStrategy {
    /// Last-Write-Wins: The value with the newer timestamp wins.
    /// This is the default strategy for most use cases.
    LastWriteWins,

    /// First-Write-Wins: The value with the older timestamp wins.
    /// Useful for immutable-after-creation data.
    FirstWriteWins,

    /// Maximum value wins (compared as bytes).
    /// Useful for version numbers, counters, etc.
    MaxValue,

    /// Minimum value wins (compared as bytes).
    /// Useful for finding earliest timestamps, lowest bids, etc.
    MinValue,

    /// Both values are kept - requires manual resolution.
    /// The sync will mark the entity as conflicted for app-level handling.
    Manual,
}

impl Default for ResolutionStrategy {
    fn default() -> Self {
        Self::LastWriteWins
    }
}

impl ResolutionStrategy {
    /// Resolve a conflict between local and remote data.
    ///
    /// Returns `true` if remote should win (apply remote to local),
    /// Returns `false` if local should win (apply local to remote).
    /// Returns `None` for `Manual` strategy (both sides need notification).
    #[must_use]
    pub fn resolve(
        &self,
        local_data: &[u8],
        remote_data: &[u8],
        local_ts: u64,
        remote_ts: u64,
    ) -> Option<bool> {
        match self {
            Self::LastWriteWins => {
                // Remote wins if its timestamp >= local
                Some(remote_ts >= local_ts)
            }
            Self::FirstWriteWins => {
                // Remote wins if its timestamp < local (it's older)
                Some(remote_ts < local_ts)
            }
            Self::MaxValue => {
                // Compare bytes lexicographically, higher value wins
                Some(remote_data >= local_data)
            }
            Self::MinValue => {
                // Compare bytes lexicographically, lower value wins
                Some(remote_data <= local_data)
            }
            Self::Manual => {
                // No automatic resolution
                None
            }
        }
    }
}

/// System metadata (timestamps in u64 nanoseconds).
#[derive(
    BorshDeserialize, BorshSerialize, Clone, Debug, Default, Eq, Ord, PartialEq, PartialOrd,
)]
#[non_exhaustive]
pub struct Metadata {
    /// Timestamp of creation time in u64 nanoseconds.
    pub created_at: u64,
    /// Timestamp of update time in u64 nanoseconds.
    pub updated_at: UpdatedAt,

    /// Storage type represents the Public/Frozen/User storage type. Each of the types has
    /// different characteristics of handling in the node.
    /// See `StorageType`.
    pub storage_type: StorageType,

    /// Strategy for resolving conflicts when syncing with other nodes.
    /// Defaults to `LastWriteWins` for backward compatibility.
    /// See `ResolutionStrategy`.
    pub resolution: ResolutionStrategy,
}

impl Metadata {
    /// Creates new metadata with the provided timestamps.
    #[must_use]
    pub fn new(created_at: u64, updated_at: u64) -> Self {
        Self {
            created_at,
            updated_at: updated_at.into(),
            storage_type: StorageType::default(),
            resolution: ResolutionStrategy::default(),
        }
    }

    /// Creates new metadata with a specific resolution strategy.
    #[must_use]
    pub fn with_resolution(
        created_at: u64,
        updated_at: u64,
        resolution: ResolutionStrategy,
    ) -> Self {
        Self {
            created_at,
            updated_at: updated_at.into(),
            storage_type: StorageType::default(),
            resolution,
        }
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
