//! The `FugueText` twin of `apps/collaborative-editor` and the guest for
//! `crates/runtime/tests/fugue_wall.rs`; its state and per-call work mirror
//! that app so the two walls differ by the collection alone.

#![allow(clippy::len_without_is_empty)]

use calimero_sdk::abi::AbiType;
use calimero_sdk::serde::{Deserialize, Serialize};
use calimero_sdk::{app, env};
use calimero_storage::collections::fugue_text::{Anchor, Bias, IdRange, Removed, TextOp, Undo};
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

    /// Ids, not positions: the payload is replayed on the RECEIVING node, where a
    /// concurrent edit has already moved everything the author counted.
    TextInserted {
        ids: Span,
        text: String,
        editor: String,
    },

    TextDeleted {
        ids: Vec<Span>,
        text: String,
        editor: String,
    },
}

/// A run of character ids, mirroring `IdRange`, which has no `AbiType` - the same
/// reason `Change` mirrors `TextOp`. `replica` is decimal text because it is a full
/// `u64` and a JSON number loses the top bits in a browser.
#[derive(Clone, Debug, Serialize, Deserialize, AbiType)]
#[serde(crate = "calimero_sdk::serde")]
pub struct Span {
    pub replica: String,
    pub counter: u32,
    pub len: u32,
}

impl From<IdRange> for Span {
    fn from(range: IdRange) -> Self {
        Self {
            replica: range.start.0.to_string(),
            counter: range.start.1,
            len: range.len,
        }
    }
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

fn emit_deleted(removed: &Removed, editor: &str) {
    app::emit!(FugueEditorEvent::TextDeleted {
        ids: removed.ids.iter().copied().map(Span::from).collect(),
        text: removed.text.clone(),
        editor: editor.to_owned(),
    });
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

        let minted = self.document.insert_str(position, &text)?;

        self.edit_count.increment()?;

        if let Some(ids) = minted {
            app::emit!(FugueEditorEvent::TextInserted {
                ids: ids.into(),
                text,
                editor,
            });
        }

        Ok(())
    }

    /// Returns an opaque token holding what reverses the whole transaction.
    pub fn apply_delta(&mut self, changes: Vec<Change>) -> app::Result<String> {
        let ops: Vec<TextOp> = changes.into_iter().map(Into::into).collect();
        let steps = self.document.apply_delta(&ops)?;
        self.edit_count.increment()?;

        // A non-empty insert always mints, so the steps pair with the ops by kind.
        let editor = encode_identity(&env::device_id());
        let mut typed = ops.iter().filter_map(|op| match *op {
            TextOp::Insert(ref text) if !text.is_empty() => Some(text),
            _ => None,
        });
        for step in &steps {
            match *step {
                Undo::Inserted(ids) => {
                    if let Some(text) = typed.next() {
                        app::emit!(FugueEditorEvent::TextInserted {
                            ids: ids.into(),
                            text: text.clone(),
                            editor: editor.clone(),
                        });
                    }
                }
                Undo::Removed(ref removed) => emit_deleted(removed, &editor),
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

    /// Where an event's ids sit in THIS replica's document, one rebuild for the lot.
    pub fn resolve_ids(&self, ids: Vec<Span>) -> app::Result<Vec<usize>> {
        let anchors = ids
            .iter()
            .map(|span| {
                Ok(Anchor::Char {
                    id: (span.replica.parse()?, span.counter),
                    bias: Bias::Before,
                })
            })
            .collect::<app::Result<Vec<Anchor>>>()?;
        Ok(self.document.resolve_many(&anchors)?)
    }
}
