//! A `FugueText` document driven by real nodes over JSON-RPC.
//!
//! Every `position`, `start` and `end` indexes Unicode scalar values, not bytes.

#![allow(clippy::len_without_is_empty)]

use calimero_sdk::{app, env};
use calimero_storage::collections::FugueText;

#[app::state(emits = FugueCollabEvent)]
#[derive(Debug)]
pub struct FugueCollabState {
    pub document: FugueText,
}

/// `position` is the index the AUTHOR saw, so a replica cannot apply it blindly.
#[app::event]
pub enum FugueCollabEvent {
    TextInserted {
        position: usize,
        text: String,
        editor: String,
    },
    TextDeleted {
        start: usize,
        end: usize,
        editor: String,
    },
}

fn encode_identity(identity: &[u8; 32]) -> String {
    bs58::encode(identity).into_string()
}

#[app::logic]
impl FugueCollabState {
    #[app::init]
    pub fn init() -> FugueCollabState {
        FugueCollabState {
            document: FugueText::new(),
        }
    }

    pub fn insert_text(&mut self, position: usize, text: String) -> app::Result<()> {
        self.document.insert_str(position, &text)?;

        app::emit!(FugueCollabEvent::TextInserted {
            position,
            text,
            editor: encode_identity(&env::device_id()),
        });

        Ok(())
    }

    /// Deletes the half-open character range `[start, end)`.
    pub fn delete_range(&mut self, start: usize, end: usize) -> app::Result<()> {
        self.document.delete_range(start, end)?;

        app::emit!(FugueCollabEvent::TextDeleted {
            start,
            end,
            editor: encode_identity(&env::device_id()),
        });

        Ok(())
    }

    pub fn get_text(&self) -> app::Result<String> {
        Ok(self.document.get_text()?)
    }

    /// Length in characters, tombstones excluded.
    pub fn get_length(&self) -> app::Result<usize> {
        Ok(self.document.len()?)
    }

    /// The characters in the half-open range `[start, end)`.
    pub fn text_range(&self, start: usize, end: usize) -> app::Result<String> {
        Ok(self.document.text_range(start, end)?)
    }
}
