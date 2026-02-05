//! Replicated Growable Array (RGA) - A CRDT for collaborative text editing
//!
//! RGA provides a conflict-free way to edit text collaboratively by storing
//! each character with ordering metadata in an UnorderedMap.
//!
//! ## Architecture
//!
//! Built on top of existing CRDT infrastructure:
//! - **UnorderedMap**: Provides storage, tombstone deletion, and CRDT merging
//! - **CharId**: Unique identifier combining HLC timestamp + sequence number
//! - **RGA Ordering**: Characters ordered by (left_neighbor, char_id)
//!
//! ## Example
//!
//! ```ignore
//! use calimero_storage::collections::ReplicatedGrowableArray;
//!
//! let mut rga = ReplicatedGrowableArray::new();
//! rga.insert(0, 'H').unwrap();
//! rga.insert(1, 'i').unwrap();
//! assert_eq!(rga.get_text().unwrap(), "Hi");
//!
//! rga.delete(0).unwrap();
//! assert_eq!(rga.get_text().unwrap(), "i");
//! ```

use borsh::{BorshDeserialize, BorshSerialize};

use super::{CrdtType, UnorderedMap};
use crate::collections::error::StoreError;
use crate::env;
use crate::store::{MainStorage, StorageAdaptor};

/// Unique identifier for a character in the RGA
///
/// Combines HLC timestamp with sequence number for global uniqueness.
/// Ordered lexicographically for deterministic conflict resolution.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, BorshSerialize, BorshDeserialize,
)]
struct CharId {
    /// HLC timestamp when character was inserted
    timestamp: crate::logical_clock::HybridTimestamp,
    /// Sequence number for characters inserted in same operation
    seq: u32,
}

impl CharId {
    fn new(timestamp: crate::logical_clock::HybridTimestamp, seq: u32) -> Self {
        Self { timestamp, seq }
    }

    /// Root ID representing the beginning of the document (sentinel)
    fn root() -> Self {
        Self {
            timestamp: crate::logical_clock::HybridTimestamp::default(),
            seq: 0,
        }
    }
}

/// Storage key for a character (owns serialized bytes for AsRef<[u8]>)
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CharKey {
    id: CharId,
    bytes: Vec<u8>,
}

impl BorshSerialize for CharKey {
    fn serialize<W: borsh::io::Write>(&self, writer: &mut W) -> borsh::io::Result<()> {
        // Only serialize the id, bytes can be reconstructed
        self.id.serialize(writer)
    }
}

impl BorshDeserialize for CharKey {
    fn deserialize_reader<R: borsh::io::Read>(reader: &mut R) -> borsh::io::Result<Self> {
        let id = CharId::deserialize_reader(reader)?;
        // Reconstruct bytes from id
        let bytes = borsh::to_vec(&id)
            .map_err(|e| borsh::io::Error::new(borsh::io::ErrorKind::Other, e))?;
        Ok(Self { id, bytes })
    }
}

impl CharKey {
    fn new(id: CharId) -> Self {
        // CharId is a simple fixed-size struct, serialization is infallible in practice
        // Use unwrap_or_default as a safety fallback (should never occur)
        let bytes = borsh::to_vec(&id).unwrap_or_default();
        Self { id, bytes }
    }

    fn id(&self) -> CharId {
        self.id
    }
}

impl AsRef<[u8]> for CharKey {
    fn as_ref(&self) -> &[u8] {
        &self.bytes
    }
}

impl From<CharId> for CharKey {
    fn from(id: CharId) -> Self {
        Self::new(id)
    }
}

/// A character in the RGA with ordering metadata
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub(crate) struct RgaChar {
    /// The actual character content (stored as u32 for Borsh compatibility)
    content: u32,
    /// ID of the character to the left (for RGA ordering)
    left: CharId,
}

impl RgaChar {
    fn new(content: char, left: CharId) -> Self {
        Self {
            content: content as u32,
            left,
        }
    }

    fn as_char(&self) -> char {
        char::from_u32(self.content).unwrap_or('�') // Replacement character for invalid
    }
}

/// Replicated Growable Array - A CRDT for collaborative text editing
///
/// Built on UnorderedMap for automatic CRDT behavior, with RGA ordering logic.
#[derive(Debug, BorshSerialize, BorshDeserialize)]
pub struct ReplicatedGrowableArray<S: StorageAdaptor = MainStorage> {
    /// Characters stored by CharKey with ordering metadata
    #[borsh(bound(serialize = "", deserialize = ""))]
    pub(crate) chars: UnorderedMap<CharKey, RgaChar, S>,
}

impl ReplicatedGrowableArray<MainStorage> {
    /// Create a new empty RGA
    #[must_use]
    pub fn new() -> Self {
        Self::new_internal()
    }

    /// Create a new RGA with a deterministic ID derived from parent ID and field name.
    /// This ensures RGAs get the same ID across all nodes when created with the same
    /// parent and field name.
    ///
    /// # Arguments
    /// * `parent_id` - The ID of the parent collection (None for root-level collections)
    /// * `field_name` - The name of the field containing this RGA
    #[must_use]
    pub fn new_with_field_name(parent_id: Option<crate::address::Id>, field_name: &str) -> Self {
        Self::new_with_field_name_internal(parent_id, field_name)
    }
}

impl Default for ReplicatedGrowableArray<MainStorage> {
    fn default() -> Self {
        Self::new()
    }
}

impl<S: StorageAdaptor> ReplicatedGrowableArray<S> {
    fn new_internal() -> Self {
        Self {
            chars: UnorderedMap::new_internal(),
        }
    }

    /// Create a new RGA with deterministic ID (internal)
    pub(super) fn new_with_field_name_internal(
        parent_id: Option<crate::address::Id>,
        field_name: &str,
    ) -> Self {
        Self {
            chars: UnorderedMap::new_with_field_name_and_crdt_type(
                parent_id,
                field_name,
                CrdtType::Rga,
            ),
        }
    }

    /// Insert a character at the given visible position
    ///
    /// # Errors
    ///
    /// Returns error if position is out of bounds or storage operation fails
    pub fn insert(&mut self, pos: usize, content: char) -> Result<(), StoreError> {
        let timestamp = env::hlc_timestamp();

        // Find the left neighbor at the visible position
        let ordered = self.get_ordered_chars()?;

        let left = if pos == 0 {
            CharId::root()
        } else if pos <= ordered.len() {
            ordered
                .get(pos - 1)
                .map(|(id, _)| *id)
                .ok_or(StoreError::StorageError(
                    crate::interface::StorageError::InvalidData("position out of bounds".into()),
                ))?
        } else {
            return Err(StoreError::StorageError(
                crate::interface::StorageError::InvalidData("position out of bounds".into()),
            ));
        };

        let char_id = CharId::new(timestamp, 0);
        let new_char = RgaChar::new(content, left);

        let _ = self.chars.insert(CharKey::new(char_id), new_char)?;

        Ok(())
    }

    /// Delete the character at the given visible position
    ///
    /// # Errors
    ///
    /// Returns error if position is out of bounds or storage operation fails
    pub fn delete(&mut self, pos: usize) -> Result<(), StoreError> {
        let ordered = self.get_ordered_chars()?;

        let (char_id, _) = ordered.get(pos).ok_or(StoreError::StorageError(
            crate::interface::StorageError::InvalidData("position out of bounds".into()),
        ))?;

        let _ = self.chars.remove(&CharKey::new(*char_id))?;

        Ok(())
    }

    /// Get the current text (excluding deleted characters)
    ///
    /// # Errors
    ///
    /// Returns error if storage operation fails
    pub fn get_text(&self) -> Result<String, StoreError> {
        let ordered = self.get_ordered_chars()?;
        Ok(ordered.iter().map(|(_, c)| c.as_char()).collect())
    }

    /// Get the length of visible text (excluding deleted characters)
    ///
    /// # Errors
    ///
    /// Returns error if storage operation fails
    pub fn len(&self) -> Result<usize, StoreError> {
        self.get_ordered_chars().map(|chars| chars.len())
    }

    /// Check if the text is empty
    ///
    /// # Errors
    ///
    /// Returns error if storage operation fails
    pub fn is_empty(&self) -> Result<bool, StoreError> {
        self.len().map(|len| len == 0)
    }

    /// Insert multiple characters at once (more efficient for strings)
    ///
    /// # Errors
    ///
    /// Returns error if position is out of bounds or storage operation fails
    pub fn insert_str(&mut self, pos: usize, s: &str) -> Result<(), StoreError> {
        let timestamp = env::hlc_timestamp();

        // Find the left neighbor
        let ordered = self.get_ordered_chars()?;
        let mut left = if pos == 0 {
            CharId::root()
        } else if pos <= ordered.len() {
            ordered
                .get(pos - 1)
                .map(|(id, _)| *id)
                .ok_or(StoreError::StorageError(
                    crate::interface::StorageError::InvalidData("position out of bounds".into()),
                ))?
        } else {
            return Err(StoreError::StorageError(
                crate::interface::StorageError::InvalidData("position out of bounds".into()),
            ));
        };

        // Insert each character
        for (seq, content) in s.chars().enumerate() {
            let char_id = CharId::new(timestamp, seq as u32);
            let new_char = RgaChar::new(content, left);

            let _ = self.chars.insert(CharKey::new(char_id), new_char)?;

            // Next char's left is this char
            left = char_id;
        }

        Ok(())
    }

    /// Delete a range of characters
    ///
    /// This operation is idempotent - if the range exceeds the current document length,
    /// it deletes up to the end of the document without error. This ensures delete
    /// operations can be safely applied even when received out of order during sync.
    ///
    /// # Errors
    ///
    /// Returns error if start > end or storage operation fails
    pub fn delete_range(&mut self, start: usize, end: usize) -> Result<(), StoreError> {
        if start > end {
            return Err(StoreError::StorageError(
                crate::interface::StorageError::InvalidData("start must be <= end".into()),
            ));
        }

        let ordered = self.get_ordered_chars()?;

        // Clamp end to the actual length - makes delete idempotent
        // This prevents "out of bounds" errors when operations arrive out of order
        let actual_end = end.min(ordered.len());

        // Delete each character in range (may be empty if start >= ordered.len())
        for (char_id, _) in &ordered[start..actual_end] {
            let _ = self.chars.remove(&CharKey::new(*char_id))?;
        }

        Ok(())
    }

    // Helper: Get all characters in RGA order (excludes deleted automatically via UnorderedMap)
    fn get_ordered_chars(&self) -> Result<Vec<(CharId, RgaChar)>, StoreError> {
        // Get all non-deleted characters from UnorderedMap
        let chars: Vec<(CharId, RgaChar)> = self
            .chars
            .entries()?
            .map(|(key, char)| (key.id(), char))
            .collect();

        // Build ordered list by following left-neighbor links from root
        let mut ordered = Vec::new();
        let mut current_left = CharId::root();

        // Keep iterating until we've placed all characters
        while ordered.len() < chars.len() {
            // Find all characters that come after current_left
            let mut candidates: Vec<_> = chars
                .iter()
                .filter(|(_, c)| c.left == current_left)
                .filter(|(id, _)| !ordered.iter().any(|(placed_id, _)| placed_id == id))
                .collect();

            if candidates.is_empty() {
                // No more characters for this left - find next unplaced char
                // This handles concurrent insertions that created gaps
                if let Some((next_id, next_char)) = chars
                    .iter()
                    .find(|(id, _)| !ordered.iter().any(|(placed_id, _)| placed_id == id))
                {
                    ordered.push((*next_id, next_char.clone()));
                    current_left = *next_id;
                } else {
                    break;
                }
            } else {
                // Sort by CharId in REVERSE order (latest timestamp first)
                // This ensures sequential mid-document insertions are placed correctly:
                // When inserting at position 6 in "Hello World", the new characters
                // should come BEFORE 'W', not after it, even though they have the same left neighbor.
                candidates.sort_by_key(|(id, _)| std::cmp::Reverse(*id));

                // Take the character with highest CharId (latest timestamp)
                let (next_id, next_char) = candidates[0];
                ordered.push((*next_id, next_char.clone()));
                current_left = *next_id;
            }
        }

        Ok(ordered)
    }
}
