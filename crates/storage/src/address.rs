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

use fixedstr::Flexstr;
use thiserror::Error as ThisError;
use uuid::{Bytes, Uuid};

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
#[derive(Copy, Clone, Debug, Eq, Hash, PartialEq, PartialOrd, Ord)]
pub struct Id(Uuid);

impl Id {
    /// Creates a new globally-unique identifier.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use calimero_storage::address::Id;
    /// let id = Id::new();
    /// ```
    ///
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }

    /// Returns a slice of 16 octets containing the value.
    #[must_use]
    pub const fn as_bytes(&self) -> &Bytes {
        self.0.as_bytes()
    }
}

impl Default for Id {
    fn default() -> Self {
        Self::new()
    }
}

impl Display for Id {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<Id> for Uuid {
    fn from(id: Id) -> Self {
        id.0
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
#[derive(Clone, Debug, Eq, Hash, PartialEq, PartialOrd, Ord)]
pub struct Path {
    /// A list of path segment offsets, where offset `0` is assumed, and not
    /// stored.
    offsets: Vec<u8>,

    /// The path to the element. This is a string of up to 255 characters in
    /// length, and is case-sensitive. Internally the segments are stored
    /// without the separators.
    path: Flexstr<255>,
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
    pub fn new<S: Into<String>>(path: S) -> Result<Self, PathError> {
        let string = path.into();

        if string.is_empty() {
            return Err(PathError::Empty);
        }
        if !string.starts_with("::") {
            return Err(PathError::NotAbsolute);
        }

        #[allow(clippy::string_slice)] // We know the string starts with `::`
        let segments = string[2..].split("::").collect::<Vec<&str>>();

        if segments.is_empty() {
            return Err(PathError::Empty);
        }

        let mut str: Flexstr<255> = Flexstr::new();
        let mut offsets = Vec::with_capacity(segments.len());

        for segment in segments {
            if segment.is_empty() {
                return Err(PathError::EmptySegment);
            }
            if str.len().saturating_add(segment.len()) > 255 {
                return Err(PathError::Overflow);
            }
            if str.len() > 0 {
                #[allow(clippy::cast_possible_truncation)] // Can't occur here
                offsets.push(str.len() as u8);
            }
            let _: bool = str.push_str(segment);
        }

        Ok(Self { offsets, path: str })
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
#[derive(Copy, Clone, Debug, Eq, Hash, PartialEq, PartialOrd, Ord, ThisError)]
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
