//! A chat log searched through the node's full-text index.
//!
//! Messages live in one `UnorderedMap`. Two declarations are the app's whole
//! part in search:
//!
//! * `#[derive(app::Searchable)]` on [`Message`]: `text` is ranked free text,
//!   with trigrams for substring search; `sender` is an exact filter; `ts` a
//!   number to filter by range.
//! * `app::search_indexes!` names the `messages` map as the `messages`
//!   index, generating the exports the node's indexer calls.
//!
//! The node indexes on its own, off the write path. [`search`](Chat::search)
//! queries through the map, which reads every hit back from state, so a
//! client always sees current messages even while the index lags.
//! [`scan_search`](Chat::scan_search) is the baseline the index replaces: a
//! lowercase substring test over every message, in WASM.
//!
//! Two more indexes show the shapes real apps have:
//!
//! * `replies`, over two collections at once: replies each owned by their
//!   author in an `AuthoredVector`, and pinned replies in a `Moderated`
//!   `SortedMap`. A reply's text is HTML, so it is indexed through
//!   `with = plain_text` (markup is not searchable), and a hidden reply is
//!   left out through `index_if`. [`search_replies`](Chat::search_replies)
//!   ranks newest first and reads each hit back from whichever collection
//!   holds it.
//! * `docs`, over documents whose title is a `FugueText`: editing the title
//!   writes the text's own rows, not the document's, and the index still
//!   follows it.
//!
//! This is the example app for search, and the fixture of its tests
//! (`crates/context`) and its merobox scenario (`workflows/`).

use calimero_sdk::abi::AbiType;
use calimero_sdk::app;
use calimero_sdk::borsh::{BorshDeserialize, BorshSerialize};
use calimero_sdk::search::{Query, SearchCollection};
use calimero_sdk::serde::{Deserialize, Serialize};
use calimero_storage::collections::{
    AuthoredVector, FugueText, LwwRegister, Moderated, SortedMap, UnorderedMap,
};

/// The one index this app declares.
const INDEX: &str = "messages";

/// One message.
#[derive(BorshSerialize, BorshDeserialize, AbiType, app::Mergeable, app::Searchable)]
#[borsh(crate = "calimero_sdk::borsh")]
pub struct Message {
    #[search(keyword)]
    pub sender: LwwRegister<String>,
    #[search(text, infix)]
    pub text: LwwRegister<String>,
    #[search(number)]
    pub ts: LwwRegister<u64>,
}

/// A thread reply, as HTML.
#[derive(Clone, BorshSerialize, BorshDeserialize, AbiType, app::Mergeable, app::Searchable)]
#[borsh(crate = "calimero_sdk::borsh")]
#[search(index_if = Reply::shown)]
pub struct Reply {
    #[search(text, with = plain_text)]
    pub html: LwwRegister<String>,
    #[search(number)]
    pub ts: LwwRegister<u64>,
    pub hidden: LwwRegister<bool>,
}

impl Reply {
    fn shown(&self) -> bool {
        !*self.hidden.get()
    }
}

/// The text a reader sees in `html`: every tag dropped. (A real app decodes
/// entities and separates blocks too; this is the fixture's.)
fn plain_text(html: &LwwRegister<String>) -> Option<String> {
    let mut out = String::new();
    let mut in_tag = false;
    for c in html.get().chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            c if !in_tag => out.push(c),
            _ => {}
        }
    }
    Some(out)
}

/// A document with a collaboratively edited title.
#[derive(BorshSerialize, BorshDeserialize, AbiType, app::Mergeable, app::Searchable)]
#[borsh(crate = "calimero_sdk::borsh")]
pub struct Doc {
    #[search(text)]
    pub title: FugueText,
}

#[app::state]
pub struct Chat {
    /// Message id -> message.
    messages: UnorderedMap<String, Message>,
    /// Thread replies, each owned by its author.
    replies: AuthoredVector<Reply>,
    /// Pinned replies, by key; a moderator may remove any.
    pinned: Moderated<SortedMap<String, Reply>>,
    /// Document id -> document.
    docs: UnorderedMap<String, Doc>,
}

app::search_indexes!(Chat {
    "messages" (version = 1) => messages,
    "replies" (version = 1) => replies | pinned,
    "docs" (version = 1) => docs,
});

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

/// A reply hit, newest first.
#[derive(Debug, Serialize, AbiType)]
#[serde(crate = "calimero_sdk::serde")]
pub struct ReplyHit {
    /// `reply:<hex entity id>` or `pinned:<key>`.
    pub id: String,
    pub html: String,
    pub ts: u64,
}

/// A page of reply hits.
#[derive(Debug, Serialize, AbiType)]
#[serde(crate = "calimero_sdk::serde")]
pub struct ReplyPage {
    pub hits: Vec<ReplyHit>,
    pub next_cursor: Option<u32>,
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
            replies: AuthoredVector::new(),
            pinned: Moderated::new(),
            docs: UnorderedMap::new(),
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

    /// Full-text search.
    ///
    /// # Arguments
    ///
    /// * `query` - The text to find; empty lists every message the filters admit.
    /// * `mode` - `words` (the default), `prefix`, `substring` or `fuzzy`.
    /// * `sender` - Only messages from this sender.
    /// * `since` - Only messages with `ts` at least this.
    /// * `cursor` - The `next_cursor` of the previous page.
    /// * `limit` - Hits per page (20 by default, 100 at most).
    #[app::view]
    pub fn search(
        &self,
        query: String,
        mode: Option<String>,
        sender: Option<String>,
        since: Option<u64>,
        cursor: Option<u32>,
        limit: Option<u32>,
    ) -> app::Result<SearchPage> {
        let mut q = match mode.as_deref() {
            _ if query.is_empty() => Query::all(),
            None | Some("words") => Query::words(query),
            Some("prefix") => Query::prefix(query),
            Some("substring") => Query::substring(query),
            Some("fuzzy") => Query::fuzzy(query),
            Some(other) => app::bail!("unknown mode {other:?}"),
        };
        if let Some(sender) = sender {
            q = q.eq("sender", sender);
        }
        if let Some(since) = since {
            q = q.range("ts", since, u64::MAX);
        }
        let results = self
            .messages
            .search(
                INDEX,
                &q.cursor(cursor.unwrap_or(0))
                    .limit(limit.unwrap_or(Query::DEFAULT_LIMIT)),
            )?
            .map(|id, message| view(id.clone(), &message));
        Ok(SearchPage {
            hits: results
                .hits
                .into_iter()
                .map(|hit| Hit {
                    message: hit.value,
                    score: hit.score,
                    snippet: hit.snippet,
                })
                .collect(),
            total: results.total,
            next_cursor: results.next_cursor,
            stale: results.stale,
        })
    }

    /// Reply in a thread; returns the reply's id (hex).
    pub fn reply(&mut self, html: String, ts: u64) -> app::Result<String> {
        let id = self.replies.push(Reply {
            html: LwwRegister::new(html),
            ts: LwwRegister::new(ts),
            hidden: LwwRegister::new(false),
        })?;
        Ok(hex_id(id.into()))
    }

    /// Hide (or show again) one of the caller's replies.
    pub fn hide_reply(&mut self, id: String, hidden: bool) -> app::Result<()> {
        let id = parse_id(&id)?;
        let Some(mut reply) = self.replies.get_by_id(id.into())? else {
            app::bail!("no reply {}", hex_id(id));
        };
        reply.hidden.set(hidden);
        self.replies.update_by_id(id.into(), reply)?;
        Ok(())
    }

    /// Pin a reply under `key`.
    pub fn pin(&mut self, key: String, html: String, ts: u64) -> app::Result<()> {
        self.pinned.insert(
            key,
            Reply {
                html: LwwRegister::new(html),
                ts: LwwRegister::new(ts),
                hidden: LwwRegister::new(false),
            },
        )?;
        Ok(())
    }

    /// Replies and pinned replies with every word of `query`, newest first:
    /// one index over two collections, each hit read back from the one that
    /// holds it.
    #[app::view]
    pub fn search_replies(
        &self,
        query: String,
        cursor: Option<u32>,
        limit: Option<u32>,
    ) -> app::Result<ReplyPage> {
        let response = Query::words(query)
            .newest_first("ts")
            .cursor(cursor.unwrap_or(0))
            .limit(limit.unwrap_or(Query::DEFAULT_LIMIT))
            .run("replies")?;
        let mut hits = Vec::with_capacity(response.hits.len());
        for hit in &response.hits {
            let (id, reply) = if let Some((id, reply)) = self.replies.search_entry(hit.id)? {
                (format!("reply:{}", hex_id(id)), reply)
            } else if let Some((key, reply)) = self.pinned.search_entry(hit.id)? {
                (format!("pinned:{key}"), reply)
            } else {
                // Gone from state since the index saw it.
                continue;
            };
            if reply.shown() {
                hits.push(ReplyHit {
                    id,
                    html: reply.html.get().clone(),
                    ts: *reply.ts.get(),
                });
            }
        }
        Ok(ReplyPage {
            hits,
            next_cursor: response.next_cursor,
        })
    }

    /// Create a document titled `title`.
    pub fn create_doc(&mut self, id: String, title: String) -> app::Result<()> {
        let mut text = FugueText::new();
        let _ = text.insert_str(0, &title)?;
        let _ = self.docs.insert(id, Doc { title: text })?;
        Ok(())
    }

    /// Append to a document's title: a write to the title's own rows.
    pub fn append_to_title(&mut self, id: String, text: String) -> app::Result<()> {
        let Some(mut doc) = self.docs.get_mut(&id)? else {
            app::bail!("no document {id:?}");
        };
        let end = doc.title.len()?;
        let _ = doc.title.insert_str(end, &text)?;
        Ok(())
    }

    /// Documents whose title has every word of `query`, best match first.
    #[app::view]
    pub fn search_docs(&self, query: String) -> app::Result<Vec<String>> {
        Ok(self
            .docs
            .search("docs", &Query::words(query))?
            .hits
            .into_iter()
            .map(|hit| hit.key)
            .collect())
    }

    /// Test fixture for the view-only contract: a *write* that searches, then
    /// posts what it found. The node hands the search handle to read-only runs
    /// alone, so this always traps before it can post (see
    /// `a_write_can_never_search` in `crates/context`'s search tests).
    pub fn search_in_a_write(&mut self, query: String) -> app::Result<()> {
        let found = self
            .messages
            .search(INDEX, &Query::words(query).limit(1))?
            .total;
        self.post(
            "found".to_owned(),
            "search_in_a_write".to_owned(),
            found.to_string(),
            0,
        )
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
}

fn hex_id(id: [u8; 32]) -> String {
    id.iter().map(|b| format!("{b:02x}")).collect()
}

fn parse_id(hex: &str) -> app::Result<[u8; 32]> {
    let mut out = [0; 32];
    if hex.len() != 64 {
        app::bail!("a reply id is 64 hex digits");
    }
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&hex[2 * i..2 * i + 2], 16)
            .map_err(|_| app::err!("a reply id is 64 hex digits"))?;
    }
    Ok(out)
}
