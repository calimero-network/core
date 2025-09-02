#![allow(clippy::multiple_inherent_impl, reason = "Used for test-only methods")]

//! Entities for the storage system.
//!
//! This module contains the entities that are used to represent the elements in
//! the storage system. These are the building blocks that are used to construct
//! the hierarchy of the storage, and to apply updates to the elements.
//!
//! # Design: Elements, data, and atomic units
//!
//! There are a number of requirements that need to be catered for by the
//! solution, and it is worth exploring some of the possible structures and
//! approaches that have been considered.
//!
//! ## Considerations
//!
//! Let's examine first principles:
//!
//!   1. `Element`s are saved, and `Element`s are structs, with system metadata.
//!   2. `Element`s also contain user data, represented by `Data`.
//!   3. User types need to have an easy interface, using an `AtomicUnit`
//!      annotation.
//!
//! It is possible that `Element` should be a trait, and that the `AtomicUnit`
//! trait should be a superset of `Element`, and that the `AtomicUnit` proc
//! macro would then apply both `Element` and `AtomicUnit`. However, this would
//! still need a `Data` struct to be constructed, owned by the `Element`.
//!
//! Therefore, if we say that `Element` remains a struct, for simplicity, then
//! what exactly is being annotated? If the user creates a `Person`, is that
//! `Person` an `Element` or an `AtomicUnit`?
//!
//! It seems that the `Person` must be an `AtomicUnit`, which means it cannot
//! *be* `Data` with this structure, as it would need to *contain* `Data`. Now,
//! it initially seemed that `AtomicUnit` should extend `Data`. But if
//! `AtomicUnit` is the `Data`, then the `Person` annotation would be misleading
//! if it was applying to the internal `Data` and not to the `Person`. This
//! would not be idiomatic nor a good pattern to follow.
//!
//! So... if the `Person` is in fact the `AtomicUnit`, and it has data fields,
//! where do we get `Data` from now? We can't really have `Data` being an
//! `Element`, so `Data` has to be owned by `Element`, and where does the
//! `Element` come from?
//!
//! The relationship between these entities is critical to get right. As much as
//! possible of the internals should be abstracted away from the user, with the
//! presented interface being as simple as possible.
//!
//! ## Option One
//!
//!   - `Person` is an `AtomicUnit`.
//!   - `Person` defines fields — some marked as private.
//!   - To satisfy `AtomicUnit`, `Person` has a `storage` field added.
//!   - `Person.storage` is an `Element`, and contains a `PersonData` struct.
//!   - `PersonData` is constructed by the macro, and contains a copy of the
//!     fields from `Person` that are not private. These fields could
//!     potentially be borrowed, referencing the fields on the `Element`.
//!   - It's then unclear if `Person` (i.e. `AtomicUnit`) needs a `save()`
//!     method, or whether it should be passed to `Interface`.
//!
//! This suffers from a number of issues, including:
//!
//!   - The `Person` is now a leaky abstraction, as it exposes the internals of
//!     the `Element` and `PersonData`.
//!   - The `Person` is now a pain to work with, as it has to be constructed in
//!     a specific way, including the construction of `PersonData`.
//!   - The very existence of `PersonData` is icky.
//!
//! ## Option Two
//!
//!   - `Person` is an `AtomicUnit`.
//!   - `Person` defines fields — some marked as private.
//!   - `Element` becomes a trait instead of a struct.
//!   - `AtomicUnit` demands `Element`.
//!   - `Person` is now therefore an `Element`.
//!   - `Person.data` contains a `PersonData` struct.
//!   - `PersonData` is constructed by the macro, and contains a copy of the
//!     fields from `Person` that are not private. Again, these fields could
//!     potentially be borrowed, referencing the fields on the `Element`.
//!   - `Person` does not appear to need a `save()` now, as `Interface.save()`
//!     accepts `Element`, and therefore `Person`.
//!
//! This is also problematic:
//!
//!   - The `Element` has a lot on it, which clutters up the `Person`.
//!   - It exposes the internals, which is undesirable.
//!
//! ## Option Three
//!
//!   - `Person` is an `AtomicUnit`.
//!   - `Person` defines fields — some marked as private.
//!   - `Element` remains a struct.
//!   - `Interface` accepts `Data` not `Element`.
//!   - `AtomicUnit` extends `Data`.
//!   - `Person` is therefore `Data`.
//!   - `Person` has a `storage` field added, to hold an `Element`.
//!   - A circular reference is carefully created between `Person` and
//!     `Element`, with `Element` holding a weak reference to `Person`, and
//!     `Person` having a strong reference to (or ownership of) `Element`.
//!   - When accepting `Data`, i.e. a `Person` here, the `Interface` can call
//!     through `Person.storage` to get to the `Element`, and the `Element` can
//!     access the `Data` through the [`Weak`](std::sync::Weak).
//!   - The macro does not need to create another struct, and the saving logic
//!     can skip the fields marked as private.
//!   - `Person` does not appear to need a `save()`, as `Interface.save()` does
//!     not need to be changed.
//!
//! The disadvantages of this approach are:
//!
//!   - The circular reference is a bit of a pain, and could be a source of
//!     bugs.
//!   - In order to achieve the circular reference, the `Element` would need to
//!     wrap the `Data` in a [`Weak`](std::sync::Weak), which means that all use
//!     of the `Data` would need to be through the reference-counted pointer,
//!     which implies wrapping `Data` in an [`Arc`](std::sync::Arc) upon
//!     construction.
//!   - In order to gain mutability, some kind of mutex or read-write lock would
//!     be required. Imposing a predetermined mutex type would be undesirable,
//!     as the user may have their own requirements. It's also an annoying
//!     imposition.
//!   - Due to the `Data` (i.e. `Person`) being wrapped in an [`Arc`](std::sync::Arc),
//!     [`Default`] cannot be implemented.
//!
//! ## Option Four
//!
//!   - `Person` is an `AtomicUnit`.
//!   - `Person` defines fields — some marked as private.
//!   - `Element` remains a struct.
//!   - `Interface` accepts `Data` not `Element`.
//!   - `AtomicUnit` extends `Data`.
//!   - `Person` is therefore `Data`.
//!   - `Person` has a `storage` field added, to hold (and own) an `Element`.
//!   - When accepting `Data`, i.e. a `Person` here, the `Interface` can call
//!     through `Person.storage` to get to the `Element`.
//!   - The macro does not need to create another struct, and the saving logic
//!     can skip the fields marked as private.
//!   - `Person` does not appear to need a `save()`, as `Interface.save()` does
//!     not need to be changed.
//!
//! This is essentially the same as Option Three, but without the circular
//! reference. The disadvantages are:
//!
//!   - There is no way for the `Element` to access the `Data` directly. This
//!     forces a different approach to those operations that require `Data`.
//!   - The `Element`'s operations can accept a `Data` instead of looking it up
//!     directly, but this loses structural assurance of the relationship.
//!
//! Passing the `Data` to the `Element`'s operations does work around the first
//! issue, albeit in a slightly unwieldy way. The second issue can be mitigated
//! by performing a check that the `Element` owned by the passed-in `Data` is
//! the same instance as the `Element` that the operation is being called on.
//! Although this is a runtime logic check, it is a simple one, and can be
//! entirely contained within the internal logic, which remains unexposed.
//!
//! ## Option Five
//!
//! For the sake of completeness, it's worth noting that technically another
//! option would be to use generics, such as `Element<D: Data>`, but this leads
//! to unnecessary complexity at this stage (including the potential to have to
//! use phantom data), with no tangible benefits. Therefore this has not been
//! discussed in detail here, although it has been carefully considered.
//!
//! ## Conclusion
//!
//! Option Four is the approach chosen, as it balances all of the following
//! aspirations:
//!
//!   - Simple to use.
//!   - Allows full ownership and mutability of the user's types.
//!   - Abstracts all storage functionality to the storage internals.
//!   - Achieves reliability.
//!   - Allows implementation of [`Default`] and similar without unexpected
//!     constraints.
//!   - Does not clutter the user's types.
//!   - Does not expose the internals of the storage system.
//!   - Does not require a circular reference.
//!   - Does not impose a mutex type.
//!   - Does not force a predetermined mutex type.
//!   - Does not require a separate struct to be constructed.
//!   - Does not require references to, or clones of, saved data.
//!
//! Not having a reference back to the `Data` from the `Element` is a slight
//! trade-off, but it is a small one, sacrificing a little structural assurance
//! and direct access for a simpler design with fewer impositions. The internal
//! validation check is simple, and it is best to promote the elegance of the
//! user interface over that of the internals — if there are a couple of hoops
//! to jump through then it is best for these to be in the library code.
//!
//! # Design: Collections
//!
//! There are three main ways to implement the collection functionality, i.e.
//! where a parent [`Element`] has children. These are:
//!
//!   1. **Struct-based**: Annotate the struct as being a `Collection`, meaning
//!      it can then have children. In this situation the child type is supplied
//!      as an associated type. This is the most straightforward approach, but
//!      it does not allow for the parent to have multiple types of children.
//!
//!   2. **Enum-based**: E.g. `enum ChildType { Page(Page), Author(Author) }`.
//!      This is more flexible, but it requires a match to determine the child
//!      type, and although the type formality is good in some ways, it adds
//!      a level of complexity and maintenance that is not desirable.
//!
//!   3. **Field-based**: E.g. `entity.pages` and `entity.authors` annotated as
//!      `Collection`, with some way to look up which fields are needed. This is
//!      the most flexible, and the easiest developer interface to use.
//!
//! The approach taken is Option 3, for the reasons given above.
//!

#[cfg(test)]
#[path = "tests/entities.rs"]
mod tests;

use core::fmt::{self, Debug, Display, Formatter};
use std::collections::BTreeMap;
use std::ops::{Deref, DerefMut};

use borsh::{BorshDeserialize, BorshSerialize};

use crate::address::{Id, Path};
use crate::env::time_now;

/// Represents an atomic unit in the storage system.
///
/// An atomic unit is a self-contained piece of data that can be stored and
/// retrieved as a single entity. It extends the [`Data`] trait with additional
/// methods specific to how it's stored and identified in the system.
///
/// This is a marker trait, and does not have any special functionality.
///
/// # Examples
///
/// ```
/// use borsh::{BorshSerialize, BorshDeserialize};
/// use calimero_storage::entities::Element;
/// use calimero_storage_macros::AtomicUnit;
///
/// #[derive(AtomicUnit, Clone, Debug, Eq, PartialEq, PartialOrd, BorshSerialize, BorshDeserialize)]
/// #[type_id(43)]
/// struct Page {
///     title: String,
///     #[private]
///     secret: String,
///     #[storage]
///     storage: Element,
/// }
/// ```
///
pub trait AtomicUnit: Data {}

/// A collection of child elements in the storage system.
///
/// [`Collection`]s are logical groupings of child [`Element`]s. They do not
/// have their own storage or [`Element`], but instead provide a way to
/// logically group and access child elements of a specific type.
///
/// # Examples
///
/// ```
/// use borsh::{BorshSerialize, BorshDeserialize};
/// use calimero_storage_macros::{AtomicUnit, Collection};
/// use calimero_storage::entities::{ChildInfo, Data, Element};
///
/// #[derive(AtomicUnit, Clone, Debug, Eq, PartialEq, PartialOrd, BorshSerialize, BorshDeserialize)]
/// #[type_id(42)]
/// struct Book {
///     title: String,
///     pages: Pages,
///     #[storage]
///     storage: Element,
/// }
///
/// #[derive(Collection, Clone, Debug, Eq, PartialEq, PartialOrd)]
/// #[children(Page)]
/// struct Pages;
///
/// #[derive(AtomicUnit, Clone, Debug, Eq, PartialEq, PartialOrd, BorshSerialize, BorshDeserialize)]
/// #[type_id(43)]
/// struct Page {
///     content: String,
///     #[storage]
///     storage: Element,
/// }
/// ```
///
pub trait Collection {
    /// The associated type of any children that the [`Collection`] may have.
    type Child: Data;

    /// The name of this [`Collection`].
    ///
    /// This is used to identify the collection when updating the index.
    ///
    fn name(&self) -> &str;
}

/// The primary data for the [`Element`].
///
/// This is the primary data for the [`Element`], that is, the data that the
/// consumer application has stored in the [`Element`]. This is the data that
/// the [`Element`] is primarily concerned with, and for the management of which
/// the [`Element`] exists.
///
/// This trait represents a common denominator for the various specific traits
/// such as [`AtomicUnit`] that are used to denote exactly what the [`Element`]
/// is representing.
///
/// # Pass-through methods
///
/// The [`Data`] trait contains a number of pass-through methods that are
/// implemented for convenience, addressing the inner [`Element`]. Potentially
/// most of the [`Element`] methods could be added, or even [`Deref`](std::ops::Deref)
/// implemented, but for now only the most useful and least likely to be
/// contentious methods are included, to keep the interface simple and focused.
///
pub trait Data: BorshDeserialize + BorshSerialize {
    /// Information about the [`Collection`]s present in the [`Data`].
    ///
    /// This method allows details about the subtree structure and children to
    /// be obtained. It does not return the actual [`Collection`] types, but
    /// provides their names and child information.
    ///
    fn collections(&self) -> BTreeMap<String, Vec<ChildInfo>>;

    /// The associated [`Element`].
    ///
    /// The [`Element`] contains additional metadata and storage-related
    /// identification and other information for the primary data, keeping it
    /// abstracted and separate from the primary data itself.
    ///
    /// # See also
    ///
    /// * [`element_mut()`](Data::element_mut())
    ///
    fn element(&self) -> &Element;

    /// The associated [`Element`], with mutability.
    ///
    /// This function is used to obtain a mutable reference to the [`Element`]
    /// that contains the primary data.
    ///
    /// # See also
    ///
    /// * [`element()`](Data::element())
    ///
    fn element_mut(&mut self) -> &mut Element;

    /// The unique identifier for the [`Element`].
    ///
    /// This is a convenience function that passes through to [`Element::id()`].
    /// See that method for more information.
    ///
    /// # See also
    ///
    /// * [`Element::id()`]
    ///
    #[must_use]
    fn id(&self) -> Id {
        self.element().id()
    }

    /// The path to the [`Element`] in the hierarchy.
    ///
    /// This is a convenience function that passes through to
    /// [`Element::path()`]. See that method for more information.
    ///
    /// # See also
    ///
    /// * [`Element::path()`]
    ///
    #[must_use]
    fn path(&self) -> Path {
        self.element().path()
    }
}

/// Summary information for the child of an [`Element`] in the storage.
///
/// This struct contains minimal information about a child of an [`Element`], to
/// be stored with the associated [`Data`]. The primary purpose is to maintain
/// an authoritative list of the children of the [`Element`], and the secondary
/// purpose is to make information such as the Merkle hash trivially available
/// and prevent the need for repeated lookups.
///
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
    /// Creates a new [`ChildInfo`].
    #[must_use]
    pub const fn new(id: Id, merkle_hash: [u8; 32], metadata: Metadata) -> Self {
        Self {
            id,
            merkle_hash,
            metadata,
        }
    }

    /// The unique identifier for the child [`Element`].
    ///
    /// This is the unique identifier for the child [`Element`], which can
    /// always be used to locate the [`Element`] in the storage system. It is
    /// generated when the [`Element`] is first created, and never changes. It
    /// is reflected onto all other systems and so is universally consistent.
    ///
    #[must_use]
    pub const fn id(&self) -> Id {
        self.id
    }

    /// Current Merkle hash of the [`Element`].
    #[must_use]
    pub const fn merkle_hash(&self) -> [u8; 32] {
        self.merkle_hash
    }

    /// The timestamp when the child was created.
    #[must_use]
    pub const fn created_at(&self) -> u64 {
        self.metadata.created_at
    }

    /// The timestamp when the child was last updated.
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
/// # Internal structure
///
/// The primary data for the [`Element`], that is, the data that the consumer
/// application has stored in the [`Element`], cannot be directly accessed by
/// the [`Element`]. This is because that data is the focus of the user
/// operations, and it is desirable to keep the [`Element`] as entirely
/// self-contained. Therefore [`Data`] contains an [`Element`], purely to
/// promote this focus and separation.
///
/// For this reason, any [`Element`] methods that require access to the primary
/// data require the [`Data`] to be passed in, and need to check and ensure that
/// the [`Element`] owned by the [`Data`] is the same instance as the
/// [`Element`] that the operation is being called on.
///
/// # Storage structure
///
/// TODO: Update when the `child_info` field is replaced with an index.
///
/// At present the [`Data`] trait has a `child_info` field, which memoises the
/// IDs and Merkle hashes of the children. This is because it is better than
/// traversing the whole database to find the children (!).
///
/// Having a `child_info` field is however non-optimal — this should come from
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
    /// # Merkle hash
    ///
    /// The Merkle hash will be empty for a brand-new [`Element`], as it has not
    /// been saved. When saved to the database, the hash will be calculated and
    /// stored, and set against the object. The way to tell if the hash is
    /// up-to-date is simply to check the [`is_dirty()`](Element::is_dirty())
    /// flag.
    ///
    /// # Parameters
    ///
    /// * `path` - The path to the [`Element`] in the hierarchy of the storage.
    /// * `id` - The id of the [`Element`] in the hierarchy of the storage.
    ///
    /// # Panics
    ///
    /// This method can technically panic if the system time goes backwards, to
    /// before the Unix epoch, which should never ever happen!
    ///
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

    /// Constructor for the root [`Element`].
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

    /// The timestamp when the [`Element`] was first created.
    #[must_use]
    pub const fn created_at(&self) -> u64 {
        self.metadata.created_at
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

    /// Current Merkle hash of the [`Element`].
    #[must_use]
    pub const fn merkle_hash(&self) -> [u8; 32] {
        self.merkle_hash
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

    /// Updates the metadata for the [`Element`].
    ///
    /// This updates the metadata for the [`Element`], and marks the [`Element`]
    /// as dirty. This is used to indicate that the [`Element`] has been
    /// modified since it was last saved, and that it needs to be saved again in
    /// order to persist the changes.
    ///
    /// It updates the [`updated_at()`](Element::updated_at()) timestamp to
    /// reflect the time that the [`Element`] was last updated (this is part of
    /// the metadata).
    ///
    /// **IMPORTANT**: It does not update the actual data itself, as it has no
    /// way of accessing this. Therefore, this method should be called after
    /// updating the data.
    ///
    /// TODO: Add data parameter back in, and also accept old data as well as
    ///       new data, to compare the values and determine if there has been an
    ///       update.
    ///
    /// # Merkle hash
    ///
    /// The Merkle hash will NOT be updated when the data is updated. It will
    /// also not be cleared. Rather, it will continue to represent the state of
    /// the *stored* data, until the data changes are saved, at which point the
    /// hash will be recalculated and updated. The way to tell if the hash is
    /// up-to-date is simply to check the [`is_dirty()`](Element::is_dirty())
    /// flag.
    ///
    /// # Panics
    ///
    /// This method can technically panic if the system time goes backwards, to
    /// before the Unix epoch, which should never ever happen!
    ///
    pub fn update(&mut self) {
        self.is_dirty = true;
        *self.metadata.updated_at = time_now();
    }

    /// The timestamp when the [`Element`] was last updated.
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

/// The timestamp when the [`Element`] was last updated.
#[derive(BorshDeserialize, BorshSerialize, Copy, Clone, Debug, Default, Eq, Ord, PartialOrd)]
pub struct UpdatedAt(u64);

impl PartialEq for UpdatedAt {
    fn eq(&self, _other: &Self) -> bool {
        // we don't care
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
