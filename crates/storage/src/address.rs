//! Element addressing via unique IDs and hierarchical paths.

#[cfg(test)]
#[path = "tests/address.rs"]
mod tests;

use core::fmt::{self, Debug, Display, Formatter};
use std::io::{Error as IoError, ErrorKind as IoErrorKind, Read, Write};

use borsh::{BorshDeserialize, BorshSerialize};
use fixedstr::Flexstr;
use thiserror::Error as ThisError;

use crate::env::{context_id, random_bytes};

/// Globally-unique 32-byte identifier (random bytes, UUIDv4-style).
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
    bytes: [u8; 32],
}

impl Id {
    #[must_use]
    pub const fn new(bytes: [u8; 32]) -> Self {
        Self { bytes }
    }

    #[must_use]
    pub fn root() -> Self {
        Self::new(context_id())
    }

    #[must_use]
    pub fn random() -> Self {
        let mut bytes = [0_u8; 32];
        random_bytes(&mut bytes);
        Self::new(bytes)
    }

    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.bytes
    }

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

/// Hierarchical path (e.g., `::root::node::leaf`).
///
/// Segments separated by `::`. Max 255 chars. Must be absolute (start with `::`).

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Path {
    offsets: Vec<u8>,
    path: Flexstr<256>,
}

impl Path {
    /// Creates path from string.
    ///
    /// # Errors
    /// - `Empty` if path is empty or only separators
    /// - `NotAbsolute` if doesn't start with `::`
    /// - `EmptySegment` if any segment is empty
    /// - `Overflow` if longer than 255 chars
    pub fn new<S: AsRef<str>>(path: S) -> Result<Self, PathError> {
        let string = path.as_ref();

        if string.is_empty() {
            return Err(PathError::Empty);
        }
        if !string.starts_with("::") {
            return Err(PathError::NotAbsolute);
        }

        #[expect(clippy::string_slice, reason = "We know the string starts with `::`")]
        let segments = string[2..].split("::").collect::<Vec<&str>>();

        if segments.is_empty() {
            return Err(PathError::Empty);
        }

        let mut str: Flexstr<256> = Flexstr::new();
        let mut offsets = Vec::with_capacity(segments.len());

        for segment in segments {
            if segment.is_empty() {
                return Err(PathError::EmptySegment);
            }
            if str.len().saturating_add(segment.len()) > 255 {
                return Err(PathError::Overflow);
            }
            if str.len() > 0 {
                #[expect(clippy::cast_possible_truncation, reason = "Can't occur here")]
                offsets.push(str.len() as u8);
            }
            let _: bool = str.push_str(segment);
        }

        Ok(Self { offsets, path: str })
    }

    #[must_use]
    pub fn depth(&self) -> usize {
        self.offsets.len()
    }

    #[must_use]
    pub fn first(&self) -> &str {
        if self.offsets.is_empty() {
            &self.path
        } else {
            &self.path[..self.offsets[0] as usize]
        }
    }

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

    #[must_use]
    pub fn is_descendant_of(&self, other: &Self) -> bool {
        other.is_ancestor_of(self)
    }

    #[must_use]
    pub fn is_root(&self) -> bool {
        self.depth() == 0
    }

    /// Joins two paths.
    ///
    /// # Errors
    /// An error will be returned if the resulting path would be too long.
    ///
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
    /// * `index` - The index of the segment to retrieve.
    ///
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
