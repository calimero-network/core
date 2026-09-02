//! # Fugue Editor
//!
//! The `FugueText` counterpart of `apps/collaborative-editor`, and it exists
//! for exactly one reason: `crates/runtime/tests/fugue_wall.rs` needs a REAL
//! compiled app to find where a `FugueText` document stops being writable and
//! readable, the way `rga_wall.rs` uses `collaborative-editor` for
//! `ReplicatedGrowableArray`. A wall measured against a synthetic guest would
//! not be the number a user hits.
//!
//! The state shape and the per-call work of `insert_text` mirror
//! `collaborative-editor` deliberately — same `Counter`, same metadata map,
//! same log/insert/increment/emit sequence — so the two walls differ by the
//! collection and not by the harness around it. What this app adds are the
//! reads `ReplicatedGrowableArray` has no analogue for: [`FugueEditorState::
//! char_at`] and [`FugueEditorState::text_range`].

#![allow(clippy::len_without_is_empty)]

use calimero_sdk::{app, env};
use calimero_storage::collections::{Counter, FugueText, LwwRegister, UnorderedMap};

/// Application state for the Fugue editor. Field-for-field
/// `collaborative-editor`'s `EditorState`, with `FugueText` in place of
/// `ReplicatedGrowableArray`.
#[app::state(emits = FugueEditorEvent)]
pub struct FugueEditorState {
    /// The collaborative text document using the Tree-Fugue CRDT
    pub document: FugueText,

    /// Total number of edits made to the document (CRDT Counter)
    pub edit_count: Counter,

    /// Metadata (title, owner) stored as CRDT UnorderedMap to prevent divergence
    /// Keys: "title", "owner"
    pub metadata: UnorderedMap<String, LwwRegister<String>>,
}

/// Events emitted by the Fugue editor.
#[app::event]
pub enum FugueEditorEvent {
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
}

/// Convert identity bytes to base58 string
fn encode_identity(identity: &[u8; 32]) -> String {
    bs58::encode(identity).into_string()
}

#[app::logic]
impl FugueEditorState {
    /// Initialize a new Fugue document with a default title.
    #[app::init]
    pub fn init() -> FugueEditorState {
        let owner_id = env::device_id();
        let owner = encode_identity(&owner_id);
        let title = "Untitled Document".to_string();

        app::log!("Initializing fugue editor: {} by {}", title, owner);

        let mut metadata = UnorderedMap::new();
        // `#[app::init]` must return `Self`, so it can't propagate a failure
        // with `?`. Surface a storage error loudly rather than silently
        // dropping the write.
        metadata
            .insert("title".to_string(), title.clone().into())
            .expect("failed to write initial title metadata");
        metadata
            .insert("owner".to_string(), owner.clone().into())
            .expect("failed to write initial owner metadata");

        let state = FugueEditorState {
            document: FugueText::new(),
            edit_count: Counter::new(),
            metadata,
        };

        app::emit!(FugueEditorEvent::DocumentCreated { title, owner });

        state
    }

    /// Insert text at a specific position.
    ///
    /// # Errors
    /// Returns an error if the position is invalid or storage fails.
    pub fn insert_text(&mut self, position: usize, text: String) -> app::Result<()> {
        let editor_id = env::device_id();
        let editor = encode_identity(&editor_id);

        app::log!(
            "Inserting '{}' at position {} by {}",
            text,
            position,
            editor
        );

        self.document.insert_str(position, &text)?;

        self.edit_count.increment()?;

        app::emit!(FugueEditorEvent::TextInserted {
            position,
            text: text.clone(),
            editor,
        });

        Ok(())
    }

    /// The whole document.
    ///
    /// # Errors
    /// Returns an error if storage fails.
    pub fn get_text(&self) -> app::Result<String> {
        self.document.get_text().map_err(Into::into)
    }

    /// The character at `position`, or `None` past the end — the positional
    /// read `collaborative-editor` cannot offer, because
    /// `ReplicatedGrowableArray` has none.
    ///
    /// Returned as a one-character `String` rather than a `char`: `char` has no
    /// `AbiType` implementation, so an `Option<char>` return does not compile
    /// under `#[app::logic]`. The extra allocation is one character wide and
    /// does not touch storage, so it cannot move the wall this app is built to
    /// measure.
    ///
    /// # Errors
    /// Returns an error if storage fails.
    pub fn char_at(&self, position: usize) -> app::Result<Option<String>> {
        Ok(self.document.char_at(position)?.map(String::from))
    }

    /// The characters in `start..end`, clamped at the end of the document.
    ///
    /// # Errors
    /// Returns an error if `start > end` or storage fails.
    pub fn text_range(&self, start: usize, end: usize) -> app::Result<String> {
        self.document.text_range(start, end).map_err(Into::into)
    }

    /// The number of characters in the document.
    ///
    /// # Errors
    /// Returns an error if storage fails.
    pub fn get_length(&self) -> app::Result<usize> {
        self.document.len().map_err(Into::into)
    }
}
