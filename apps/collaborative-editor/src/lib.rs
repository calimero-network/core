//! # Collaborative Editor Implementation
//!
//! A real-time collaborative text editor using RGA (Replicated Growable Array) CRDT.
//!
//! This app demonstrates conflict-free collaborative editing where multiple users
//! can edit the same document simultaneously without manual conflict resolution.
//!
//! See README.md for complete documentation and usage examples.

#![allow(clippy::len_without_is_empty)]

use calimero_sdk::borsh::{BorshDeserialize, BorshSerialize};
use calimero_sdk::{app, env};
use calimero_storage::collections::{Counter, LwwRegister, ReplicatedGrowableArray, UnorderedMap};

// === DATA STRUCTURES ===

/// Application state for the collaborative editor
///
/// All fields must be CRDTs to avoid divergence during concurrent updates.
/// Using LWW merge on root state with non-CRDT fields causes data loss.
#[app::state(emits = EditorEvent)]
#[derive(BorshDeserialize, BorshSerialize)]
#[borsh(crate = "calimero_sdk::borsh")]
pub struct EditorState {
    /// The collaborative text document using RGA CRDT
    pub document: ReplicatedGrowableArray,

    /// Total number of edits made to the document (CRDT Counter)
    pub edit_count: Counter,

    /// Metadata (title, owner) stored as CRDT UnorderedMap to prevent divergence
    /// Keys: "title", "owner"
    pub metadata: UnorderedMap<String, LwwRegister<String>>,
}

/// Events emitted by the collaborative editor
#[app::event]
#[derive(Debug, BorshSerialize, BorshDeserialize)]
#[borsh(crate = "calimero_sdk::borsh")]
pub enum EditorEvent {
    /// Emitted when the document is initialized
    DocumentCreated {
        /// Document title
        title: String,
        /// Owner's identity
        owner: String,
    },

    /// Emitted when text is inserted
    TextInserted {
        /// Position where text was inserted
        position: usize,
        /// The text that was inserted
        text: String,
        /// Editor who made the change
        editor: String,
    },

    /// Emitted when text is deleted
    TextDeleted {
        /// Starting position of deletion
        start: usize,
        /// Ending position of deletion
        end: usize,
        /// Editor who made the change
        editor: String,
    },

    /// Emitted when the document title is changed
    TitleChanged {
        /// Old title
        old_title: String,
        /// New title
        new_title: String,
        /// Editor who made the change
        editor: String,
    },
}

// === HELPER FUNCTIONS ===

/// Convert identity bytes to base58 string
fn encode_identity(identity: &[u8; 32]) -> String {
    bs58::encode(identity).into_string()
}

// === APPLICATION LOGIC ===

#[app::logic]
impl EditorState {
    /// Initialize a new collaborative document with a default title
    #[app::init]
    pub fn init() -> EditorState {
        let owner_id = env::executor_id();
        let owner = encode_identity(&owner_id);
        let title = "Untitled Document".to_string();

        app::log!("Initializing collaborative editor: {} by {}", title, owner);

        let mut metadata = UnorderedMap::new();
        let _ = metadata.insert("title".to_string(), title.clone().into());
        let _ = metadata.insert("owner".to_string(), owner.clone().into());

        let state = EditorState {
            document: ReplicatedGrowableArray::new(),
            edit_count: Counter::new(),
            metadata,
        };

        app::emit!(EditorEvent::DocumentCreated { title, owner });

        state
    }

    /// Insert text at a specific position
    ///
    /// # Arguments
    /// * `position` - The position to insert text (0-indexed)
    /// * `text` - The text to insert
    ///
    /// # Returns
    /// * `Ok(())` - Text successfully inserted
    /// * `Err(app::Error)` - Error if position is invalid or insertion fails
    pub fn insert_text(&mut self, position: usize, text: String) -> app::Result<()> {
        let editor_id = env::executor_id();
        let editor = encode_identity(&editor_id);

        app::log!(
            "Inserting '{}' at position {} by {}",
            text,
            position,
            editor
        );

        self.document.insert_str(position, &text)?;

        self.edit_count.increment()?;

        app::emit!(EditorEvent::TextInserted {
            position,
            text: text.clone(),
            editor,
        });

        Ok(())
    }

    /// Delete text in a range
    ///
    /// # Arguments
    /// * `start` - Starting position (inclusive, 0-indexed)
    /// * `end` - Ending position (exclusive, 0-indexed)
    ///
    /// # Returns
    /// * `Ok(())` - Text successfully deleted
    /// * `Err(app::Error)` - Error if range is invalid or deletion fails
    pub fn delete_text(&mut self, start: usize, end: usize) -> app::Result<()> {
        let editor_id = env::executor_id();
        let editor = encode_identity(&editor_id);

        app::log!("Deleting text from {} to {} by {}", start, end, editor);

        self.document.delete_range(start, end)?;

        self.edit_count.increment()?;

        app::emit!(EditorEvent::TextDeleted { start, end, editor });

        Ok(())
    }

    /// Get the current document text
    ///
    /// # Returns
    /// * `Ok(String)` - The current document text
    /// * `Err(app::Error)` - Error if retrieval fails
    pub fn get_text(&self) -> app::Result<String> {
        self.document.get_text().map_err(Into::into)
    }

    /// Get the length of the document
    ///
    /// # Returns
    /// * `Ok(usize)` - The number of characters in the document
    /// * `Err(app::Error)` - Error if retrieval fails
    pub fn get_length(&self) -> app::Result<usize> {
        self.document.len().map_err(Into::into)
    }

    /// Check if the document is empty
    ///
    /// # Returns
    /// * `Ok(bool)` - True if the document is empty
    /// * `Err(app::Error)` - Error if check fails
    pub fn is_empty(&self) -> app::Result<bool> {
        self.document.is_empty().map_err(Into::into)
    }

    /// Set the document title
    ///
    /// # Arguments
    /// * `new_title` - The new document title
    ///
    /// # Returns
    /// * `Ok(())` - Title successfully changed
    /// * `Err(app::Error)` - Error if title is empty
    pub fn set_title(&mut self, new_title: String) -> app::Result<()> {
        if new_title.is_empty() {
            app::bail!("Title cannot be empty");
        }

        let editor_id = env::executor_id();
        let editor = encode_identity(&editor_id);

        let old_title = self.get_title();

        self.metadata
            .insert("title".to_string(), new_title.clone().into())?;

        app::log!(
            "Title changed from '{}' to '{}' by {}",
            old_title,
            new_title,
            editor
        );

        app::emit!(EditorEvent::TitleChanged {
            old_title,
            new_title,
            editor,
        });

        Ok(())
    }

    /// Get the document title
    ///
    /// # Returns
    /// * `String` - The current document title
    pub fn get_title(&self) -> String {
        self.metadata
            .get("title")
            .ok()
            .flatten()
            .map(|v| v.get().clone())
            .unwrap_or_else(|| "Untitled Document".to_string())
    }

    /// Get document statistics
    ///
    /// # Returns
    /// * `Ok(String)` - Formatted statistics
    /// * `Err(app::Error)` - Error if stats retrieval fails
    ///
    /// # Example Output
    /// ```text
    /// Document Statistics:
    /// - Title: My Collaborative Document
    /// - Length: 42 characters
    /// - Total edits: 15
    /// - Owner: 5FHneW46xGXgs5mUiveU4sbTyGBzmstUspZC92UhjJM694ty
    /// ```
    pub fn get_stats(&self) -> app::Result<String> {
        let length = self.get_length()?;
        let total_edits = self.edit_count.value()?;

        let title = self.get_title();
        let owner = self
            .metadata
            .get("owner")
            .ok()
            .flatten()
            .map(|v| v.get().clone())
            .unwrap_or_else(|| "Unknown".to_string());

        Ok(format!(
            "Document Statistics:\n\
             - Title: {}\n\
             - Length: {} characters\n\
             - Total edits: {}\n\
             - Owner: {}",
            title, length, total_edits, owner
        ))
    }

    /// Replace a range of text with new text (atomic operation)
    ///
    /// # Arguments
    /// * `start` - Starting position (inclusive, 0-indexed)
    /// * `end` - Ending position (exclusive, 0-indexed)
    /// * `text` - The new text to insert
    ///
    /// # Returns
    /// * `Ok(())` - Text successfully replaced
    /// * `Err(app::Error)` - Error if operation fails
    pub fn replace_text(&mut self, start: usize, end: usize, text: String) -> app::Result<()> {
        // Delete the range first
        if start < end {
            self.delete_text(start, end)?;
        }

        // Then insert the new text at the start position
        if !text.is_empty() {
            self.insert_text(start, text)?;
        }

        Ok(())
    }

    /// Append text to the end of the document
    ///
    /// # Arguments
    /// * `text` - The text to append
    ///
    /// # Returns
    /// * `Ok(())` - Text successfully appended
    /// * `Err(app::Error)` - Error if operation fails
    pub fn append_text(&mut self, text: String) -> app::Result<()> {
        let length = self.get_length()?;
        self.insert_text(length, text)
    }

    /// Clear the entire document
    ///
    /// # Returns
    /// * `Ok(())` - Document successfully cleared
    /// * `Err(app::Error)` - Error if operation fails
    pub fn clear(&mut self) -> app::Result<()> {
        let length = self.get_length()?;
        if length > 0 {
            self.delete_text(0, length)?;
        }
        Ok(())
    }
}
