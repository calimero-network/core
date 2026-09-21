//! # Fugue Collab
//!
//! A `FugueText` document driven by real nodes over JSON-RPC.
//!
//! Kept separate from `apps/fugue-editor`, which exists to be measured: that
//! app's per-call work mirrors `collaborative-editor`'s so the wall probes
//! differ by the collection and nothing else, and e2e-only surface would spend
//! that property.
//!
//! The workflows assert the merged VALUE of
//! [`get_text`](FugueCollabState::get_text), not just that replicas agree:
//! every text-CRDT bug found so far converged on a deterministically WRONG
//! text, and a hash check passed on all of them.
//!
//! Every `position`, `start` and `end` below is an index into the document's
//! Unicode scalar values, not bytes and not grapheme clusters: a caller
//! counting UTF-8 bytes addresses the wrong character in non-ASCII text.

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

/// Events emitted to WebSocket clients and re-emitted by every node the delta
/// reaches. `position` is the index the AUTHOR saw when they typed, so a
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
    /// Errors if `position` is past the end of the document.
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
    /// Errors if the range is out of bounds or inverted.
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
    pub fn get_text(&self) -> app::Result<String> {
        Ok(self.document.get_text()?)
    }

    /// The document's length in characters, tombstones excluded.
    pub fn get_length(&self) -> app::Result<usize> {
        Ok(self.document.len()?)
    }

    /// The characters in `[start, end)`.
    ///
    /// # Errors
    /// Errors if the range is out of bounds or inverted.
    pub fn text_range(&self, start: usize, end: usize) -> app::Result<String> {
        Ok(self.document.text_range(start, end)?)
    }
}
