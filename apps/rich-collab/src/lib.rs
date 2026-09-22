//! A `RichDocument` per key, driven by real nodes over JSON-RPC.
//!
//! Every `position`, `start`, `end` and `at` indexes Unicode scalar values, not
//! bytes. Block, mark and anchor identities cross the boundary as bs58-encoded
//! borsh, so a scenario passes them back verbatim and never parses them.

use std::collections::BTreeMap;
use std::ops::DerefMut;

use calimero_sdk::abi::AbiType;
use calimero_sdk::app;
use calimero_sdk::serde::{Deserialize, Serialize};
use calimero_storage::collections::fugue_text::{Anchor, Bias, IdRange};
use calimero_storage::collections::rich_text::{Attrs, DeltaOp, DeltaUndo, UndoStep};
use calimero_storage::collections::{
    BlockId, BlockView, DefaultMarks, MarkId, RichDocument, Span, UnorderedMap, ValueRef,
};

type Doc = RichDocument<DefaultMarks>;

#[app::state(emits = RichCollabEvent)]
#[derive(Debug)]
pub struct RichCollabState {
    pub docs: UnorderedMap<String, Doc>,
}

/// Ids and anchors, never positions: the payload is replayed on the RECEIVING
/// node, where a concurrent edit has already moved everything the author counted.
#[app::event]
pub enum RichCollabEvent {
    BlockInserted {
        doc: String,
        block: String,
    },
    BlockDeleted {
        doc: String,
        block: String,
    },
    BlockMoved {
        doc: String,
        block: String,
    },
    /// Kind, depth or an attribute changed; re-read the block.
    BlockChanged {
        doc: String,
        block: String,
    },
    TextChanged {
        doc: String,
        block: String,
        ids: Vec<Run>,
    },
    MarkApplied {
        doc: String,
        block: String,
        mark_id: String,
    },
}

/// A run of character ids, mirroring `IdRange`, which has no `AbiType`.
/// `replica` is decimal text because it is a full `u64` and a JSON number loses
/// the top bits in a browser.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, AbiType)]
#[serde(crate = "calimero_sdk::serde")]
pub struct Run {
    pub replica: String,
    pub counter: u32,
    pub len: u32,
}

impl From<IdRange> for Run {
    fn from(range: IdRange) -> Self {
        Self {
            replica: range.start.0.to_string(),
            counter: range.start.1,
            len: range.len,
        }
    }
}

/// One rendered block, mirroring `BlockView` so its id is the same bs58 token
/// every other method takes and returns.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, AbiType)]
#[serde(crate = "calimero_sdk::serde")]
pub struct Block {
    pub id: String,
    pub kind: String,
    pub depth: u8,
    pub attrs: BTreeMap<String, String>,
    pub spans: Vec<Span>,
}

impl Block {
    fn new(view: BlockView) -> app::Result<Self> {
        Ok(Self {
            id: encode_token(&view.id)?,
            kind: view.kind,
            depth: view.depth,
            attrs: view.attrs,
            spans: view.spans,
        })
    }
}

/// One step of an attributed editor change, mirroring `DeltaOp`, which has no
/// `AbiType`. Untagged so the YAML stays Quill's: `- retain: 6`.
#[derive(Clone, Debug, Serialize, Deserialize, AbiType)]
#[serde(crate = "calimero_sdk::serde", untagged)]
pub enum Change {
    Retain {
        retain: usize,
        #[serde(default)]
        attributes: Option<Attrs>,
    },
    Insert {
        insert: String,
        #[serde(default)]
        attributes: Option<Attrs>,
    },
    Delete {
        delete: usize,
    },
}

impl From<Change> for DeltaOp {
    fn from(change: Change) -> Self {
        match change {
            Change::Retain { retain, attributes } => Self::Retain { retain, attributes },
            Change::Insert { insert, attributes } => Self::Insert { insert, attributes },
            Change::Delete { delete } => Self::Delete { delete },
        }
    }
}

/// Opaque identities cross JSON-RPC as one string, so a client never parses them.
fn encode_token<T: calimero_sdk::borsh::BorshSerialize>(value: &T) -> app::Result<String> {
    Ok(bs58::encode(calimero_sdk::borsh::to_vec(value)?).into_string())
}

fn decode_token<T: calimero_sdk::borsh::BorshDeserialize>(token: &str) -> app::Result<T> {
    let bytes = bs58::decode(token).into_vec()?;
    Ok(calimero_sdk::borsh::from_slice(&bytes)?)
}

/// The ids one delta touched, taken off the undo it returned: a `Delete` step
/// names what the delta minted, an `Insert` step names what it took out.
fn touched(undo: &DeltaUndo) -> Vec<Run> {
    let mut out = Vec::new();
    for step in &undo.0 {
        match *step {
            UndoStep::Delete(ids) => out.push(ids.into()),
            UndoStep::Insert { ref removed, .. } => {
                out.extend(removed.ids.iter().copied().map(Run::from));
            }
            UndoStep::Mark { .. } => {}
        }
    }
    out
}

/// One block as a line of the document digest.
fn digest_block(view: &BlockView, out: &mut String) {
    out.push_str(&view.kind);
    out.push('/');
    out.push_str(&view.depth.to_string());
    for (key, value) in &view.attrs {
        out.push_str(&format!("[{key}={value}]"));
    }
    for span in &view.spans {
        let attrs: Vec<String> = span
            .attributes
            .iter()
            .map(|(key, value)| format!("{key}={value}"))
            .collect();
        out.push_str(&format!("{{{}:{}}}", attrs.join(","), span.text));
    }
    out.push(';');
}

#[app::logic]
impl RichCollabState {
    #[app::init]
    pub fn init() -> RichCollabState {
        RichCollabState {
            docs: UnorderedMap::new(),
        }
    }

    // ---- documents ----

    pub fn create_doc(&mut self, doc: String) -> app::Result<()> {
        if self.docs.get(&doc)?.is_some() {
            app::bail!("document '{doc}' already exists");
        }
        let _old = self.docs.insert(doc, Doc::new())?;
        Ok(())
    }

    /// Sorted, so a scenario can assert the whole list: map iteration order is
    /// a hash order and no two replicas have to agree on it.
    pub fn list_docs(&self) -> app::Result<Vec<String>> {
        let mut docs: Vec<String> = self.docs.entries()?.map(|(key, _)| key).collect();
        docs.sort();
        Ok(docs)
    }

    // ---- structure ----

    pub fn insert_block(
        &mut self,
        doc: String,
        after: Option<String>,
        kind: String,
        depth: u8,
    ) -> app::Result<String> {
        let after = after.as_deref().map(decode_token).transpose()?;
        let block = self.write(&doc)?.insert_block(after, &kind, depth)?;
        let block = encode_token(&block)?;
        app::emit!(RichCollabEvent::BlockInserted {
            doc,
            block: block.clone()
        });
        Ok(block)
    }

    pub fn delete_block(&mut self, doc: String, block: String) -> app::Result<()> {
        let id = decode_token(&block)?;
        let _was = self.write(&doc)?.delete_block(id)?;
        app::emit!(RichCollabEvent::BlockDeleted { doc, block });
        Ok(())
    }

    pub fn move_block(
        &mut self,
        doc: String,
        block: String,
        after: Option<String>,
    ) -> app::Result<()> {
        let id = decode_token(&block)?;
        let after = after.as_deref().map(decode_token).transpose()?;
        self.write(&doc)?.move_block(id, after)?;
        app::emit!(RichCollabEvent::BlockMoved { doc, block });
        Ok(())
    }

    pub fn set_kind(&mut self, doc: String, block: String, kind: String) -> app::Result<()> {
        let id = decode_token(&block)?;
        self.write(&doc)?.set_kind(id, &kind)?;
        app::emit!(RichCollabEvent::BlockChanged { doc, block });
        Ok(())
    }

    pub fn set_depth(&mut self, doc: String, block: String, depth: u8) -> app::Result<()> {
        let id = decode_token(&block)?;
        self.write(&doc)?.set_depth(id, depth)?;
        app::emit!(RichCollabEvent::BlockChanged { doc, block });
        Ok(())
    }

    /// `value: null` removes the attribute.
    pub fn set_attr(
        &mut self,
        doc: String,
        block: String,
        key: String,
        value: Option<String>,
    ) -> app::Result<()> {
        let id = decode_token(&block)?;
        self.write(&doc)?.set_attr(id, &key, value.as_deref())?;
        app::emit!(RichCollabEvent::BlockChanged { doc, block });
        Ok(())
    }

    /// Split at visible position `at`, returning the new block's id.
    pub fn split_block(&mut self, doc: String, block: String, at: usize) -> app::Result<String> {
        let id = decode_token(&block)?;
        let new = self.write(&doc)?.split_block(id, at)?;
        let new = encode_token(&new)?;
        app::emit!(RichCollabEvent::BlockInserted {
            doc: doc.clone(),
            block: new.clone()
        });
        app::emit!(RichCollabEvent::BlockChanged { doc, block });
        Ok(new)
    }

    /// Append `second`'s body to `first` and tombstone `second`.
    pub fn merge_blocks(&mut self, doc: String, first: String, second: String) -> app::Result<()> {
        let (head, tail) = (decode_token(&first)?, decode_token(&second)?);
        self.write(&doc)?.merge_blocks(head, tail)?;
        app::emit!(RichCollabEvent::BlockChanged {
            doc: doc.clone(),
            block: first
        });
        app::emit!(RichCollabEvent::BlockDeleted { doc, block: second });
        Ok(())
    }

    // ---- body ----

    /// One editor transaction, text and formatting together, returning an opaque
    /// token `undo` takes.
    pub fn apply_delta(
        &mut self,
        doc: String,
        block: String,
        ops: Vec<Change>,
    ) -> app::Result<String> {
        let ops: Vec<DeltaOp> = ops.into_iter().map(Into::into).collect();
        let id = decode_token(&block)?;
        let undo = self.write(&doc)?.apply_delta(id, &ops)?;
        app::emit!(RichCollabEvent::TextChanged {
            doc,
            block,
            ids: touched(&undo)
        });
        encode_token(&undo)
    }

    /// Take a whole transaction back, returning a token that redoes it.
    pub fn undo(&mut self, doc: String, block: String, token: String) -> app::Result<String> {
        let id: BlockId = decode_token(&block)?;
        let undo: DeltaUndo = decode_token(&token)?;
        let redo = self.write(&doc)?.apply_undo(id, &undo)?;
        app::emit!(RichCollabEvent::TextChanged {
            doc,
            block,
            ids: touched(&redo)
        });
        encode_token(&redo)
    }

    /// Set `key` over visible positions `[start, end)`. `null` is a no-op result
    /// when every character already resolves to that value.
    pub fn mark(
        &mut self,
        doc: String,
        block: String,
        start: usize,
        end: usize,
        key: String,
        value: Option<String>,
    ) -> app::Result<Option<String>> {
        let id = decode_token(&block)?;
        let minted = self
            .write(&doc)?
            .mark(id, start, end, &key, value.as_deref())?;
        self.emit_mark(doc, block, minted)
    }

    pub fn unmark(
        &mut self,
        doc: String,
        block: String,
        start: usize,
        end: usize,
        key: String,
    ) -> app::Result<Option<String>> {
        let id = decode_token(&block)?;
        let minted = self.write(&doc)?.mark(id, start, end, &key, None)?;
        self.emit_mark(doc, block, minted)
    }

    // ---- reads ----

    pub fn get_document(&self, doc: String) -> app::Result<Vec<Block>> {
        self.read(&doc)?
            .blocks()?
            .into_iter()
            .map(Block::new)
            .collect()
    }

    pub fn get_block(&self, doc: String, block: String) -> app::Result<Option<Block>> {
        self.read(&doc)?
            .block(decode_token(&block)?)?
            .map(Block::new)
            .transpose()
    }

    /// One block's rendered spans: the read a binding does on every keystroke.
    pub fn get_block_delta(&self, doc: String, block: String) -> app::Result<Vec<Span>> {
        Ok(self.read(&doc)?.block_delta(decode_token(&block)?)?)
    }

    pub fn get_text(&self, doc: String, block: String) -> app::Result<String> {
        Ok(self
            .read(&doc)?
            .block_body(decode_token(&block)?)?
            .get_text()?)
    }

    pub fn list_blocks(&self, doc: String) -> app::Result<Vec<String>> {
        self.read(&doc)?
            .blocks()?
            .iter()
            .map(|view| encode_token(&view.id))
            .collect()
    }

    /// The ordered document as one canonical line, so three nodes are compared
    /// exactly by one value. Block ids are excluded because they carry the
    /// minting replica, which no two nodes agree on.
    pub fn get_state_digest(&self, doc: String) -> app::Result<String> {
        let mut out = String::new();
        for view in &self.read(&doc)?.blocks()? {
            digest_block(view, &mut out);
        }
        Ok(out)
    }

    /// How many times `needle` appears contiguously in a block's text, which is
    /// an exact claim about interleaving that `contains` cannot make.
    pub fn passage_count(&self, doc: String, block: String, needle: String) -> app::Result<usize> {
        let text = self
            .read(&doc)?
            .block_body(decode_token(&block)?)?
            .get_text()?;
        Ok(text.matches(&needle).count())
    }

    /// A cursor for the gap at `position`, as an opaque token any member resolves.
    pub fn anchor_at(
        &self,
        doc: String,
        block: String,
        position: usize,
        before: bool,
    ) -> app::Result<String> {
        let bias = if before { Bias::Before } else { Bias::After };
        encode_token(
            &self
                .read(&doc)?
                .block_body(decode_token(&block)?)?
                .anchor_at(position, bias)?,
        )
    }

    /// Where anchors sit in THIS replica's block, one tree rebuild for the lot.
    /// `null` is an anchor this replica cannot place yet.
    pub fn resolve_ids(
        &self,
        doc: String,
        block: String,
        anchors: Vec<String>,
    ) -> app::Result<Vec<Option<usize>>> {
        let anchors = anchors
            .iter()
            .map(|token| decode_token::<Anchor>(token))
            .collect::<app::Result<Vec<Anchor>>>()?;
        Ok(self
            .read(&doc)?
            .block_body(decode_token(&block)?)?
            .resolve_many(&anchors)?)
    }

    // ---- internals ----

    fn emit_mark(
        &self,
        doc: String,
        block: String,
        minted: Option<MarkId>,
    ) -> app::Result<Option<String>> {
        let Some(minted) = minted else {
            return Ok(None);
        };
        let mark_id = encode_token(&minted)?;
        app::emit!(RichCollabEvent::MarkApplied {
            doc,
            block,
            mark_id: mark_id.clone()
        });
        Ok(Some(mark_id))
    }
}

/// Outside `#[app::logic]`: these are plumbing, not JSON-RPC surface.
impl RichCollabState {
    fn read(&self, doc: &str) -> app::Result<ValueRef<Doc>> {
        match self.docs.get(doc)? {
            Some(found) => Ok(found),
            None => app::bail!("unknown document '{doc}'"),
        }
    }

    fn write(&mut self, doc: &str) -> app::Result<impl DerefMut<Target = Doc> + '_> {
        match self.docs.get_mut(doc)? {
            Some(found) => Ok(found),
            None => app::bail!("unknown document '{doc}'"),
        }
    }
}

#[cfg(test)]
mod tests {
    use calimero_sdk::serde_json::{from_slice, json, Value};
    use calimero_sdk::testing::TestHost;

    use super::*;

    const DOC: &str = "doc";

    fn host() -> TestHost<RichCollabState> {
        let mut app = TestHost::new(RichCollabState::init);
        app.call(|s| s.create_doc(DOC.to_owned())).unwrap();
        app
    }

    fn insert(app: &mut TestHost<RichCollabState>, after: Option<String>, kind: &str) -> String {
        app.call(|s| s.insert_block(DOC.to_owned(), after, kind.to_owned(), 0))
            .unwrap()
    }

    fn type_text(app: &mut TestHost<RichCollabState>, block: &str, text: &str) {
        let _undo = app
            .call(|s| {
                s.apply_delta(
                    DOC.to_owned(),
                    block.to_owned(),
                    vec![Change::Insert {
                        insert: text.to_owned(),
                        attributes: None,
                    }],
                )
            })
            .unwrap();
    }

    fn payloads(app: &TestHost<RichCollabState>) -> Vec<(String, Value)> {
        app.events()
            .iter()
            .map(|e| (e.kind.clone(), from_slice(&e.data).expect("JSON payload")))
            .collect()
    }

    /// A subscriber is told WHERE to re-read and nothing else, so every payload
    /// has to name the document and the block it points at.
    #[test]
    fn every_event_names_the_document_and_the_block_it_points_at() {
        let mut app = host();
        let block = insert(&mut app, None, "heading");
        type_text(&mut app, &block, "hello world");
        let _mark = app
            .call(|s| {
                s.mark(
                    DOC.to_owned(),
                    block.clone(),
                    0,
                    5,
                    "bold".to_owned(),
                    Some("true".to_owned()),
                )
            })
            .unwrap();
        app.call(|s| s.set_kind(DOC.to_owned(), block.clone(), "paragraph".to_owned()))
            .unwrap();
        app.call(|s| s.move_block(DOC.to_owned(), block.clone(), None))
            .unwrap();
        app.call(|s| s.delete_block(DOC.to_owned(), block.clone()))
            .unwrap();

        let seen = payloads(&app);
        let kinds: Vec<&str> = seen.iter().map(|(kind, _)| kind.as_str()).collect();
        assert_eq!(
            kinds,
            [
                "BlockInserted",
                "TextChanged",
                "MarkApplied",
                "BlockChanged",
                "BlockMoved",
                "BlockDeleted"
            ]
        );
        for (kind, payload) in &seen {
            assert_eq!(payload["doc"], json!(DOC), "{kind} lost the document");
            assert_eq!(payload["block"], json!(block), "{kind} lost the block");
            assert!(
                payload.get("position").is_none() && payload.get("start").is_none(),
                "{kind} ships a position, which indexes the EMITTING node only"
            );
        }
    }

    /// `TextChanged` carries the ids the delta minted, and `MarkApplied` the id
    /// the write returned, so neither needs a position to be replayable.
    #[test]
    fn a_text_change_carries_its_ids_and_a_mark_its_id() {
        let mut app = host();
        let block = insert(&mut app, None, "paragraph");
        type_text(&mut app, &block, "hello world");
        let _ignored = app.take_events();

        let _undo = app
            .call(|s| {
                s.apply_delta(
                    DOC.to_owned(),
                    block.clone(),
                    vec![
                        Change::Retain {
                            retain: 6,
                            attributes: None,
                        },
                        Change::Delete { delete: 5 },
                        Change::Insert {
                            insert: "there".to_owned(),
                            attributes: None,
                        },
                    ],
                )
            })
            .unwrap();
        let mark = app
            .call(|s| {
                s.mark(
                    DOC.to_owned(),
                    block.clone(),
                    0,
                    5,
                    "bold".to_owned(),
                    Some("true".to_owned()),
                )
            })
            .unwrap()
            .expect("the write was not redundant");

        let seen = payloads(&app);
        assert_eq!(seen[0].0, "TextChanged");
        // The minted run first, then the tombstoned one: the undo replays in
        // that order and the payload is the undo's own id list.
        let ids = seen[0].1["ids"].as_array().expect("two runs").clone();
        assert_eq!(ids.len(), 2);
        assert_eq!(ids[0]["counter"], json!(11));
        assert_eq!(ids[0]["len"], json!(5));
        assert_eq!(ids[1]["counter"], json!(6));
        assert_eq!(ids[1]["len"], json!(5));
        assert_eq!(ids[0]["replica"], ids[1]["replica"]);

        assert_eq!(seen[1].0, "MarkApplied");
        assert_eq!(seen[1].1["mark_id"], json!(mark));

        assert_eq!(
            app.view(|s| s.get_text(DOC.to_owned(), block)).unwrap(),
            "hello there"
        );
    }

    /// The digest is what three nodes are compared by, so edits to different
    /// blocks must produce one value whatever order they were applied in.
    #[test]
    fn the_digest_is_blind_to_the_order_of_independent_edits() {
        let digest_after = |reversed: bool| {
            let mut app = host();
            let first = insert(&mut app, None, "heading");
            let second = insert(&mut app, Some(first.clone()), "paragraph");
            let order = if reversed {
                [(&second, "Beta"), (&first, "Alpha")]
            } else {
                [(&first, "Alpha"), (&second, "Beta")]
            };
            for (block, text) in order {
                type_text(&mut app, block, text);
            }
            let _mark = app
                .call(|s| {
                    s.mark(
                        DOC.to_owned(),
                        first.clone(),
                        0,
                        5,
                        "bold".to_owned(),
                        Some("true".to_owned()),
                    )
                })
                .unwrap();
            app.call(|s| {
                s.set_attr(
                    DOC.to_owned(),
                    second.clone(),
                    "align".to_owned(),
                    Some("end".to_owned()),
                )
            })
            .unwrap();
            app.view(|s| s.get_state_digest(DOC.to_owned())).unwrap()
        };

        assert_eq!(
            digest_after(false),
            "heading/0{bold=true:Alpha};paragraph/0[align=end]{:Beta};"
        );
        assert_eq!(digest_after(false), digest_after(true));
    }
}
