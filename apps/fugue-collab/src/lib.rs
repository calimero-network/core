//! A `FugueText` document driven by real nodes over JSON-RPC.
//!
//! Every `position`, `start` and `end` indexes Unicode scalar values, not bytes.

#![allow(clippy::len_without_is_empty)]

use calimero_sdk::abi::AbiType;
use calimero_sdk::serde::{Deserialize, Serialize};
use calimero_sdk::{app, env};
use calimero_storage::collections::fugue_text::{Anchor, Bias, IdRange, Removed, TextOp, Undo};
use calimero_storage::collections::FugueText;

#[app::state(emits = FugueCollabEvent)]
#[derive(Debug)]
pub struct FugueCollabState {
    pub document: FugueText,
}

/// Ids, not positions: the payload is replayed on the RECEIVING node, where a
/// concurrent edit has already moved everything the author counted.
#[app::event]
pub enum FugueCollabEvent {
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
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, AbiType)]
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

impl Span {
    /// The gap before the run's first character, which is what a subscriber places.
    fn anchor(&self) -> app::Result<Anchor> {
        Ok(Anchor::Char {
            id: (self.replica.parse()?, self.counter),
            bias: Bias::Before,
        })
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

/// Undo payloads cross JSON-RPC as one opaque string, so a client never parses them.
fn encode_token<T: calimero_sdk::borsh::BorshSerialize>(value: &T) -> app::Result<String> {
    Ok(bs58::encode(calimero_sdk::borsh::to_vec(value)?).into_string())
}

fn decode_token<T: calimero_sdk::borsh::BorshDeserialize>(token: &str) -> app::Result<T> {
    let bytes = bs58::decode(token).into_vec()?;
    Ok(calimero_sdk::borsh::from_slice(&bytes)?)
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
        let minted = self.document.insert_str(position, &text)?;
        if let Some(ids) = minted {
            self.emit_inserted(ids, text);
        }
        Ok(())
    }

    /// Deletes the half-open character range `[start, end)`.
    pub fn delete_range(&mut self, start: usize, end: usize) -> app::Result<()> {
        if let Some(removed) = self.document.delete_range(start, end)? {
            self.emit_deleted(&removed);
        }
        Ok(())
    }

    /// Like `insert_text`, returning an opaque token `undo_insert` takes.
    pub fn insert_text_tracked(&mut self, position: usize, text: String) -> app::Result<String> {
        let minted = self.document.insert_str(position, &text)?;
        if let Some(ids) = minted {
            self.emit_inserted(ids, text);
        }
        encode_token(&minted)
    }

    pub fn undo_insert(&mut self, token: String) -> app::Result<()> {
        if let Some(minted) = decode_token::<Option<IdRange>>(&token)? {
            if let Some(removed) = self.document.delete_ids(&minted)? {
                self.emit_deleted(&removed);
            }
        }
        Ok(())
    }

    /// Like `delete_range`, returning an opaque token `undo_delete` takes.
    pub fn delete_range_tracked(&mut self, start: usize, end: usize) -> app::Result<String> {
        let removed = self.document.delete_range(start, end)?;
        if let Some(removed) = &removed {
            self.emit_deleted(removed);
        }
        encode_token(&removed)
    }

    pub fn undo_delete(&mut self, token: String) -> app::Result<()> {
        if let Some(removed) = decode_token::<Option<Removed>>(&token)? {
            if let Some(ids) = self
                .document
                .insert_str_at(&removed.anchor, &removed.text)?
            {
                self.emit_inserted(ids, removed.text);
            }
        }
        Ok(())
    }

    fn emit_inserted(&self, ids: IdRange, text: String) {
        app::emit!(FugueCollabEvent::TextInserted {
            ids: ids.into(),
            text,
            editor: encode_identity(&env::device_id()),
        });
    }

    fn emit_deleted(&self, removed: &Removed) {
        app::emit!(FugueCollabEvent::TextDeleted {
            ids: removed.ids.iter().copied().map(Span::from).collect(),
            text: removed.text.clone(),
            editor: encode_identity(&env::device_id()),
        });
    }

    /// A whole editor transaction in one call, returning an opaque token `undo_delta` takes.
    pub fn apply_delta(&mut self, changes: Vec<Change>) -> app::Result<String> {
        let ops: Vec<TextOp> = changes.into_iter().map(Into::into).collect();
        let steps = self.document.apply_delta(&ops)?;

        // A non-empty insert always mints, so the steps pair with the ops by kind.
        let mut typed = ops.iter().filter_map(|op| match *op {
            TextOp::Insert(ref text) if !text.is_empty() => Some(text),
            _ => None,
        });
        for step in &steps {
            match *step {
                Undo::Inserted(ids) => {
                    if let Some(text) = typed.next() {
                        self.emit_inserted(ids, text.clone());
                    }
                }
                Undo::Removed(ref removed) => self.emit_deleted(removed),
            }
        }

        encode_token(&steps)
    }

    /// Take back a whole transaction, returning a token that redoes it.
    pub fn undo_delta(&mut self, token: String) -> app::Result<String> {
        let steps: Vec<Undo> = decode_token(&token)?;
        let redo = self.document.undo(&steps)?;

        // Re-inserting non-empty text always mints, so these pair by kind too.
        let mut restored = steps.iter().rev().filter_map(|step| match *step {
            Undo::Removed(ref removed) => Some(&removed.text),
            Undo::Inserted(_) => None,
        });
        for step in &redo {
            match *step {
                Undo::Inserted(ids) => {
                    if let Some(text) = restored.next() {
                        self.emit_inserted(ids, text.clone());
                    }
                }
                Undo::Removed(ref removed) => self.emit_deleted(removed),
            }
        }

        encode_token(&redo)
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
        encode_token(&self.document.anchor_at(position, bias)?)
    }

    pub fn resolve_anchor(&self, anchor: String) -> app::Result<usize> {
        Ok(self.document.resolve(&decode_token::<Anchor>(&anchor)?)?)
    }

    /// Where an event's ids sit in THIS replica's document, one rebuild for the lot.
    pub fn resolve_ids(&self, ids: Vec<Span>) -> app::Result<Vec<usize>> {
        let anchors = ids
            .iter()
            .map(Span::anchor)
            .collect::<app::Result<Vec<Anchor>>>()?;
        Ok(self.document.resolve_many(&anchors)?)
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

        // The replica is this host's device id, so only the run within it is fixed.
        let payload = |at: usize| -> Value { from_slice(&events[at].data).expect("JSON payload") };
        let deleted = payload(0);
        assert_eq!(deleted["text"], json!("world"));
        assert_eq!(deleted["ids"].as_array().expect("one run").len(), 1);
        assert_eq!(deleted["ids"][0]["counter"], json!(6));
        assert_eq!(deleted["ids"][0]["len"], json!(5));

        let inserted = payload(1);
        assert_eq!(inserted["text"], json!("there"));
        assert_eq!(inserted["ids"]["counter"], json!(11));
        assert_eq!(inserted["ids"]["len"], json!(5));
        assert_eq!(inserted["ids"]["replica"], deleted["ids"][0]["replica"]);

        assert_eq!(app.view(|s| s.get_text()).unwrap(), "hello there");

        // What a subscriber does with the payload: ids to local positions.
        let span = |value: &Value| Span {
            replica: value["replica"].as_str().expect("replica").to_owned(),
            counter: u32::try_from(value["counter"].as_u64().expect("counter")).expect("u32"),
            len: u32::try_from(value["len"].as_u64().expect("len")).expect("u32"),
        };
        let placed = app
            .view(|s| s.resolve_ids(vec![span(&deleted["ids"][0]), span(&inserted["ids"])]))
            .unwrap();
        // The replacement text was typed at the gap the delete left, and Fugue orders
        // it BEFORE the tombstones, so the deleted run now reads as the document end.
        assert_eq!(placed, [11, 6]);
    }

    /// An editor sends whole transactions, so the transaction is what it undoes.
    #[test]
    fn apply_delta_returns_a_token_that_takes_the_whole_change_back() {
        let mut app = TestHost::new(FugueCollabState::init);
        app.call(|s| s.insert_text(0, "hello world".to_owned()))
            .unwrap();

        let undo = app
            .call(|s| {
                s.apply_delta(vec![
                    Change::Retain(6),
                    Change::Delete(5),
                    Change::Insert("there".to_owned()),
                ])
            })
            .unwrap();
        assert_eq!(app.view(|s| s.get_text()).unwrap(), "hello there");

        let redo = app.call(|s| s.undo_delta(undo)).unwrap();
        assert_eq!(app.view(|s| s.get_text()).unwrap(), "hello world");

        let _again = app.call(|s| s.undo_delta(redo)).unwrap();
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
            ("TextDeleted", json!({"text": "world"})),
            ("TextInserted", json!({"text": "world"})),
            ("TextInserted", json!({"text": ">> "})),
            ("TextDeleted", json!({"text": ">> "})),
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
