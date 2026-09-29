//! A chat log searched through the node's full-text index (search PoC).
//!
//! Messages live in one `UnorderedMap`; the node indexes them on its own,
//! off the write path, and a view queries the index with [`env::search`].
//!
//! What the app contributes is the part the node cannot do: say what a
//! message *means* as a document. Three views, which a production
//! `#[app::search_index]` macro would generate (as `__calimero_search_*`
//! exports, with the schema in the embedded ABI):
//!
//! * [`search_poc_schema`](Chat::search_poc_schema) — the index: `text`
//!   (BM25, trigram twin for substring match), `sender` (keyword filter), `ts`.
//! * [`search_poc_extract`](Chat::search_poc_extract) — entity ids the node
//!   saw change, to one document or `None` each. An id that is not a message
//!   (the map itself, the root) or is no longer one (deleted) is `None`, which
//!   the node turns into a delete, so replaying an id is harmless.
//! * [`search_poc_scan`](Chat::search_poc_scan) — every message, a page at a
//!   time, for a full build.
//!
//! Each takes and returns borsh, hex-encoded so it travels as a JSON string.
//!
//! [`search`](Chat::search) is what a client calls. The index lags state by up
//! to the indexer's commit interval, so it re-reads every hit from state and
//! drops the ones that are gone. [`scan_search`](Chat::scan_search) is the
//! baseline it replaces: a lowercase substring test over every message.

use calimero_primitives::search::{
    ExtractRequest, ExtractResponse, ScanRequest, ScanResponse, SearchDoc, SearchFieldKind,
    SearchFieldSchema, SearchFilter, SearchIndexSchema, SearchMode, SearchRequest, SearchValue,
};
use calimero_sdk::abi::AbiType;
use calimero_sdk::borsh::{BorshDeserialize, BorshSerialize};
use calimero_sdk::serde::{Deserialize, Serialize};
use calimero_sdk::{app, env};
use calimero_storage::address::Id;
use calimero_storage::collections::{LwwRegister, UnorderedMap};

/// The one index this app declares.
const INDEX: &str = "messages";

/// Bumped when what [`Chat::document`] extracts changes meaning.
const INDEX_VERSION: u32 = 1;

/// One message.
#[derive(BorshSerialize, BorshDeserialize, AbiType, app::Mergeable)]
#[borsh(crate = "calimero_sdk::borsh")]
pub struct Message {
    pub sender: LwwRegister<String>,
    pub text: LwwRegister<String>,
    pub ts: LwwRegister<u64>,
}

#[app::state]
pub struct Chat {
    /// Message id -> message.
    messages: UnorderedMap<String, Message>,
}

/// A message to post (the bulk form).
#[derive(Debug, Deserialize, AbiType)]
#[serde(crate = "calimero_sdk::serde")]
pub struct NewMessage {
    pub id: String,
    pub sender: String,
    pub text: String,
    pub ts: u64,
}

/// A message as a client sees it.
#[derive(Debug, Serialize, AbiType)]
#[serde(crate = "calimero_sdk::serde")]
pub struct MessageView {
    pub id: String,
    pub sender: String,
    pub text: String,
    pub ts: u64,
}

/// One search hit, re-read from state.
#[derive(Debug, Serialize, AbiType)]
#[serde(crate = "calimero_sdk::serde")]
pub struct Hit {
    pub message: MessageView,
    pub score: f32,
    pub snippet: String,
}

/// One page of search results.
#[derive(Debug, Serialize, AbiType)]
#[serde(crate = "calimero_sdk::serde")]
pub struct SearchPage {
    pub hits: Vec<Hit>,
    /// Documents the index matched in all (before the stale filter).
    pub total: u64,
    /// Pass back as `cursor` for the next page.
    pub next_cursor: Option<u32>,
    /// Hits the index returned whose message is gone from state.
    pub stale: u32,
}

/// What the baseline scan found.
#[derive(Debug, Serialize, AbiType)]
#[serde(crate = "calimero_sdk::serde")]
pub struct ScanPage {
    /// Newest first, at most `limit`.
    pub hits: Vec<MessageView>,
    /// Messages that matched.
    pub total: u64,
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push(char::from(DIGITS[usize::from(b >> 4)]));
        out.push(char::from(DIGITS[usize::from(b & 0xF)]));
    }
    out
}

fn unhex(text: &str) -> app::Result<Vec<u8>> {
    fn nibble(c: u8) -> Option<u8> {
        match c {
            b'0'..=b'9' => Some(c - b'0'),
            b'a'..=b'f' => Some(c - b'a' + 10),
            b'A'..=b'F' => Some(c - b'A' + 10),
            _ => None,
        }
    }
    let bytes = text.as_bytes();
    if !bytes.len().is_multiple_of(2) {
        app::bail!("odd-length hex");
    }
    bytes
        .chunks(2)
        .map(|pair| {
            nibble(pair[0])
                .zip(nibble(pair[1]))
                .map(|(hi, lo)| hi << 4 | lo)
                .ok_or_else(|| app::err!("not hex"))
        })
        .collect()
}

fn decode<T: BorshDeserialize>(text: &str) -> app::Result<T> {
    Ok(calimero_sdk::borsh::from_slice(&unhex(text)?)?)
}

fn encode<T: BorshSerialize>(value: &T) -> app::Result<String> {
    Ok(hex(&calimero_sdk::borsh::to_vec(value)?))
}

fn view(id: String, message: &Message) -> MessageView {
    MessageView {
        id,
        sender: message.sender.get().clone(),
        text: message.text.get().clone(),
        ts: *message.ts.get(),
    }
}

#[app::logic]
impl Chat {
    #[app::init]
    pub fn init() -> Chat {
        Chat {
            messages: UnorderedMap::new(),
        }
    }

    /// Post (or overwrite) one message.
    pub fn post(&mut self, id: String, sender: String, text: String, ts: u64) -> app::Result<()> {
        let _ = self.messages.insert(
            id,
            Message {
                sender: LwwRegister::new(sender),
                text: LwwRegister::new(text),
                ts: LwwRegister::new(ts),
            },
        )?;
        Ok(())
    }

    /// Post several messages in one execution (seeding a benchmark corpus).
    pub fn post_many(&mut self, messages: Vec<NewMessage>) -> app::Result<u32> {
        let n = u32::try_from(messages.len()).unwrap_or(u32::MAX);
        for m in messages {
            self.post(m.id, m.sender, m.text, m.ts)?;
        }
        Ok(n)
    }

    /// Replace a message's text.
    pub fn edit(&mut self, id: String, text: String) -> app::Result<()> {
        let Some(mut message) = self.messages.get_mut(&id)? else {
            app::bail!("no message {id:?}");
        };
        message.text.set(text);
        Ok(())
    }

    /// Delete a message.
    pub fn delete(&mut self, id: String) -> app::Result<bool> {
        Ok(self.messages.remove(&id)?.is_some())
    }

    /// One message.
    pub fn get(&self, id: String) -> app::Result<Option<MessageView>> {
        Ok(self.messages.get(&id)?.map(|m| view(id, &m)))
    }

    /// How many messages there are.
    pub fn count(&self) -> app::Result<u64> {
        Ok(self.messages.len()? as u64)
    }

    /// Full-text search. `mode` is `words` (default), `prefix`, `substring` or
    /// `fuzzy`; `sender` restricts to one sender.
    #[app::view]
    pub fn search(
        &self,
        query: String,
        mode: Option<String>,
        sender: Option<String>,
        cursor: Option<u32>,
        limit: Option<u32>,
    ) -> app::Result<SearchPage> {
        let mode = match mode.as_deref() {
            None | Some("words") => SearchMode::Words,
            Some("prefix") => SearchMode::Prefix,
            Some("substring") => SearchMode::Substring,
            Some("fuzzy") => SearchMode::Fuzzy,
            Some(other) => app::bail!("unknown mode {other:?}"),
        };
        let filters = sender
            .map(|value| SearchFilter::Eq {
                field: "sender".to_owned(),
                value,
            })
            .into_iter()
            .collect();
        let response = env::search(&SearchRequest {
            index: INDEX.to_owned(),
            query,
            mode,
            filters,
            cursor: cursor.unwrap_or(0),
            limit: limit.unwrap_or(20),
        })
        .map_err(|reason| app::err!("search refused: {reason}"))?;

        let mut hits = Vec::with_capacity(response.hits.len());
        let mut stale = 0;
        for hit in response.hits {
            // A hit names an entity as the index last saw it; state is the truth.
            match self.messages.get_by_entity_id(Id::new(hit.id))? {
                Some((id, message)) => hits.push(Hit {
                    message: view(id, &message),
                    score: hit.score,
                    snippet: hit.snippet,
                }),
                None => stale += 1,
            }
        }
        Ok(SearchPage {
            hits,
            total: response.total,
            next_cursor: response.next_cursor,
            stale,
        })
    }

    /// The baseline: every message whose lowercased text contains the
    /// lowercased `term`, newest first — what a chat app does without an index.
    #[app::view]
    pub fn scan_search(&self, term: String, limit: Option<u32>) -> app::Result<ScanPage> {
        let term = term.to_lowercase();
        let mut hits: Vec<MessageView> = self
            .messages
            .entries()?
            .filter(|(_, m)| m.text.get().to_lowercase().contains(&term))
            .map(|(id, m)| view(id, &m))
            .collect();
        hits.sort_by_key(|m| core::cmp::Reverse(m.ts));
        let total = hits.len() as u64;
        hits.truncate(limit.unwrap_or(20) as usize);
        Ok(ScanPage { hits, total })
    }

    /// Hex borsh `Vec<SearchIndexSchema>`: the indexes this app declares.
    #[app::view]
    pub fn search_poc_schema(&self) -> app::Result<String> {
        encode(&vec![SearchIndexSchema {
            name: INDEX.to_owned(),
            version: INDEX_VERSION,
            fields: vec![
                SearchFieldSchema {
                    name: "text".to_owned(),
                    kind: SearchFieldKind::Text {
                        weight: 100,
                        infix: true,
                    },
                },
                SearchFieldSchema {
                    name: "sender".to_owned(),
                    kind: SearchFieldKind::Keyword,
                },
                SearchFieldSchema {
                    name: "ts".to_owned(),
                    kind: SearchFieldKind::U64,
                },
            ],
        }])
    }

    /// Hex borsh `ExtractRequest` in, hex borsh `ExtractResponse` out.
    #[app::view]
    pub fn search_poc_extract(&self, request: String) -> app::Result<String> {
        let request: ExtractRequest = decode(&request)?;
        let docs: ExtractResponse = if request.index == INDEX {
            request
                .ids
                .iter()
                .map(|id| {
                    Ok(self
                        .messages
                        .get_by_entity_id(Id::new(*id))?
                        .map(|(_, m)| Self::document(*id, &m)))
                })
                .collect::<app::Result<_>>()?
        } else {
            request.ids.iter().map(|_| None).collect()
        };
        encode(&docs)
    }

    /// Hex borsh `ScanRequest` in, hex borsh `ScanResponse` out.
    #[app::view]
    pub fn search_poc_scan(&self, request: String) -> app::Result<String> {
        let request: ScanRequest = decode(&request)?;
        if request.index != INDEX {
            return encode(&ScanResponse::default());
        }
        let limit = request.limit as usize;
        let mut docs = Vec::with_capacity(limit);
        let mut seen = 0_usize;
        for (key, message) in self.messages.entries()?.skip(request.offset as usize) {
            if docs.len() == limit {
                break;
            }
            let id: [u8; 32] = self.messages.entity_id_of(&key).into();
            docs.push(Self::document(id, &message));
            seen += 1;
        }
        let next = (docs.len() == limit)
            .then(|| u32::try_from(request.offset as usize + seen).ok())
            .flatten();
        encode(&ScanResponse { docs, next })
    }
}

impl Chat {
    fn document(id: [u8; 32], message: &Message) -> SearchDoc {
        SearchDoc {
            id,
            fields: vec![
                (
                    "text".to_owned(),
                    SearchValue::Str(message.text.get().clone()),
                ),
                (
                    "sender".to_owned(),
                    SearchValue::Str(message.sender.get().clone()),
                ),
                ("ts".to_owned(), SearchValue::U64(*message.ts.get())),
            ],
        }
    }
}
