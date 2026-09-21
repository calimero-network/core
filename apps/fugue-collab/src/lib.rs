//! # Fugue Collab
//!
//! A `FugueText` document driven by three real nodes over JSON-RPC, so the
//! Fugue rework can be argued from a running network rather than from unit
//! tests and cost tables.
//!
//! Kept separate from `apps/fugue-editor`, which exists to be measured: that
//! app's per-call work is a deliberate mirror of `collaborative-editor`'s so
//! the wall probes differ by the collection and nothing else, and e2e-only
//! surface would spend that property. This one is free to grow methods that
//! would perturb a wall measurement.
//!
//! The workflows assert the merged VALUE of
//! [`get_text`](FugueCollabState::get_text), not just that replicas agree:
//! every critical bug found on this branch converged on a deterministically
//! WRONG text, and a hash check passed on all of them.
//! [`get_length`](FugueCollabState::get_length) then pins that a merge neither
//! lost nor duplicated a character, [`text_range`](FugueCollabState::text_range)
//! and [`char_at`](FugueCollabState::char_at) cover the positional reads
//! `ReplicatedGrowableArray` has no analogue for, and
//! [`delete_range`](FugueCollabState::delete_range) lets a scenario race a
//! delete against a concurrent insert.
//!
//! Every `position`, `start` and `end` below is an index into the document's
//! Unicode scalar values, not bytes and not grapheme clusters. A JSON-RPC
//! caller counting UTF-8 bytes will address the wrong character in any
//! document containing non-ASCII text.

#![allow(clippy::len_without_is_empty)]

use calimero_sdk::{app, env};
use calimero_storage::collections::FugueText;

/// A collaboratively edited document.
#[app::state(emits = FugueCollabEvent)]
#[derive(Debug)]
pub struct FugueCollabState {
    /// The document itself.
    pub document: FugueText,
}

/// Events emitted to WebSocket clients, and carried inside the delta to every
/// other node, where the receiving node re-emits them.
///
/// Note what `position` means on a remote node: it is the index the AUTHOR saw
/// when they typed, before their edit was integrated anywhere else, so a
/// replica holding concurrent edits cannot apply it blindly.
#[app::event]
pub enum FugueCollabEvent {
    /// Text was inserted at `position` by `editor`.
    TextInserted {
        /// Character index the author inserted at.
        position: usize,
        /// The inserted text.
        text: String,
        /// Base58 device id of the writer.
        editor: String,
    },
    /// The half-open character range `[start, end)` was deleted by `editor`.
    TextDeleted {
        /// First deleted character index.
        start: usize,
        /// One past the last deleted character index.
        end: usize,
        /// Base58 device id of the writer.
        editor: String,
    },
}

/// Base58 so an assertion can read it out of a JSON-RPC response.
fn encode_identity(identity: &[u8; 32]) -> String {
    bs58::encode(identity).into_string()
}

#[app::logic]
impl FugueCollabState {
    /// An empty document.
    #[app::init]
    pub fn init() -> FugueCollabState {
        FugueCollabState {
            document: FugueText::new(),
        }
    }

    /// Insert `text` at character index `position`.
    ///
    /// # Errors
    /// Returns an error if `position` is past the end of the document, or if
    /// storage fails.
    pub fn insert_text(&mut self, position: usize, text: String) -> app::Result<()> {
        self.document.insert_str(position, &text)?;

        app::emit!(FugueCollabEvent::TextInserted {
            position,
            text,
            editor: encode_identity(&env::device_id()),
        });

        Ok(())
    }

    /// Delete the half-open character range `[start, end)`.
    ///
    /// # Errors
    /// Returns an error if the range is out of bounds or inverted, or if
    /// storage fails.
    pub fn delete_range(&mut self, start: usize, end: usize) -> app::Result<()> {
        self.document.delete_range(start, end)?;

        app::emit!(FugueCollabEvent::TextDeleted {
            start,
            end,
            editor: encode_identity(&env::device_id()),
        });

        Ok(())
    }

    /// The whole document.
    ///
    /// # Errors
    /// Returns an error if storage fails.
    pub fn get_text(&self) -> app::Result<String> {
        Ok(self.document.get_text()?)
    }

    /// The document's length in characters, tombstones excluded.
    ///
    /// # Errors
    /// Returns an error if storage fails.
    pub fn get_length(&self) -> app::Result<usize> {
        Ok(self.document.len()?)
    }

    /// The characters in `[start, end)`.
    ///
    /// # Errors
    /// Returns an error if the range is out of bounds or inverted, or if
    /// storage fails.
    pub fn text_range(&self, start: usize, end: usize) -> app::Result<String> {
        Ok(self.document.text_range(start, end)?)
    }

    /// The character at `position`, or `None` past the end.
    ///
    /// Returned as a `String` because the ABI has no `char`; it is always
    /// either empty or exactly one character wide.
    ///
    /// # Errors
    /// Returns an error if storage fails.
    pub fn char_at(&self, position: usize) -> app::Result<Option<String>> {
        Ok(self.document.char_at(position)?.map(String::from))
    }
}
