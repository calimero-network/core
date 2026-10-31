//! A team space with one write rule per kind of record, on the component SDK.
//!
//! v1 held each field to a storage write policy that every node checked on applying a
//! peer's write. A component app runs on the app's leader and its writes are the log's,
//! so each policy is a table plus an explicit check against `env::caller()` before
//! anything is written:
//!
//! | record | v1 policy | the rule here |
//! |---|---|---|
//! | founding record, quorum | `Frozen<T>` | written in `init`; no method writes them |
//! | messages | `WriteOnce<SortedMap>` | keyed `(channel, id, author)`: the author's own row, refused (`AlreadyWritten`) once it exists, never edited or removed |
//! | announcements | `ModeratedOnce<UnorderedMap>` | keyed `(id, author)`, never edited; only a row of `moderators` (the app owner from `init`, and whom a moderator appoints) removes one, not its author |
//! | pages and their revisions | `Authored<UnorderedMap>` | keyed by `(author, id)`: `renamePage`, `addRevision` and `deletePage` act on the caller's own; `page` names the author it reads |
//! | evidence | `ContentAddressed<IndexedMap>` | keyed by the SHA-256 of its content: filing equal content again stores nothing; never changed or removed |
//!
//! As in v1, two accounts using one message, announcement or page id hold two
//! independent rows, and listings show one row per author.

use calimero_app_sdk::env::Caller;
use calimero_app_sdk::prelude::*;
use sha2::{Digest, Sha256};

/// A written-once message of one channel.
#[derive(Record)]
#[table(key = ["channel", "id", "author"], index(by_channel = ["channel", "id", "sent_at"]))]
pub struct Message {
    pub channel: String,
    pub id: String,
    pub author: String,
    pub text: String,
    pub sent_at: u64,
}

/// An announcement: written once by its author, removed only by a moderator.
#[derive(Record)]
#[table(key = ["id", "author"])]
pub struct Announcement {
    pub id: String,
    pub author: String,
    pub text: String,
    pub sent_at: u64,
}

/// An account that may remove announcements and appoint moderators.
#[derive(Record)]
#[table(key = "account")]
pub struct Moderator {
    pub account: String,
}

/// A wiki page, its author's own.
#[derive(Record)]
#[table(key = ["author", "id"])]
pub struct Page {
    pub author: String,
    pub id: String,
    pub title: String,
}

/// One revision of a page, owned with the page by the page's author.
#[derive(Record)]
#[table(key = ["author", "page", "revision"], index(of_page = ["author", "page"]))]
pub struct Revision {
    pub author: String,
    pub page: String,
    pub revision: String,
    pub text: String,
}

/// A piece of evidence, under the hash of its content.
#[derive(Record)]
#[table(key = "hash", index(by_kind = ["kind", "body"]))]
pub struct Evidence {
    /// 64 lowercase hex characters.
    pub hash: String,
    pub kind: String,
    pub body: String,
}

/// Fixed at founding.
#[derive(serde::Serialize)]
pub struct Founding {
    pub name: String,
    pub motto: String,
}

/// A message or an announcement as a client sees it.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageView {
    pub key: String,
    pub author: String,
    pub text: String,
    #[serde(with = "calimero_app_sdk::json::u64_str")]
    pub sent_at: u64,
}

/// A page as a client sees it; `revisions` is `[[revision id, text], ..]` by revision id.
#[derive(serde::Serialize)]
pub struct PageView {
    pub id: String,
    pub author: String,
    pub title: String,
    pub revisions: Vec<(String, String)>,
}

#[app::state(version = 1)]
pub struct TeamSpace {
    /// `name`, `motto`, `quorum`: written in `init` only.
    founding: UnorderedMap<String, String>,
    messages: Table<Message>,
    announcements: Table<Announcement>,
    moderators: Table<Moderator>,
    pages: Table<Page>,
    revisions: Table<Revision>,
    evidence: Table<Evidence>,
}

#[app::error]
pub enum Error {
    /// The caller already wrote `key`, and what is written once stays.
    AlreadyWritten { key: String },
    /// The caller holds no page `id`.
    NoPage { id: String },
    /// A `/` in `value` would move a message into another channel.
    HasSeparator { value: String },
    /// `hash` is not a 64-character hex hash.
    BadHash { hash: String },
    /// Only a moderator may do this.
    NotModerator,
}

#[app::event]
pub enum Event {
    MessagePosted { key: String },
    Announced { id: String },
    AnnouncementRemoved { id: String },
    PageChanged { id: String },
    EvidenceFiled { hash: String },
}

const NAME: &str = "calimero core";
const MOTTO: &str = "converge without coordinating";
const QUORUM: u64 = 2;
/// The most rows one listing returns.
const MAX_ROWS: u32 = 1000;

fn caller() -> app::Result<String> {
    match env::caller() {
        Caller::Account(a) | Caller::ApiKey { account: a, .. } => Ok(a),
        _ => Err(AppError::login_required()),
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// The SHA-256 of a piece of evidence: each field length-prefixed, so no two
/// `(kind, body)` pairs share an encoding.
fn content_hash(kind: &str, body: &str) -> String {
    let mut h = Sha256::new();
    for part in [kind, body] {
        h.update((part.len() as u64).to_be_bytes());
        h.update(part.as_bytes());
    }
    hex(&h.finalize())
}

fn message_view(key: String, author: String, text: String, sent_at: u64) -> MessageView {
    MessageView {
        key,
        author,
        text,
        sent_at,
    }
}

#[app::logic]
impl TeamSpace {
    /// Writes the founding record and the quorum, and makes the app owner the first
    /// moderator.
    #[app::init]
    pub fn init() -> app::Result<Self> {
        let mut s = Self::load();
        for (k, v) in [("name", NAME.to_owned()), ("motto", MOTTO.to_owned())] {
            s.founding.insert(&k.to_owned(), &v)?;
        }
        s.founding
            .insert(&"quorum".to_owned(), &QUORUM.to_string())?;
        s.moderators.insert(Moderator {
            account: env::app_owner(),
        })?;
        Ok(s)
    }

    /// The caller's account, as an author field or a moderator row names it.
    #[app::view]
    pub fn me(&self) -> app::Result<String> {
        caller()
    }

    // ── values fixed at founding ───────────────────────────────────────────

    /// What the space was founded with. There is no setter.
    #[app::view]
    pub fn founding(&self) -> app::Result<Founding> {
        Ok(Founding {
            name: self.fixed("name")?,
            motto: self.fixed("motto")?,
        })
    }

    #[app::view]
    pub fn quorum(&self) -> app::Result<u64> {
        Ok(self.fixed("quorum")?.parse().unwrap_or_default())
    }

    /// Who founded the space: whoever ran `init`, the app owner.
    #[app::view]
    pub fn founder(&self) -> app::Result<Option<String>> {
        Ok(Some(env::app_owner()))
    }

    // ── write-once messages ────────────────────────────────────────────────

    /// Posts a message and returns its key, `"<channel>/<id>"`. The caller owns it and
    /// nobody edits or deletes it: posting again at a key the caller wrote is
    /// `AlreadyWritten`. Another account's message at the key is a row of its own.
    pub fn post_message(
        &mut self,
        channel: String,
        id: String,
        text: String,
        sent_at: u64,
    ) -> app::Result<String> {
        if channel.contains('/') {
            return Err(Error::HasSeparator { value: channel }.into());
        }
        let author = caller()?;
        let key = format!("{channel}/{id}");
        if self
            .messages
            .contains(&(channel.as_str(), id.as_str(), author.as_str()))?
        {
            return Err(Error::AlreadyWritten { key }.into());
        }
        self.messages.insert(Message {
            channel,
            id,
            author,
            text,
            sent_at,
        })?;
        env::emit(Event::MessagePosted { key: key.clone() }, Audience::All)?;
        Ok(key)
    }

    /// A channel's messages in id order, then by `sentAt`: one row per author of an id, and
    /// one range of the `byChannel` index.
    #[app::view]
    pub fn channel(&self, channel: String) -> app::Result<Vec<MessageView>> {
        let page = self.messages.find(
            "byChannel",
            &[channel.to_key_part()],
            FindOpts::default().limit(MAX_ROWS),
        )?;
        Ok(page
            .items
            .into_iter()
            .map(|m| {
                message_view(
                    format!("{}/{}", m.channel, m.id),
                    m.author,
                    m.text,
                    m.sent_at,
                )
            })
            .collect())
    }

    // ── moderated, written-once announcements ──────────────────────────────

    /// Announces `text` under `id`; the caller's announcement at `id` is never edited.
    pub fn announce(&mut self, id: String, text: String, sent_at: u64) -> app::Result<()> {
        let author = caller()?;
        if self
            .announcements
            .contains(&(id.as_str(), author.as_str()))?
        {
            return Err(Error::AlreadyWritten { key: id }.into());
        }
        self.announcements.insert(Announcement {
            id: id.clone(),
            author,
            text,
            sent_at,
        })?;
        env::emit(Event::Announced { id }, Audience::All)?;
        Ok(())
    }

    /// Removes every account's announcement at `id`. Moderators only, not even the
    /// announcement's author. `false` if nobody holds `id`.
    pub fn remove_announcement(&mut self, id: String) -> app::Result<bool> {
        self.require_moderator()?;
        let held = self.announcements_at(&id)?;
        for a in &held {
            self.announcements
                .remove(&(a.id.as_str(), a.author.as_str()))?;
        }
        if held.is_empty() {
            return Ok(false);
        }
        env::emit(Event::AnnouncementRemoved { id }, Audience::All)?;
        Ok(true)
    }

    /// Every announcement by id, one row per author holding the id.
    #[app::view]
    pub fn announcements(&self) -> app::Result<Vec<MessageView>> {
        let page = self
            .announcements
            .scan(FindOpts::default().limit(MAX_ROWS))?;
        Ok(page
            .items
            .into_iter()
            .map(|a| message_view(a.id, a.author, a.text, a.sent_at))
            .collect())
    }

    /// Who may remove announcements.
    #[app::view]
    pub fn moderators(&self) -> app::Result<Vec<String>> {
        let page = self.moderators.scan(FindOpts::default().limit(MAX_ROWS))?;
        Ok(page.items.into_iter().map(|m| m.account).collect())
    }

    /// Appoints `account` a moderator. Moderators only.
    pub fn add_moderator(&mut self, account: String) -> app::Result<()> {
        self.require_moderator()?;
        self.moderators.upsert(Moderator { account })?;
        Ok(())
    }

    // ── authored pages with their revisions ────────────────────────────────

    /// Files the caller's page at `id` with revision `r1`, replacing a page of theirs
    /// already there (and its revisions). Another account's page at `id` is its own.
    pub fn create_page(&mut self, id: String, title: String, text: String) -> app::Result<()> {
        let author = caller()?;
        self.drop_revisions(&author, &id)?;
        self.pages.upsert(Page {
            author: author.clone(),
            id: id.clone(),
            title,
        })?;
        self.revisions.insert(Revision {
            author,
            page: id.clone(),
            revision: "r1".to_owned(),
            text,
        })?;
        env::emit(Event::PageChanged { id }, Audience::All)?;
        Ok(())
    }

    /// Renames the caller's page at `id`; `NoPage` if the caller holds none there.
    pub fn rename_page(&mut self, id: String, title: String) -> app::Result<()> {
        let author = caller()?;
        if !self
            .pages
            .update(&(author.as_str(), id.as_str()), |p| p.title = title)?
        {
            return Err(Error::NoPage { id }.into());
        }
        env::emit(Event::PageChanged { id }, Audience::All)?;
        Ok(())
    }

    /// Adds (or replaces) a revision of the caller's page at `id`; `NoPage` if the
    /// caller holds none there, so nobody revises someone else's page.
    pub fn add_revision(&mut self, id: String, revision: String, text: String) -> app::Result<()> {
        let author = caller()?;
        if !self.pages.contains(&(author.as_str(), id.as_str()))? {
            return Err(Error::NoPage { id }.into());
        }
        self.revisions.upsert(Revision {
            author,
            page: id.clone(),
            revision,
            text,
        })?;
        env::emit(Event::PageChanged { id }, Audience::All)?;
        Ok(())
    }

    /// Deletes the caller's page at `id` and its revisions. `false` if the caller holds
    /// none there, whoever else does.
    pub fn delete_page(&mut self, id: String) -> app::Result<bool> {
        let author = caller()?;
        if !self.pages.remove(&(author.as_str(), id.as_str()))? {
            return Ok(false);
        }
        self.drop_revisions(&author, &id)?;
        env::emit(Event::PageChanged { id }, Audience::All)?;
        Ok(true)
    }

    /// `author`'s page at `id`, or `null`. Another account's page at the same id never
    /// answers for this one.
    #[app::view]
    pub fn page(&self, author: String, id: String) -> app::Result<Option<PageView>> {
        let Some(page) = self.pages.get(&(author.as_str(), id.as_str()))? else {
            return Ok(None);
        };
        let revisions = self
            .revisions_of(&author, &id)?
            .into_iter()
            .map(|r| (r.revision, r.text))
            .collect();
        Ok(Some(PageView {
            id,
            author,
            title: page.title,
            revisions,
        }))
    }

    // ── content-addressed evidence ─────────────────────────────────────────

    /// Files a piece of evidence and returns the hash it is stored under. Filing equal
    /// content again returns the same hash and stores nothing.
    pub fn file_evidence(&mut self, kind: String, body: String) -> app::Result<String> {
        let hash = content_hash(&kind, &body);
        if !self.evidence.contains(&hash)? {
            self.evidence.insert(Evidence {
                hash: hash.clone(),
                kind,
                body,
            })?;
            env::emit(Event::EvidenceFiled { hash: hash.clone() }, Audience::All)?;
        }
        Ok(hash)
    }

    /// The evidence under `hash`, or `null`; `BadHash` unless `hash` is 64 hex characters.
    #[app::view]
    pub fn evidence(&self, hash: String) -> app::Result<Option<Evidence>> {
        if hash.len() != 64 || !hash.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(Error::BadHash { hash }.into());
        }
        Ok(self.evidence.get(&hash.to_ascii_lowercase())?)
    }

    /// Every piece of evidence of one kind, by body: one range of the `byKind` index.
    #[app::view]
    pub fn evidence_of_kind(&self, kind: String) -> app::Result<Vec<Evidence>> {
        Ok(self
            .evidence
            .find(
                "byKind",
                &[kind.to_key_part()],
                FindOpts::default().limit(MAX_ROWS),
            )?
            .items)
    }

    // ── helpers ────────────────────────────────────────────────────────────

    fn fixed(&self, name: &str) -> app::Result<String> {
        Ok(self.founding.get(&name.to_owned())?.unwrap_or_default())
    }

    fn require_moderator(&self) -> app::Result<()> {
        if self.moderators.contains(&caller()?)? {
            Ok(())
        } else {
            Err(Error::NotModerator.into())
        }
    }

    fn announcements_at(&self, id: &str) -> app::Result<Vec<Announcement>> {
        let page = self.announcements.scan(
            FindOpts::default()
                .from(vec![id.to_key_part()])
                .to(vec![id.to_key_part(), KeyPart::Bytes(vec![0xFF])])
                .limit(MAX_ROWS),
        )?;
        Ok(page.items.into_iter().filter(|a| a.id == id).collect())
    }

    fn revisions_of(&self, author: &str, page: &str) -> app::Result<Vec<Revision>> {
        Ok(self
            .revisions
            .find(
                "ofPage",
                &[author.to_key_part(), page.to_key_part()],
                FindOpts::default().limit(MAX_ROWS),
            )?
            .items)
    }

    fn drop_revisions(&mut self, author: &str, page: &str) -> app::Result<()> {
        for r in self.revisions_of(author, page)? {
            self.revisions
                .remove(&(r.author.as_str(), r.page.as_str(), r.revision.as_str()))?;
        }
        Ok(())
    }
}
