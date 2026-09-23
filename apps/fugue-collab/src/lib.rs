//! A `FugueText` document driven by real nodes over JSON-RPC.
//!
//! Every `position`, `start` and `end` indexes Unicode scalar values, not bytes.

#![allow(clippy::len_without_is_empty)]

use calimero_sdk::abi::AbiType;
use calimero_sdk::serde::{Deserialize, Serialize};
use calimero_sdk::{app, env};
use calimero_storage::collections::fugue_text::{Anchor, Bias, IdRange, Removed, TextOp};
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

    /// Like `insert_text`, returning an opaque token `undo_insert` takes.
    pub fn insert_text_tracked(&mut self, position: usize, text: String) -> app::Result<String> {
        let minted = self.document.insert_str(position, &text)?;
        self.emit_inserted(position, text);
        Ok(bs58::encode(calimero_sdk::borsh::to_vec(&minted)?).into_string())
    }

    pub fn undo_insert(&mut self, token: String) -> app::Result<()> {
        let bytes = bs58::decode(token).into_vec()?;
        if let Some(minted) = calimero_sdk::borsh::from_slice::<Option<IdRange>>(&bytes)? {
            if let Some(removed) = self.document.delete_ids(&minted)? {
                self.emit_deleted(&removed)?;
            }
        }
        Ok(())
    }

    /// Like `delete_range`, returning an opaque token `undo_delete` takes.
    pub fn delete_range_tracked(&mut self, start: usize, end: usize) -> app::Result<String> {
        let removed = self.document.delete_range(start, end)?;
        if let Some(removed) = &removed {
            self.emit_deleted(removed)?;
        }
        Ok(bs58::encode(calimero_sdk::borsh::to_vec(&removed)?).into_string())
    }

    pub fn undo_delete(&mut self, token: String) -> app::Result<()> {
        let bytes = bs58::decode(token).into_vec()?;
        if let Some(removed) = calimero_sdk::borsh::from_slice::<Option<Removed>>(&bytes)? {
            let position = self.document.resolve(&removed.anchor)?;
            let _minted = self
                .document
                .insert_str_at(&removed.anchor, &removed.text)?;
            self.emit_inserted(position, removed.text);
        }
        Ok(())
    }

    fn emit_inserted(&self, position: usize, text: String) {
        app::emit!(FugueCollabEvent::TextInserted {
            position,
            text,
            editor: encode_identity(&env::device_id()),
        });
    }

    /// The removed characters' gap is where they started; a peer's text typed inside them is not counted.
    fn emit_deleted(&self, removed: &Removed) -> app::Result<()> {
        let start = self.document.resolve(&removed.anchor)?;
        app::emit!(FugueCollabEvent::TextDeleted {
            start,
            end: start + removed.text.chars().count(),
            editor: encode_identity(&env::device_id()),
        });
        Ok(())
    }

    /// A whole editor transaction in one call.
    pub fn apply_delta(&mut self, changes: Vec<Change>) -> app::Result<()> {
        let ops: Vec<TextOp> = changes.into_iter().map(Into::into).collect();
        self.document.apply_delta(&ops)?;

        // The cursor walk `FugueText::apply_delta` just made: each event carries
        // the position the document held once the events before it were applied.
        let editor = encode_identity(&env::device_id());
        let mut position = 0;
        for op in &ops {
            match *op {
                TextOp::Retain(count) => position += count,
                TextOp::Insert(ref text) => {
                    app::emit!(FugueCollabEvent::TextInserted {
                        position,
                        text: text.clone(),
                        editor: editor.clone(),
                    });
                    position += text.chars().count();
                }
                TextOp::Delete(count) => app::emit!(FugueCollabEvent::TextDeleted {
                    start: position,
                    end: position + count,
                    editor: editor.clone(),
                }),
            }
        }

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

    /// A cursor for the gap at `position`, as an opaque token any member can resolve.
    pub fn anchor_at(&self, position: usize, before: bool) -> app::Result<String> {
        let bias = if before { Bias::Before } else { Bias::After };
        let anchor = self.document.anchor_at(position, bias)?;
        Ok(bs58::encode(calimero_sdk::borsh::to_vec(&anchor)?).into_string())
    }

    pub fn resolve_anchor(&self, anchor: String) -> app::Result<usize> {
        let bytes = bs58::decode(anchor).into_vec()?;
        let anchor: Anchor = calimero_sdk::borsh::from_slice(&bytes)?;
        Ok(self.document.resolve(&anchor)?)
    }
}

#[cfg(test)]
mod tests {
    use calimero_sdk::serde_json::{from_slice, json, Value};
    use calimero_sdk::testing::TestHost;

    use super::*;

    /// A subscriber replaying these in order must land on the text we wrote.
    #[test]
    fn apply_delta_emits_one_event_per_change() {
        let mut app = TestHost::new(FugueCollabState::init);

        app.call(|s| s.insert_text(0, "hello world".to_owned()))
            .unwrap();
        let _ignored = app.take_events();

        app.call(|s| {
            s.apply_delta(vec![
                Change::Retain(6),
                Change::Delete(5),
                Change::Insert("there".to_owned()),
            ])
        })
        .unwrap();

        let events = app.events();
        let kinds: Vec<&str> = events.iter().map(|e| e.kind.as_str()).collect();
        assert_eq!(kinds, ["TextDeleted", "TextInserted"]);

        let payload = |at: usize| -> Value { from_slice(&events[at].data).expect("JSON payload") };
        assert_eq!(payload(0)["start"], json!(6));
        assert_eq!(payload(0)["end"], json!(11));
        assert_eq!(payload(1)["position"], json!(6));
        assert_eq!(payload(1)["text"], json!("there"));

        assert_eq!(app.view(|s| s.get_text()).unwrap(), "hello there");
    }

    /// The tracked edits and their undos change the document, so they must say so.
    #[test]
    fn tracked_edits_and_their_undos_emit_events() {
        let mut app = TestHost::new(FugueCollabState::init);
        app.call(|s| s.insert_text(0, "hello world".to_owned()))
            .unwrap();
        let _ignored = app.take_events();

        let cut = app.call(|s| s.delete_range_tracked(6, 11)).unwrap();
        app.call(|s| s.undo_delete(cut)).unwrap();
        let typed = app
            .call(|s| s.insert_text_tracked(0, ">> ".to_owned()))
            .unwrap();
        app.call(|s| s.undo_insert(typed)).unwrap();
        assert_eq!(app.view(|s| s.get_text()).unwrap(), "hello world");

        let events = app.events();
        let seen: Vec<(String, Value)> = events
            .iter()
            .map(|e| (e.kind.clone(), from_slice(&e.data).expect("JSON payload")))
            .collect();
        let expected = [
            ("TextDeleted", json!({"start": 6, "end": 11})),
            ("TextInserted", json!({"position": 6, "text": "world"})),
            ("TextInserted", json!({"position": 0, "text": ">> "})),
            ("TextDeleted", json!({"start": 0, "end": 3})),
        ];
        assert_eq!(seen.len(), expected.len(), "{seen:?}");
        for ((kind, payload), (want_kind, want)) in seen.iter().zip(&expected) {
            assert_eq!(kind, want_kind);
            for (key, value) in want.as_object().expect("object") {
                assert_eq!(&payload[key], value, "{kind}.{key}");
            }
        }
    }
}
