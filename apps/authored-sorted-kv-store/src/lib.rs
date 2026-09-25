//! Canonical example for [`AuthoredSortedMap`] — a shared keyspace where every
//! entry is owned by whoever wrote it, and a reader can take a **slice** of the
//! keyspace instead of walking all of it.
//!
//! Read it beside its two neighbours:
//!
//! * [`sorted-kv-store`](../../sorted-kv-store) is the same ordered reads with
//!   no ownership — anyone may overwrite anything.
//! * [`kv-store-with-shared-storage`](../../kv-store-with-shared-storage) is
//!   ownership by a *writer set* for the whole collection, rather than
//!   per entry.
//!
//! # The shape this collection is for
//!
//! Keys here are `"<topic>/<account>/<seq>"`, and that is the whole idea:
//!
//! * the **topic** is what a reader asks for — `prefix("news/")` is an index
//!   seek, not a scan;
//! * the **account** is in the key, so a row that lies about who wrote it
//!   disagrees with its own owner stamp and any reader can tell for free,
//!   without a metadata lookup per row;
//! * the **seq** makes each write its own key, so no two writers ever contend
//!   for one — and on a shared keyspace an occupied key is a key its owner
//!   holds forever.
//!
//! # Why not just use `AuthoredMap`
//!
//! `AuthoredMap`'s only iteration is `entries()`, which loads every entry in
//! the collection. On an authored collection that is a liveness question and
//! not only a speed one, because the two halves of the ownership model
//! compound: **anyone may insert under any key**, and **only an entry's own
//! owner may ever remove it**. So a member acting in bad faith can grow the
//! collection without bound and nobody else can shrink it — not the app, not
//! the other members, ever. With only a full scan available, every honest
//! reader then pays for that on every read, permanently.
//!
//! The entries never have to be *believed* to do the damage. An app that
//! correctly ignores all of them still reads all of them.
//!
//! `read_topic` below is an index seek, so entries under other topics cost
//! nothing to skip. `flood` and `count_all` exist to demonstrate exactly that,
//! and `workflows/authored-sorted-kv-store.yml` drives the demonstration across
//! two real nodes.

use calimero_sdk::abi::AbiType;
use calimero_sdk::serde::Serialize;
use calimero_sdk::{app, env};
use calimero_storage::collections::{AuthoredSortedMap, LwwRegister};
use thiserror::Error;

#[app::state(emits = for<'a> Event<'a>)]
pub struct AuthoredSortedKvStore {
    /// `"<topic>/<account>/<seq>" -> note`.
    ///
    /// The value is an `LwwRegister` because a replicated collection's values
    /// must themselves be CRDTs — the storage layer merges each entry, and a
    /// bare `String` has no merge rule. It is also the right one here: an edit
    /// replaces the text outright, and only its author can make one.
    notes: AuthoredSortedMap<String, LwwRegister<String>>,
}

#[app::event]
pub enum Event<'a> {
    Posted { key: &'a str },
    Edited { key: &'a str },
    Retracted { key: &'a str },
}

#[derive(Debug, Error, Serialize)]
#[serde(crate = "calimero_sdk::serde")]
#[serde(tag = "kind", content = "data")]
pub enum Error<'a> {
    #[error("no note at {0}")]
    NotFound(&'a str),
    #[error("a `/` in a topic would change which slice this lands in")]
    TopicHasSeparator,
}

/// One note, as a reader sees it.
///
/// `author` is read back from the entry's **owner stamp**, never from the key
/// alone — see [`AuthoredSortedKvStore::read_topic`].
#[derive(Debug, Serialize, AbiType)]
#[serde(crate = "calimero_sdk::serde")]
pub struct Note {
    pub key: String,
    pub author: String,
    pub text: String,
    /// Whether the account named in the key is the account that owns the entry.
    ///
    /// Always `true` for anything written through [`post`](AuthoredSortedKvStore::post),
    /// which builds the key from the caller. It is here because a peer running
    /// patched code can write a row whose key names somebody else — core
    /// refuses to stamp it as theirs, so the two disagree and this says so.
    pub key_matches_owner: bool,
}

fn hex(bytes: [u8; 32]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[app::logic]
impl AuthoredSortedKvStore {
    #[app::init]
    pub fn init() -> AuthoredSortedKvStore {
        AuthoredSortedKvStore {
            notes: AuthoredSortedMap::new(),
        }
    }

    /// The caller's own account, as the contract sees it. A client is never
    /// asked who it is.
    pub fn me(&self) -> app::Result<String> {
        Ok(hex(env::account_id()))
    }

    /// Post a note under `topic`, at a key naming the caller.
    ///
    /// `seq` is the caller's to choose and only ever collides with their own
    /// earlier writes: the key carries their account, so two people posting
    /// "first" to the same topic land on different keys and neither is refused.
    /// That is the property to copy — a key two writers might share is a key
    /// one of them can hold forever, because only its owner may remove it.
    pub fn post(&mut self, topic: String, seq: u64, text: String) -> app::Result<String> {
        if topic.contains('/') {
            app::bail!(Error::TopicHasSeparator);
        }
        let key = format!("{topic}/{}/{seq:08}", hex(env::account_id()));
        self.notes.insert(key.clone(), text.into())?;
        app::emit!(Event::Posted { key: &key });
        Ok(key)
    }

    /// Replace a note. Only its author may call this — enforced locally here
    /// and, authoritatively, at merge on every node that receives the write.
    pub fn edit(&mut self, key: String, text: String) -> app::Result<()> {
        self.notes.update(&key, text.into())?;
        app::emit!(Event::Edited { key: &key });
        Ok(())
    }

    /// Take a note back. Only its author may call this.
    pub fn retract(&mut self, key: String) -> app::Result<String> {
        let Some(text) = self.notes.remove(&key)? else {
            app::bail!(Error::NotFound(&key));
        };
        app::emit!(Event::Retracted { key: &key });
        Ok(text.get().clone())
    }

    pub fn get(&self, key: String) -> app::Result<Option<String>> {
        Ok(self.notes.get(&key)?.map(|v| v.get().clone()))
    }

    /// The account that owns `key`, or `""` if there is no entry there.
    pub fn owner_of(&self, key: String) -> app::Result<String> {
        Ok(self
            .notes
            .owner_of(&key)?
            .map_or_else(String::new, |owner| hex(*owner.as_bytes())))
    }

    /// Every note under `topic`, in key order.
    ///
    /// **An index seek, not a scan.** Notes under other topics are not read, so
    /// they cost nothing — which is what keeps this method usable on a
    /// collection anyone may add to and nobody may prune.
    ///
    /// The author of each note is taken from its owner stamp rather than from
    /// its key. The two agree for anything written through [`post`](Self::post);
    /// comparing them is how a reader spots a row a patched peer wrote under
    /// somebody else's name.
    pub fn read_topic(&self, topic: String) -> app::Result<Vec<Note>> {
        let prefix = format!("{topic}/");
        let mut notes = Vec::new();
        for (key, text) in self.notes.prefix(prefix.as_bytes())? {
            let owner = self
                .notes
                .owner_of(&key)?
                .map_or_else(String::new, |owner| hex(*owner.as_bytes()));
            let named = key
                .strip_prefix(&prefix)
                .and_then(|rest| rest.split('/').next())
                .unwrap_or_default();
            notes.push(Note {
                key_matches_owner: !owner.is_empty() && named == owner,
                author: owner,
                key,
                text: text.get().clone(),
            });
        }
        Ok(notes)
    }

    /// The notes under `topic` that the CALLER wrote.
    ///
    /// Narrowing the prefix by the caller's own account is the cheapest form of
    /// this query: the index seeks straight to their slice, so the work is
    /// proportional to their own notes rather than to the topic's.
    pub fn my_notes(&self, topic: String) -> app::Result<Vec<String>> {
        let prefix = format!("{topic}/{}/", hex(env::account_id()));
        Ok(self
            .notes
            .prefix(prefix.as_bytes())?
            .map(|(key, _)| key)
            .collect())
    }

    /// Total notes in the collection, across every topic.
    ///
    /// Deliberately separate from [`read_topic`](Self::read_topic): a reader
    /// that needs the whole collection asks for the whole collection, and one
    /// that needs a slice does not accidentally pay for it.
    pub fn count_all(&self) -> app::Result<usize> {
        Ok(self.notes.len()?)
    }

    /// Write `n` notes under `topic` in one call.
    ///
    /// A demonstration, not a feature: this is what a member acting in bad
    /// faith does, and nothing in the contract can stop them — insert is open
    /// on a shared keyspace, and nobody but each entry's own author may ever
    /// remove it. The merobox scenario calls this and then re-reads an
    /// unrelated topic to show the slice is unmoved.
    pub fn flood(&mut self, topic: String, from: u64, n: u64) -> app::Result<usize> {
        if topic.contains('/') {
            app::bail!(Error::TopicHasSeparator);
        }
        let me = hex(env::account_id());
        for seq in from..from.saturating_add(n) {
            self.notes
                .insert(format!("{topic}/{me}/{seq:08}"), String::new().into())?;
        }
        Ok(self.notes.len()?)
    }
}

#[cfg(test)]
mod tests {
    use calimero_sdk::testing::TestHost;

    use super::*;

    const ALICE: [u8; 32] = [0xA1; 32];
    const BOB: [u8; 32] = [0xB0; 32];

    fn store() -> TestHost<AuthoredSortedKvStore> {
        TestHost::new(AuthoredSortedKvStore::init)
    }

    #[test]
    fn two_people_post_to_one_topic_without_contending() {
        let mut app = store();
        // The same `seq`, deliberately: on a keyspace without the author in the
        // key this is the collision that wedges one of them out permanently.
        let a = app
            .call_as_account(ALICE, ALICE, |s| {
                s.post("news".into(), 1, "from alice".into())
            })
            .expect("alice posts");
        let b = app
            .call_as_account(BOB, BOB, |s| s.post("news".into(), 1, "from bob".into()))
            .expect("bob posts");
        assert_ne!(a, b);

        app.set_account(ALICE);
        let notes = app.view(|s| s.read_topic("news".into())).expect("read");
        assert_eq!(notes.len(), 2);
        assert!(notes.iter().all(|n| n.key_matches_owner));
    }

    #[test]
    fn only_the_author_may_edit_or_retract() {
        let mut app = store();
        let key = app
            .call_as_account(ALICE, ALICE, |s| s.post("news".into(), 1, "mine".into()))
            .expect("alice posts");

        assert!(app
            .call_as_account(BOB, BOB, |s| s.edit(key.clone(), "not yours".into()))
            .is_err());
        assert!(app
            .call_as_account(BOB, BOB, |s| s.retract(key.clone()))
            .is_err());
        app.set_account(ALICE);
        assert_eq!(
            app.view(|s| s.get(key.clone())).expect("get"),
            Some("mine".to_owned())
        );

        app.call_as_account(ALICE, ALICE, |s| s.edit(key.clone(), "edited".into()))
            .expect("author edits");
        assert_eq!(
            app.call_as_account(ALICE, ALICE, |s| s.retract(key.clone()))
                .expect("author retracts"),
            "edited"
        );
    }

    #[test]
    fn a_topic_read_is_unmoved_by_another_topic() {
        // The property the collection exists for: entries nobody can delete,
        // written under a prefix nobody reads, do not enter another slice.
        let mut app = store();
        let _ = app
            .call_as_account(ALICE, ALICE, |s| s.post("news".into(), 1, "hello".into()))
            .expect("alice posts");
        let total = app
            .call_as_account(BOB, BOB, |s| s.flood("spam".into(), 0, 500))
            .expect("bob floods");
        assert_eq!(total, 501);

        app.set_account(ALICE);
        let notes = app.view(|s| s.read_topic("news".into())).expect("read");
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].text, "hello");
        assert_eq!(app.view(|s| s.count_all()).expect("count"), 501);
    }

    #[test]
    fn my_notes_narrows_to_the_caller() {
        let mut app = store();
        let _ = app.call_as_account(ALICE, ALICE, |s| s.post("news".into(), 1, "a".into()));
        let _ = app.call_as_account(ALICE, ALICE, |s| s.post("news".into(), 2, "a".into()));
        let _ = app.call_as_account(BOB, BOB, |s| s.post("news".into(), 1, "b".into()));

        app.set_account(ALICE);
        assert_eq!(
            app.view(|s| s.my_notes("news".into())).expect("mine").len(),
            2
        );
        app.set_account(BOB);
        assert_eq!(
            app.view(|s| s.my_notes("news".into())).expect("mine").len(),
            1
        );
    }

    #[test]
    fn a_topic_cannot_smuggle_a_separator() {
        // `"a/b"` as a topic would put the note in `a/`'s slice while reading
        // as `a/b`'s — a key that lands somewhere its writer did not name.
        let mut app = store();
        assert!(app
            .call_as_account(ALICE, ALICE, |s| s.post("a/b".into(), 1, "x".into()))
            .is_err());
        assert!(app
            .call_as_account(ALICE, ALICE, |s| s.flood("a/b".into(), 0, 1))
            .is_err());
    }
}
