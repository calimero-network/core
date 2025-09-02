//! Addressing of elements in the storage system.
//!
//! This module provides the types and functionality needed for addressing
//! [`Element`](crate::entities::Element)s in the storage system. This includes
//! identification by [`Id`] and [`Path`].
//!

#[cfg(test)]
#[path = "tests/address.rs"]
mod tests;

use core::fmt::{self, Debug, Display, Formatter};
use std::io::{Error as IoError, ErrorKind as IoErrorKind, Read, Write};

use borsh::{BorshDeserialize, BorshSerialize};
use fixedstr::Flexstr;
use thiserror::Error as ThisError;

use crate::env::{context_id, random_bytes};

/// Globally-unique identifier for an [`Element`](crate::entities::Element).
///
/// This is unique across the entire context, across all devices and all time.
/// We use UUIDv4 for this, which provides a 128-bit value designed to be unique
/// across time and space, and uses randomness to help ensure this. Critically,
/// there is no need to coordinate with other systems to ensure uniqueness, or
/// to have any central authority to allocate these. The possibility of having a
/// collision is technically non-zero, but is so astronomically low that it can
/// be considered negligible.
///
/// We use a newtype pattern here to give semantic meaning to the identifier,
/// and to be able to add specific functionality to the IDs that we need for the
/// system operation. Abstracting the true type away provides a level of
/// insulation that is useful for any future changes.
///
#[derive(
    BorshSerialize,
    BorshDeserialize,
    Copy,
    Clone,
    Debug,
    Eq,
    PartialEq,
    Hash,
    Ord,
    PartialOrd,
    Default,
)]
pub struct Id {
    /// The byte array representation of the ID.
    bytes: [u8; 32],
}

impl Id {
    /// Creates a new globally-unique identifier.
    ///
    /// Returns the byte array representation of the ID.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use calimero_storage::address::Id;
    /// let id = Id::new([0; 32]);
    /// assert_eq!(id.as_bytes(), &[0; 32]);
    /// ```
    #[must_use]
    pub const fn new(bytes: [u8; 32]) -> Self {
        // random_bytes(&mut bytes);
        Self { bytes }
    }

    /// Root ID which is set to the context ID.
    #[must_use]
    pub fn root() -> Self {
        Self::new(context_id())
    }

    /// Creates a new random globally-unique identifier.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use calimero_storage::address::Id;
    /// let id = Id::random();
    /// ```
    #[must_use]
    pub fn random() -> Self {
        let mut bytes = [0_u8; 32];
        random_bytes(&mut bytes);
        Self::new(bytes)
    }

    /// Returns the byte array representation of the ID.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use calimero_storage::address::Id;
    /// let id = Id::new([0; 32]);
    /// assert_eq!(id.as_bytes(), &[0; 32]);
    /// ```
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.bytes
    }

    /// Checks if the ID is the root.
    pub fn is_root(&self) -> bool {
        self.bytes == context_id()
    }
}

impl Display for Id {
    #[expect(clippy::use_debug, reason = "fine for now")]
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "{}", hex::encode(self.bytes))
    }
}

impl From<[u8; 32]> for Id {
    fn from(bytes: [u8; 32]) -> Self {
        Self::new(bytes)
    }
}

impl From<Id> for [u8; 32] {
    fn from(id: Id) -> Self {
        id.bytes
    }
}

/// Path to an [`Element`](crate::entities::Element).
///
/// [`Element`](crate::entities::Element)s are stored in a hierarchical
/// structure, and their path represents their location within that structure.
/// Path segments are separated by a double-colon `::`, and all paths are
/// absolute, and should start with a leading separator to enforce the clarity
/// of this plus allow for future expansion of functionality.
///
/// [`Path`]s are case-sensitive, support Unicode, and are limited to 255
/// characters in length. Note, the separators do NOT count towards this limit.
///
/// [`Path`]s are not allowed to be empty.
///
/// There is no formal limit to the levels of hierarchy allowed, but in practice
/// this is limited to 255 levels (assuming a single byte per segment name).
///
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Path {
    /// A list of path segment offsets, where offset `0` is assumed, and not
    /// stored.
    offsets: Vec<u8>,

    /// The path to the element. This is a string of up to 255 characters in
    /// length, and is case-sensitive. Internally the segments are stored
    /// without the separators.
    path: Flexstr<256>,
}

impl Path {
    /// Creates a new [`Path`] from a string.
    ///
    /// # Parameters
    ///
    /// * `path` - The path to the [`Element`](crate::entities::Element).
    ///
    /// # Errors
    ///
    /// An error will be returned if:
    ///
    ///   - The path is empty, including if it contains only separators.
    ///   - The path is too long (the maximum length allowed is 255 characters).
    ///   - Any of the path segments are empty.
    ///   - The path is not absolute, i.e. it does not start with a double colon
    ///     separator `::`.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use calimero_storage::address::Path;
    /// Path::new("::root::node::leaf").unwrap();
    /// ```
    ///
    pub fn new<S: AsRef<str>>(path: S) -> Result<Self, PathError> {
        let string = path.as_ref();

        eprintln!("🔍 Path::new called with: '{}'", string);
        eprintln!("🔍 Path::new - string length: {}", string.len());
        eprintln!(
            "🔍 Path::new - string starts with '::': {}",
            string.starts_with("::")
        );

        if string.is_empty() {
            eprintln!("❌ Path::new - Empty string error");
            return Err(PathError::Empty);
        }
        if !string.starts_with("::") {
            eprintln!("❌ Path::new - Not absolute error");
            return Err(PathError::NotAbsolute);
        }

        #[expect(clippy::string_slice, reason = "We know the string starts with `::`")]
        let segments = string[2..].split("::").collect::<Vec<&str>>();

        eprintln!("🔍 Path::new - segments: {:?}", segments);
        eprintln!("🔍 Path::new - segments count: {}", segments.len());

        if segments.is_empty() {
            eprintln!("❌ Path::new - Empty segments error");
            return Err(PathError::Empty);
        }

        let mut str: Flexstr<256> = Flexstr::new();
        let mut offsets = Vec::with_capacity(segments.len());

        for (i, segment) in segments.iter().enumerate() {
            eprintln!("🔍 Path::new - processing segment {}: '{}'", i, segment);
            if segment.is_empty() {
                eprintln!("❌ Path::new - Empty segment error at index {}", i);
                return Err(PathError::EmptySegment);
            }
            if str.len().saturating_add(segment.len()) > 255 {
                eprintln!("❌ Path::new - Overflow error");
                return Err(PathError::Overflow);
            }
            if str.len() > 0 {
                #[expect(clippy::cast_possible_truncation, reason = "Can't occur here")]
                offsets.push(str.len() as u8);
            }
            let _: bool = str.push_str(segment);
            eprintln!("🔍 Path::new - after push, str length: {}", str.len());
        }

        eprintln!("🔍 Path::new - final str: '{}'", str);
        eprintln!("🔍 Path::new - final offsets: {:?}", offsets);
        eprintln!("🔍 Path::new - success!");

        Ok(Self { offsets, path: str })
    }

    /// The number of segments in the [`Path`].
    ///
    /// Returns the depth of the path, which is one less than the number of
    /// segments, because the roots are level 0.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use calimero_storage::address::Path;
    /// let path = Path::new("::root::node::leaf").unwrap();
    /// assert_eq!(path.depth(), 2);
    /// ```
    ///
    #[must_use]
    pub fn depth(&self) -> usize {
        self.offsets.len()
    }

    /// The first segment of the [`Path`].
    ///
    /// Returns the first segment of the path, which is the top-most in the
    /// hierarchy expressed by the path.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use calimero_storage::address::Path;
    /// let path = Path::new("::root::node::leaf").unwrap();
    /// assert_eq!(path.first(), "root");
    /// ```
    ///
    #[must_use]
    pub fn first(&self) -> &str {
        if self.offsets.is_empty() {
            &self.path
        } else {
            &self.path[..self.offsets[0] as usize]
        }
    }

    /// Checks if the [`Path`] is an ancestor of another [`Path`].
    ///
    /// Returns `true` if the [`Path`] is an ancestor of the other [`Path`], and
    /// `false` otherwise. In order to be counted as an ancestor, the path must
    /// be strictly shorter than the other path, and all segments must match.
    ///
    /// # Parameters
    ///
    /// * `other` - The other [`Path`] to check against.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use calimero_storage::address::Path;
    /// let path1 = Path::new("::root::node").unwrap();
    /// let path2 = Path::new("::root::node::leaf").unwrap();
    /// assert!(path1.is_ancestor_of(&path2));
    /// ```
    ///
    #[must_use]
    pub fn is_ancestor_of(&self, other: &Self) -> bool {
        if self.depth() >= other.depth() {
            return false;
        }
        let mut last_offset = 0_usize;

        for &offset in &self.offsets {
            if self.path[last_offset..offset as usize] != other.path[last_offset..offset as usize] {
                return false;
            }
            last_offset = offset as usize;
        }

        true
    }

    /// Checks if the [`Path`] is a descendant of another [`Path`].
    ///
    /// Returns `true` if the [`Path`] is a descendant of the other [`Path`],
    /// and `false` otherwise. In order to be counted as a descendant, the path
    /// must be strictly longer than the other path, and all segments must
    /// match.
    ///
    /// # Parameters
    ///
    /// * `other` - The other [`Path`] to check against.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use calimero_storage::address::Path;
    /// let path1 = Path::new("::root::node::leaf").unwrap();
    /// let path2 = Path::new("::root::node").unwrap();
    /// assert!(path1.is_descendant_of(&path2));
    /// ```
    ///
    #[must_use]
    pub fn is_descendant_of(&self, other: &Self) -> bool {
        other.is_ancestor_of(self)
    }

    /// Checks if the [`Path`] is the root.
    ///
    /// Returns `true` if the [`Path`] is the root, and `false` otherwise.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use calimero_storage::address::Path;
    /// assert!(Path::new("::root").unwrap().is_root());
    /// assert!(!Path::new("::root::node").unwrap().is_root());
    /// ```
    ///
    #[must_use]
    pub fn is_root(&self) -> bool {
        self.depth() == 0
    }

    /// Joins two [`Path`]s.
    ///
    /// Joins the [`Path`] with another [`Path`], returning a new [`Path`] that
    /// is the concatenation of the two.
    ///
    /// # Parameters
    ///
    /// * `other` - The other [`Path`] to join with.
    ///
    /// # Errors
    ///
    /// An error will be returned if the resulting path would be too long.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use calimero_storage::address::Path;
    /// let path1 = Path::new("::root::node").unwrap();
    /// let path2 = Path::new("::leaf").unwrap();
    /// let joined = path1.join(&path2).unwrap();
    /// assert_eq!(joined.to_string(), "::root::node::leaf");
    /// ```
    ///
    pub fn join(&self, other: &Self) -> Result<Self, PathError> {
        if self.path.len().saturating_add(other.path.len()) > 255 {
            return Err(PathError::Overflow);
        }
        let mut path: Flexstr<256> = Flexstr::new();
        let _: bool = path.push_str(&self.path);
        let _: bool = path.push_str(&other.path);
        let mut offsets = self.offsets.clone();
        #[expect(clippy::cast_possible_truncation, reason = "Can't occur here")]
        offsets.push(self.path.len() as u8);
        #[expect(clippy::cast_possible_truncation, reason = "Can't occur here")]
        offsets.extend(
            other
                .offsets
                .iter()
                .map(|&offset| offset.saturating_add(self.path.len() as u8)),
        );
        Ok(Self { offsets, path })
    }

    /// The last segment of the [`Path`].
    ///
    /// Returns the last segment of the path, which is the bottom-most in the
    /// hierarchy expressed by the path.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use calimero_storage::address::Path;
    /// let path = Path::new("::root::node::leaf").unwrap();
    /// assert_eq!(path.last(), "leaf");
    /// ```
    ///
    #[must_use]
    pub fn last(&self) -> &str {
        if self.offsets.is_empty() {
            &self.path
        } else {
            self.offsets
                .last()
                .map_or(&self.path, |&offset| &self.path[offset as usize..])
        }
    }

    /// The parent of the [`Path`].
    ///
    /// Returns the parent of the [`Path`], which is the path with the last
    /// segment removed. If the path is already at the root, then `None` is
    /// returned.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use calimero_storage::address::Path;
    /// let path = Path::new("::root::node::leaf").unwrap();
    /// assert_eq!(path.parent().unwrap().to_string(), "::root::node");
    /// assert_eq!(Path::new("::root").unwrap().parent(), None);
    /// ```
    ///
    #[must_use]
    pub fn parent(&self) -> Option<Self> {
        self.offsets.last().map(|&offset| Self {
            offsets: self.offsets[..self.offsets.len().saturating_sub(1)].to_vec(),
            path: self.path[..offset as usize].into(),
        })
    }

    /// The segment at a given index.
    ///
    /// Returns the segment at the given index, or `None` if the index is out of
    /// bounds. Note that the root is at index 0.
    ///
    /// # Parameters
    ///
    /// * `index` - The index of the segment to retrieve.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use calimero_storage::address::Path;
    /// let path = Path::new("::root::node::leaf").unwrap();
    /// assert_eq!(path.segment(1).unwrap(), "node");
    /// assert_eq!(path.segment(3), None);
    /// ```
    ///
    #[must_use]
    pub fn segment(&self, index: usize) -> Option<&str> {
        if index > self.depth() {
            return None;
        }
        let start = index.checked_sub(1).map_or(0, |i| self.offsets[i] as usize);
        #[expect(clippy::cast_possible_truncation, reason = "Can't occur here")]
        let end = self
            .offsets
            .get(index)
            .copied()
            .unwrap_or(self.path.len() as u8);
        Some(&self.path[start..end as usize])
    }

    /// The segments of the [`Path`].
    ///
    /// Returns the segments of the path as a vector of strings.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use calimero_storage::address::Path;
    /// let path = Path::new("::root::node::leaf").unwrap();
    /// assert_eq!(path.segments().collect::<Vec<_>>(), vec!["root", "node", "leaf"]);
    /// ```
    ///
    pub fn segments(&self) -> impl Iterator<Item = &str> {
        (0..=self.depth()).filter_map(|i| self.segment(i))
    }
}

impl BorshDeserialize for Path {
    fn deserialize_reader<R: Read>(reader: &mut R) -> Result<Self, IoError> {
        Self::new(&String::deserialize_reader(reader)?)
            .map_err(|err| IoError::new(IoErrorKind::InvalidData, err))
    }
}

impl BorshSerialize for Path {
    fn serialize<W: Write>(&self, writer: &mut W) -> Result<(), IoError> {
        BorshSerialize::serialize(&self.to_string(), writer)
    }
}

impl Display for Path {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let mut last_offset = 0;
        for &offset in &self.offsets {
            write!(f, "::{}", &self.path[last_offset as usize..offset as usize])?;
            last_offset = offset;
        }
        write!(f, "::{}", &self.path[last_offset as usize..])
    }
}

impl From<Path> for String {
    fn from(path: Path) -> Self {
        path.to_string()
    }
}

impl TryFrom<&str> for Path {
    type Error = PathError;

    fn try_from(path: &str) -> Result<Self, PathError> {
        Self::new(path)
    }
}

impl TryFrom<String> for Path {
    type Error = PathError;

    fn try_from(path: String) -> Result<Self, PathError> {
        Self::new(path)
    }
}

/// Errors that can occur when working with [`Path`]s.
#[derive(Copy, Clone, Debug, Eq, Hash, PartialEq, Ord, PartialOrd, ThisError)]
#[non_exhaustive]
pub enum PathError {
    /// The path is empty.
    #[error("Path cannot be empty")]
    Empty,

    /// A path segment is empty.
    #[error("Path segments cannot be empty")]
    EmptySegment,

    /// The path is not absolute. All paths must start with a double-colon
    /// separator, i.e. `::`.
    #[error("Path is not absolute")]
    NotAbsolute,

    /// The path is too long. The maximum length allowed is 255 characters.
    #[error("Path is too long")]
    Overflow,
}
