//! The `FugueText` twin of `apps/collaborative-editor` and the guest for
//! `crates/runtime/tests/fugue_wall.rs`; its state and per-call work mirror
//! that app so the two walls differ by the collection alone.

#![allow(clippy::len_without_is_empty)]

use calimero_sdk::abi::AbiType;
use calimero_sdk::serde::{Deserialize, Serialize};
use calimero_sdk::{app, env};
use calimero_storage::collections::fugue_text::TextOp;
use calimero_storage::collections::{Counter, FugueText, LwwRegister, UnorderedMap};

#[app::state(emits = FugueEditorEvent)]
pub struct FugueEditorState {
    pub document: FugueText,

    pub edit_count: Counter,

    pub metadata: UnorderedMap<String, LwwRegister<String>>,
}

#[app::event]
pub enum FugueEditorEvent {
    DocumentCreated {
        title: String,
        owner: String,
    },

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

/// One step of an editor change, walking the document as it was before the change.
#[derive(Clone, Debug, Serialize, Deserialize, AbiType)]
#[serde(crate = "calimero_sdk::serde", rename_all = "snake_case")]
pub enum Change {
    Retain(usize),
    Insert(String),
    Delete(usize),
}

impl From<Change> for TextOp {
    fn from(change: Change) -> Self {
        match change {
            Change::Retain(count) => Self::Retain(count),
            Change::Insert(text) => Self::Insert(text),
            Change::Delete(count) => Self::Delete(count),
        }
    }
}

fn encode_identity(identity: &[u8; 32]) -> String {
    bs58::encode(identity).into_string()
}

#[app::logic]
impl FugueEditorState {
    #[app::init]
    pub fn init() -> FugueEditorState {
        let owner_id = env::device_id();
        let owner = encode_identity(&owner_id);
        let title = "Untitled Document".to_string();

        app::log!("Initializing fugue editor: {} by {}", title, owner);

        let mut metadata = UnorderedMap::new();
        // `#[app::init]` returns `Self`, so a failed write can only surface as a panic.
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

    /// Returns an opaque token holding what reverses the whole transaction.
    pub fn apply_delta(&mut self, changes: Vec<Change>) -> app::Result<String> {
        let ops: Vec<TextOp> = changes.into_iter().map(Into::into).collect();
        let steps = self.document.apply_delta(&ops)?;
        self.edit_count.increment()?;

        // The cursor walk `FugueText::apply_delta` just made: each event carries
        // the position the document held once the events before it were applied.
        let editor = encode_identity(&env::device_id());
        let mut position = 0;
        for op in &ops {
            match *op {
                TextOp::Retain(count) => position += count,
                TextOp::Insert(ref text) => {
                    app::emit!(FugueEditorEvent::TextInserted {
                        position,
                        text: text.clone(),
                        editor: editor.clone(),
                    });
                    position += text.chars().count();
                }
                TextOp::Delete(count) => app::emit!(FugueEditorEvent::TextDeleted {
                    start: position,
                    end: position + count,
                    editor: editor.clone(),
                }),
            }
        }

        Ok(bs58::encode(calimero_sdk::borsh::to_vec(&steps)?).into_string())
    }

    pub fn get_text(&self) -> app::Result<String> {
        self.document.get_text().map_err(Into::into)
    }

    /// A one-character `String` because `char` has no `AbiType`.
    pub fn char_at(&self, position: usize) -> app::Result<Option<String>> {
        Ok(self.document.char_at(position)?.map(String::from))
    }

    /// The characters in `start..end`, clamped at the end of the document.
    pub fn text_range(&self, start: usize, end: usize) -> app::Result<String> {
        self.document.text_range(start, end).map_err(Into::into)
    }

    pub fn get_length(&self) -> app::Result<usize> {
        self.document.len().map_err(Into::into)
    }
}
