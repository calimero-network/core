//! # Fugue Collab
//!
//! A `FugueText` document driven by three real nodes over JSON-RPC, so the
//! Fugue rework can be argued from a running network rather than from unit
//! tests and cost tables.
//!
//! # Why this is not `apps/fugue-editor`
//!
//! `fugue-editor` exists to be measured. Its per-call work is a deliberate
//! mirror of `collaborative-editor`'s — same `Counter`, same metadata map,
//! same log/insert/increment/emit sequence — so that
//! `crates/runtime/tests/fugue_wall.rs` and `rga_wall.rs` differ by the
//! collection and nothing else. Adding e2e-only surface to it would spend that
//! property for no gain, and the walls are the one number on this branch that
//! cannot be re-derived cheaply.
//!
//! This app exists to be *collaborated on*. It carries only what a
//! multi-writer scenario needs to assert something, and it is free to grow
//! methods that would perturb a wall measurement.
//!
//! # What a scenario can prove with it
//!
//! * [`get_text`](FugueCollabState::get_text) — convergence, and the merged
//!   VALUE. Asserting only that replicas agree is not enough: three critical
//!   bugs on this branch converged on a deterministically WRONG value, and a
//!   hash check passed on every one of them. The workflows assert the text.
//! * [`get_length`](FugueCollabState::get_length) — that a merge neither lost
//!   nor duplicated a character, independent of order.
//! * [`text_range`](FugueCollabState::text_range) and
//!   [`char_at`](FugueCollabState::char_at) — positional reads, which
//!   `ReplicatedGrowableArray` has no analogue for.
//! * [`delete_range`](FugueCollabState::delete_range) — so a scenario can race
//!   a delete against a concurrent insert, the shape that produced this
//!   branch's C1 and C2 defects.
//!
//! # Positions are character indices
//!
//! Every `position`, `start` and `end` below is an index into the document's
//! Unicode scalar values — not bytes, and not grapheme clusters. A JSON-RPC
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
/// when they typed, before their edit was integrated anywhere else. A replica
/// holding concurrent edits cannot apply it blindly — that is exactly the gap
/// a receiver-computed patch would close.
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
