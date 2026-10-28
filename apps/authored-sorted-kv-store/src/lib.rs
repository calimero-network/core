//! A shared noticeboard where every note belongs to whoever wrote it, and a reader takes a
//! **slice** of the board (one topic) instead of walking all of it.
//!
//! The v1 version of this app was the canonical example for `AuthoredSortedMap`; on the
//! component SDK the same rules are a table and two explicit checks.
//!
//! # The key shape
//!
//! A note's key is `"<topic>/<account>/<seq>"` (seq zero-padded to 8 digits):
//!
//! * the **topic** is what a reader asks for: `readTopic` seeks the `byTopic` index, so
//!   notes under other topics cost nothing to skip;
//! * the **account** is the author's, taken from `env::caller()`, never from an argument, so
//!   a key names one note and its author, and anyone can read it by key alone;
//! * the **seq** makes each of one author's writes its own key: two people posting `seq: 1`
//!   to one topic land on two keys and neither is refused.
//!
//! # The ownership rule
//!
//! **Anyone in the org (member or above) may post; only a note's author may edit or retract
//! it.** `edit` and `retract` compare the stored `author` with `env::caller()`'s account and
//! answer `NotFound` to anyone else (as v1 did: the key held no entry of theirs). Every write
//! runs on the app's leader and is replicated as a log entry, so the check that ran there is
//! the check every replica applies; there is no second, merge-time check to make.
//!
//! # Why the slice matters
//!
//! Insert is open and only an author may remove a note, so a member acting in bad faith can
//! grow the board without bound and nobody else can shrink it. `readTopic` reads only its
//! topic's slice, so such a flood under another topic does not slow it. `flood` and
//! `countAll` exist to demonstrate exactly that (see `scenarios/authored-sorted-kv-store.json`).

use calimero_app_sdk::prelude::*;

/// A note as stored.
#[derive(Record)]
#[table(
    key = "key",
    index(by_topic = ["topic", "posted"]),
    index(by_author = ["topic", "author", "seq"]),
    count(in_topic = ["topic"]),
)]
pub struct Note {
    /// `"<topic>/<author>/<seq:08>"`.
    pub key: String,
    pub topic: String,
    /// The account that posted it; only it may edit or retract the note.
    pub author: String,
    pub seq: u64,
    /// The board-wide posting order, which `readTopic` lists a topic in.
    pub posted: u64,
    pub text: String,
}

/// One note, as a reader sees it.
#[derive(Record)]
#[table(key = "key")]
pub struct NoteView {
    pub key: String,
    /// The account that wrote the note (the stored author, not a part of the key).
    pub author: String,
    pub text: String,
    /// Whether the account named in the key is the note's author. Always true here: keys are
    /// built from the caller. Kept from v1, where a patched peer could file a row under
    /// somebody else's name and this was how a reader spotted it.
    pub key_matches_owner: bool,
}

#[app::state(version = 1)]
pub struct AuthoredSortedKvStore {
    notes: Table<Note>,
    /// The last `posted` handed out.
    posted: Counter,
}

#[app::error]
pub enum Error {
    /// The caller holds no note at `key` (it is absent, or another account's).
    NotFound { key: String },
    /// A `/` in a topic would change which slice the note lands in.
    TopicHasSeparator,
}

#[app::event]
pub enum Event {
    Posted { key: String },
    Edited { key: String },
    Retracted { key: String },
}

/// The page size `readTopic` and `myNotes` read a slice in.
const PAGE: u32 = 500;

/// The account a `"<topic>/<account>/<seq>"` key names.
fn named_account(key: &str) -> Option<&str> {
    key.split('/').nth(1)
}

fn key_of(topic: &str, author: &str, seq: u64) -> String {
    format!("{topic}/{author}/{seq:08}")
}

fn view_of(note: Note) -> NoteView {
    NoteView {
        key_matches_owner: named_account(&note.key) == Some(note.author.as_str()),
        key: note.key,
        author: note.author,
        text: note.text,
    }
}

#[app::logic]
impl AuthoredSortedKvStore {
    #[app::init]
    pub fn init() -> app::Result<Self> {
        Ok(Self::load())
    }

    /// The caller's own account, as keys and `author` fields show it.
    #[app::view]
    pub fn me(&self) -> app::Result<String> {
        guards::require_member()
    }

    /// Posts a note under `topic` at a key naming the caller; returns the key. Posting again
    /// at one of the caller's own keys replaces its text.
    pub fn post(&mut self, topic: String, seq: u64, text: String) -> app::Result<String> {
        let me = guards::require_member()?;
        if topic.contains('/') {
            app::bail!(Error::TopicHasSeparator);
        }
        let key = key_of(&topic, &me, seq);
        self.write(&key, topic, me, seq, text)?;
        env::emit(Event::Posted { key: key.clone() }, Audience::Org)?;
        Ok(key)
    }

    /// Replaces the caller's note at `key`. Only its author may (`NotFound` for anyone else).
    pub fn edit(&mut self, key: String, text: String) -> app::Result<()> {
        let me = guards::require_member()?;
        self.authored_by(&key, &me)?;
        self.notes.update(&key, |n| n.text = text)?;
        env::emit(Event::Edited { key }, Audience::Org)?;
        Ok(())
    }

    /// Takes the caller's note at `key` back and returns its text. Only its author may
    /// (`NotFound` for anyone else).
    pub fn retract(&mut self, key: String) -> app::Result<String> {
        let me = guards::require_member()?;
        let note = self.authored_by(&key, &me)?;
        self.notes.remove(&key)?;
        env::emit(Event::Retracted { key }, Audience::Org)?;
        Ok(note.text)
    }

    /// The text of the note at `key`, whoever wrote it.
    #[app::view]
    pub fn get(&self, key: String) -> app::Result<Option<String>> {
        guards::require_member()?;
        Ok(self.notes.get(&key)?.map(|n| n.text))
    }

    /// The author of the note at `key`, or `""`.
    #[app::view]
    pub fn owner_of(&self, key: String) -> app::Result<String> {
        guards::require_member()?;
        Ok(self.notes.get(&key)?.map(|n| n.author).unwrap_or_default())
    }

    /// Every note under `topic`, in posting order. An index seek over the topic's slice, not
    /// a scan: notes under other topics are not read.
    #[app::view]
    pub fn read_topic(&self, topic: String) -> app::Result<Vec<NoteView>> {
        guards::require_member()?;
        let mut out = Vec::new();
        let mut cursor = None;
        loop {
            let mut opts = FindOpts::default().limit(PAGE);
            if let Some(c) = cursor.take() {
                opts = opts.cursor(c);
            }
            let page = self
                .notes
                .find("byTopic", &[KeyPart::Str(topic.clone())], opts)?;
            out.extend(page.items.into_iter().map(view_of));
            match page.next {
                Some(next) => cursor = Some(next),
                None => return Ok(out),
            }
        }
    }

    /// How many notes under `topic` name their author in their key: `readTopic`'s
    /// `keyMatchesOwner`, counted.
    #[app::view]
    pub fn verified_notes(&self, topic: String) -> app::Result<u32> {
        let n = self
            .read_topic(topic)?
            .iter()
            .filter(|n| n.key_matches_owner)
            .count();
        Ok(u32::try_from(n).unwrap_or(u32::MAX))
    }

    /// The keys of the caller's notes under `topic`, in key order: a seek to the caller's own
    /// slice of the topic, so the work is proportional to their notes, not the topic's.
    #[app::view]
    pub fn my_notes(&self, topic: String) -> app::Result<Vec<String>> {
        let me = guards::require_member()?;
        let mut out = Vec::new();
        let mut cursor = None;
        loop {
            let mut opts = FindOpts::default().limit(PAGE);
            if let Some(c) = cursor.take() {
                opts = opts.cursor(c);
            }
            let page = self.notes.find(
                "byAuthor",
                &[KeyPart::Str(topic.clone()), KeyPart::Str(me.clone())],
                opts,
            )?;
            out.extend(page.items.into_iter().map(|n| n.key));
            match page.next {
                Some(next) => cursor = Some(next),
                None => return Ok(out),
            }
        }
    }

    /// Every note on the board, across topics (the per-topic counts, summed).
    #[app::view]
    pub fn count_all(&self) -> app::Result<u32> {
        guards::require_member()?;
        Ok(u32::try_from(self.notes.count("inTopic", &[])?).unwrap_or(u32::MAX))
    }

    /// Writes `n` empty notes under `topic` (seqs `from..from + n`) and returns the board's
    /// total. A demonstration, not a feature: what a member acting in bad faith does, which
    /// nothing in the app can stop.
    pub fn flood(&mut self, topic: String, from: u64, n: u64) -> app::Result<u32> {
        let me = guards::require_member()?;
        if topic.contains('/') {
            app::bail!(Error::TopicHasSeparator);
        }
        for seq in from..from.saturating_add(n) {
            let key = key_of(&topic, &me, seq);
            self.write(&key, topic.clone(), me.clone(), seq, String::new())?;
        }
        self.count_all()
    }

    /// The note at `key` if `me` wrote it, else `NotFound`.
    fn authored_by(&self, key: &str, me: &str) -> app::Result<Note> {
        match self.notes.get(key)? {
            Some(n) if n.author == me => Ok(n),
            _ => Err(Error::NotFound {
                key: key.to_owned(),
            }
            .into()),
        }
    }

    /// Inserts the caller's note at `key`, or replaces its text if they already hold it.
    fn write(
        &mut self,
        key: &str,
        topic: String,
        author: String,
        seq: u64,
        text: String,
    ) -> app::Result<()> {
        if self.notes.contains(key)? {
            self.notes.update(key, |n| n.text = text)?;
        } else {
            let posted = self.posted.increment()?;
            self.notes.insert(Note {
                key: key.to_owned(),
                topic,
                author,
                seq,
                posted,
                text,
            })?;
        }
        Ok(())
    }
}
