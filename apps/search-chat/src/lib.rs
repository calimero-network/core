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
//! This is the example app for search, and the fixture of its tests
//! (`crates/context`) and its merobox scenario (`workflows/`).

use calimero_sdk::abi::AbiType;
use calimero_sdk::app;
use calimero_sdk::borsh::{BorshDeserialize, BorshSerialize};
use calimero_sdk::search::{Query, SearchCollection};
use calimero_sdk::serde::{Deserialize, Serialize};
use calimero_storage::collections::{LwwRegister, UnorderedMap};

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

#[app::state]
pub struct Chat {
    /// Message id -> message.
    messages: UnorderedMap<String, Message>,
}

app::search_indexes!(Chat {
    "messages" (version = 1) => messages,
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
