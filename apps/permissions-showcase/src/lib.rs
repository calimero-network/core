//! A team space that uses every write policy storage enforces, one per field.
//!
//! | field | type | the rule every node holds it to |
//! |---|---|---|
//! | `founding` | `Frozen<Founding>` | written once in `init`; nobody, the founder included, changes or removes it |
//! | `quorum` | `Frozen<u64>` | the same, for a plain number |
//! | `messages` | `WriteOnce<SortedMap<String, Message>>` | the author owns a message; nobody edits or deletes it, the author included |
//! | `announcements` | `ModeratedOnce<UnorderedMap<String, Message>>` | nobody edits one; only a moderator removes one, not even its author |
//! | `pages` | `Authored<UnorderedMap<String, Page>>` | the author edits and deletes a page, and owns the revisions nested in it |
//! | `evidence` | `ContentAddressed<IndexedMap<[u8; 32], Evidence>>` | keyed by the hash of its bytes; nobody changes or removes it |
//!
//! None of the methods here check who is calling before writing. Each write
//! goes straight to the collection, and the refusals the tests and the
//! scenario assert are the storage layer's: raised locally for an honest node,
//! and on apply by every other node for a patched one.
//!
//! # Keys are per owner
//!
//! Under every owning policy here (`WriteOnce`, `ModeratedOnce`, `Authored`)
//! an entry's id derives from its owner and its key, so two accounts using one
//! key hold two independent entries, and a key-only `get`, `contains`,
//! `remove` or `owner_of` acts on the caller's own. So:
//!
//! * posting a message or an announcement under a key someone else holds files
//!   the caller's own, beside theirs; the caller's own key is final;
//! * `channel` and `announcements` list one row per author, each with its
//!   author read from its owner stamp;
//! * `remove_announcement` removes every account's announcement at the id,
//!   naming each owner with `remove_by`;
//! * `page` reads the page of the lowest account holding the id, and
//!   `rename_page`, `add_revision` and `delete_page` act on the caller's own.
//!
//! See `apps/indexed-forum` for the remaining policy, `Moderated<C>`: an
//! entry its author may edit and delete, and a moderator may delete.

use std::collections::BTreeMap;

use calimero_sdk::abi::AbiType;
use calimero_sdk::borsh::{BorshDeserialize, BorshSerialize};
use calimero_sdk::serde::Serialize;
use calimero_sdk::{app, env, AccountId};
use calimero_storage::collections::{
    Authored, ContentAddressed, Frozen, IndexedMap, LwwRegister, ModeratedOnce, SortedMap,
    UnorderedMap, WriteOnce,
};
use thiserror::Error;

/// Fixed at founding.
#[derive(
    BorshSerialize, BorshDeserialize, AbiType, Clone, Debug, Default, PartialEq, Serialize,
)]
#[borsh(crate = "calimero_sdk::borsh")]
#[serde(crate = "calimero_sdk::serde")]
pub struct Founding {
    pub name: String,
    pub motto: String,
}

/// A message or an announcement: plain data, since nothing may change it.
#[derive(BorshSerialize, BorshDeserialize, AbiType, Clone, Debug, PartialEq)]
#[borsh(crate = "calimero_sdk::borsh")]
pub struct Message {
    pub text: String,
    pub sent_at: u64,
}

/// A wiki page. Its revisions are a collection nested in the page's entry, so
/// they are owned by the page's author like the page itself.
#[derive(BorshSerialize, BorshDeserialize, AbiType, app::Mergeable)]
#[borsh(crate = "calimero_sdk::borsh")]
pub struct Page {
    pub title: LwwRegister<String>,
    pub revisions: UnorderedMap<String, LwwRegister<String>>,
}

/// A piece of evidence, found by kind.
#[derive(BorshSerialize, BorshDeserialize, AbiType, Clone, Debug, PartialEq, app::Indexed)]
#[borsh(crate = "calimero_sdk::borsh")]
pub struct Evidence {
    #[index]
    pub kind: String,
    pub body: String,
}

#[app::state(emits = for<'a> Event<'a>)]
pub struct TeamSpace {
    founding: Frozen<Founding>,
    quorum: Frozen<u64>,
    /// `"<channel>/<id>"` -> message.
    messages: WriteOnce<SortedMap<String, Message>>,
    announcements: ModeratedOnce<UnorderedMap<String, Message>>,
    pages: Authored<UnorderedMap<String, Page>>,
    evidence: ContentAddressed<IndexedMap<[u8; 32], Evidence>>,
}

#[app::event]
pub enum Event<'a> {
    MessagePosted { key: &'a str },
    Announced { id: &'a str },
    AnnouncementRemoved { id: &'a str },
    PageChanged { id: &'a str },
    EvidenceFiled { hash: &'a str },
}

#[derive(Debug, Error, Serialize)]
#[serde(crate = "calimero_sdk::serde")]
#[serde(tag = "kind", content = "data")]
pub enum Error<'a> {
    #[error("no page {0}")]
    NoPage(&'a str),
    #[error("a `/` in {0} would move a message into another channel")]
    HasSeparator(&'a str),
    #[error("{0} is not a 64-character hex hash")]
    BadHash(&'a str),
}

/// A message as a client sees it.
#[derive(Debug, Serialize, AbiType)]
#[serde(crate = "calimero_sdk::serde")]
pub struct MessageView {
    pub key: String,
    /// Read back from the entry's owner stamp.
    pub author: String,
    pub text: String,
    pub sent_at: u64,
}

/// A page as a client sees it.
#[derive(Debug, Serialize, AbiType)]
#[serde(crate = "calimero_sdk::serde")]
pub struct PageView {
    pub id: String,
    pub author: String,
    pub title: String,
    /// `(revision id, text)`, by revision id.
    pub revisions: Vec<(String, String)>,
}

/// A piece of evidence as a client sees it.
#[derive(Debug, Serialize, AbiType)]
#[serde(crate = "calimero_sdk::serde")]
pub struct EvidenceView {
    pub hash: String,
    pub kind: String,
    pub body: String,
}

const NAME: &str = "calimero core";
// No commas: merobox splits an assertion's arguments on them.
const MOTTO: &str = "converge without coordinating";
const QUORUM: u64 = 2;

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn account_hex(account: AccountId) -> String {
    hex(account.as_bytes())
}

fn parse_hash(hash: &str) -> Option<[u8; 32]> {
    if hash.len() != 64 {
        return None;
    }
    let mut out = [0; 32];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(hash.get(2 * i..2 * i + 2)?, 16).ok()?;
    }
    Some(out)
}

fn message_key(channel: &str, id: &str) -> String {
    format!("{channel}/{id}")
}

/// Views of `rows`, read across every owner, each with its author.
///
/// A row doesn't say whose it is, and one key has a row per owner holding it,
/// so each row's author is the holder of its key whose message it is.
/// `holders` is the collection's `entries_at`. A holder is used once, so two
/// authors of equal messages at one key are both named.
fn message_views(
    rows: impl IntoIterator<Item = (String, Message)>,
    holders: impl Fn(&String) -> app::Result<Vec<(AccountId, Message)>>,
) -> app::Result<Vec<MessageView>> {
    let mut unclaimed: BTreeMap<String, Vec<(AccountId, Message)>> = BTreeMap::new();
    let mut views = Vec::new();
    for (key, message) in rows {
        if !unclaimed.contains_key(&key) {
            let _ = unclaimed.insert(key.clone(), holders(&key)?);
        }
        let author = unclaimed.get_mut(&key).and_then(|held| {
            let at = held.iter().position(|(_, m)| *m == message)?;
            Some(account_hex(held.remove(at).0))
        });
        views.push(MessageView {
            key,
            author: author.unwrap_or_default(),
            text: message.text,
            sent_at: message.sent_at,
        });
    }
    Ok(views)
}

#[app::logic]
impl TeamSpace {
    #[app::init]
    pub fn init() -> TeamSpace {
        TeamSpace {
            founding: Frozen::new(Founding {
                name: NAME.to_owned(),
                motto: MOTTO.to_owned(),
            }),
            quorum: Frozen::new(QUORUM),
            messages: WriteOnce::new(),
            // The founder is the first moderator.
            announcements: ModeratedOnce::new(),
            pages: Authored::new(),
            evidence: ContentAddressed::new(),
        }
    }

    /// The caller's account, as a writer set or an owner stamp names it.
    pub fn me(&self) -> app::Result<String> {
        Ok(AccountId::from(env::account_id()).to_string())
    }

    // ── frozen values ──────────────────────────────────────────────────────

    /// What the space was founded with. There is no setter.
    pub fn founding(&self) -> app::Result<Founding> {
        Ok(self.founding.get()?.clone())
    }

    pub fn quorum(&self) -> app::Result<u64> {
        Ok(*self.quorum.get()?)
    }

    /// Who froze the founding record: whoever ran `init`.
    pub fn founder(&self) -> app::Result<Option<String>> {
        Ok(self.founding.writer().map(account_hex))
    }

    // ── write-once messages ────────────────────────────────────────────────

    /// Post a message. The caller owns it, and nobody can edit or delete it.
    /// Posting again under a key the caller holds is refused by storage.
    /// Another account's message at the same key is a separate entry, and
    /// neither can touch the other.
    pub fn post_message(
        &mut self,
        channel: String,
        id: String,
        text: String,
        sent_at: u64,
    ) -> app::Result<String> {
        if channel.contains('/') {
            app::bail!(Error::HasSeparator(&channel));
        }
        let key = message_key(&channel, &id);
        self.messages
            .insert(key.clone(), Message { text, sent_at })?;
        app::emit!(Event::MessagePosted { key: &key });
        Ok(key)
    }

    /// A channel's messages, in id order: one prefix slice of the sorted map.
    /// Two authors' messages at one id both appear, each with its author.
    pub fn channel(&self, channel: String) -> app::Result<Vec<MessageView>> {
        let prefix = format!("{channel}/");
        message_views(self.messages.prefix(prefix.as_bytes())?, |key| {
            Ok(self.messages.entries_at(key)?)
        })
    }

    // ── moderated, immutable announcements ─────────────────────────────────

    pub fn announce(&mut self, id: String, text: String, sent_at: u64) -> app::Result<()> {
        self.announcements
            .insert(id.clone(), Message { text, sent_at })?;
        app::emit!(Event::Announced { id: &id });
        Ok(())
    }

    /// Remove the announcement at `id`: every account's, since ids are per
    /// author. Storage lets a moderator do it, and nobody else: not even the
    /// announcement's author. `false` if nobody holds `id`.
    pub fn remove_announcement(&mut self, id: String) -> app::Result<bool> {
        let mut removed = false;
        for (owner, _) in self.announcements.entries_at(&id)? {
            removed |= self.announcements.remove_by(&owner, &id)?.is_some();
        }
        if removed {
            app::emit!(Event::AnnouncementRemoved { id: &id });
        }
        Ok(removed)
    }

    /// Every announcement, by id: one row per author holding the id.
    pub fn announcements(&self) -> app::Result<Vec<MessageView>> {
        let mut views = message_views(self.announcements.entries()?, |key| {
            Ok(self.announcements.entries_at(key)?)
        })?;
        views.sort_by(|a, b| a.key.cmp(&b.key));
        Ok(views)
    }

    pub fn moderators(&self) -> app::Result<Vec<String>> {
        Ok(self
            .announcements
            .moderators()
            .into_iter()
            .map(account_hex)
            .collect())
    }

    /// Add a moderator. Only a current moderator may; storage refuses anyone
    /// else, and every node checks the change against the moderators.
    pub fn add_moderator(&mut self, account: AccountId) -> app::Result<()> {
        let mut moderators = self.announcements.moderators();
        let _ = moderators.insert(account);
        self.announcements.set_moderators(moderators)?;
        Ok(())
    }

    // ── authored pages with nested revisions ───────────────────────────────

    pub fn create_page(&mut self, id: String, title: String, text: String) -> app::Result<()> {
        let mut revisions = UnorderedMap::new();
        let _ = revisions.insert("r1".to_owned(), LwwRegister::new(text))?;
        self.pages.insert(
            id.clone(),
            Page {
                title: LwwRegister::new(title),
                revisions,
            },
        )?;
        app::emit!(Event::PageChanged { id: &id });
        Ok(())
    }

    /// Rename the caller's page at `id`: only its author may, and another
    /// account's page at the id is not the caller's to name.
    pub fn rename_page(&mut self, id: String, title: String) -> app::Result<()> {
        if !self.pages.contains(&id)? {
            app::bail!(Error::NoPage(&id));
        }
        self.pages.modify(&id, |page| page.title.set(title))?;
        app::emit!(Event::PageChanged { id: &id });
        Ok(())
    }

    /// Add a revision to the caller's page at `id`. The revisions live in a
    /// collection nested in the page's entry, and storage holds them to the
    /// page's owner.
    pub fn add_revision(&mut self, id: String, revision: String, text: String) -> app::Result<()> {
        let Some(mut page) = self.pages.get(&id)? else {
            app::bail!(Error::NoPage(&id));
        };
        let _ = page.revisions.insert(revision, LwwRegister::new(text))?;
        app::emit!(Event::PageChanged { id: &id });
        Ok(())
    }

    /// Delete the caller's page at `id`. `false` if the caller holds none,
    /// whoever else does.
    pub fn delete_page(&mut self, id: String) -> app::Result<bool> {
        let removed = self.pages.remove(&id)?.is_some();
        if removed {
            app::emit!(Event::PageChanged { id: &id });
        }
        Ok(removed)
    }

    /// The page at `id`, whoever wrote it: the lowest account's, if several
    /// accounts hold the id. The same pick on every node.
    pub fn page(&self, id: String) -> app::Result<Option<PageView>> {
        let Some((owner, page)) = self
            .pages
            .entries_at(&id)?
            .into_iter()
            .min_by(|(a, _), (b, _)| a.cmp(b))
        else {
            return Ok(None);
        };
        let author = account_hex(owner);
        let mut revisions: Vec<(String, String)> = page
            .revisions
            .entries()?
            .map(|(key, text)| (key, text.get().clone()))
            .collect();
        revisions.sort();
        Ok(Some(PageView {
            id,
            author,
            title: page.title.get().clone(),
            revisions,
        }))
    }

    // ── content-addressed evidence ─────────────────────────────────────────

    /// File a piece of evidence and get back the hash it is stored under.
    /// Filing equal content again returns the same hash and stores nothing.
    pub fn file_evidence(&mut self, kind: String, body: String) -> app::Result<String> {
        let hash = hex(&self.evidence.insert(Evidence { kind, body })?);
        app::emit!(Event::EvidenceFiled { hash: &hash });
        Ok(hash)
    }

    pub fn evidence(&self, hash: String) -> app::Result<Option<EvidenceView>> {
        let Some(key) = parse_hash(&hash) else {
            app::bail!(Error::BadHash(&hash));
        };
        Ok(self.evidence.get(&key)?.map(|e| EvidenceView {
            hash,
            kind: e.kind,
            body: e.body,
        }))
    }

    /// Every piece of evidence of one kind: an index seek, not a scan.
    pub fn evidence_of_kind(&self, kind: String) -> app::Result<Vec<EvidenceView>> {
        let mut views: Vec<EvidenceView> = self
            .evidence
            .query("kind")
            .eq(kind.as_str())
            .entries()?
            .into_iter()
            .map(|(hash, e)| EvidenceView {
                hash: hex(&hash),
                kind: e.kind,
                body: e.body,
            })
            .collect();
        views.sort_by(|a, b| a.body.cmp(&b.body));
        Ok(views)
    }
}

#[cfg(test)]
mod tests {
    use calimero_sdk::testing::TestHost;

    use super::*;

    const ALICE: [u8; 32] = [0xA1; 32];
    const BOB: [u8; 32] = [0xB0; 32];

    fn space() -> TestHost<TeamSpace> {
        TestHost::new(TeamSpace::init)
    }

    fn founder(app: &TestHost<TeamSpace>) -> [u8; 32] {
        let hex = app.view(|s| s.founder()).expect("founder").expect("set");
        parse_hash(&hex).expect("an account is 32 bytes")
    }

    #[test]
    fn the_founding_record_and_quorum_are_frozen_at_init() {
        let app = space();
        assert_eq!(
            app.view(|s| s.founding()).expect("founding"),
            Founding {
                name: NAME.to_owned(),
                motto: MOTTO.to_owned(),
            }
        );
        assert_eq!(app.view(|s| s.quorum()).expect("quorum"), QUORUM);
        let founder = founder(&app);
        assert_eq!(
            app.view(|s| s.moderators()).expect("moderators"),
            [hex(&founder)],
            "whoever ran init froze the record and is the first moderator"
        );
    }

    /// `init` runs as the harness's own account, as it does on a node, so the
    /// founder is whoever `call` runs as by default.
    #[test]
    fn the_harness_account_founds_the_space_and_moderates_it() {
        let mut app = space();
        assert_eq!(founder(&app), app.account_id());
        app.call_as_account(ALICE, ALICE, |s| s.announce("a1".into(), "x".into(), 1))
            .expect("announce");
        assert!(app
            .call(|s| s.remove_announcement("a1".into()))
            .expect("the founder moderates as the default caller"));
    }

    #[test]
    fn a_message_key_is_per_author_and_final() {
        let mut app = space();
        let key = app
            .call_as_account(ALICE, ALICE, |s| {
                s.post_message("general".into(), "m1".into(), "hello".into(), 1)
            })
            .expect("post");
        assert_eq!(key, "general/m1");

        let bobs = app
            .call_as_account(BOB, BOB, |s| {
                s.post_message("general".into(), "m1".into(), "bob's own".into(), 2)
            })
            .expect("the same key is a separate entry for another account");
        assert_eq!(bobs, key);
        assert!(
            app.call_as_account(ALICE, ALICE, |s| {
                s.post_message("general".into(), "m1".into(), "edited".into(), 3)
            })
            .is_err(),
            "its author cannot rewrite it: it was written once"
        );

        let mut general: Vec<_> = app
            .view(|s| s.channel("general".into()))
            .expect("channel")
            .into_iter()
            .map(|m| (m.key, m.author, m.text))
            .collect();
        general.sort();
        let mut expected = vec![
            (key.clone(), hex(&ALICE), "hello".to_owned()),
            (key, hex(&BOB), "bob's own".to_owned()),
        ];
        expected.sort();
        assert_eq!(general, expected, "one row per author, neither touched");
    }

    #[test]
    fn a_channel_is_one_prefix_slice() {
        let mut app = space();
        for (channel, id, who) in [
            ("general", "a", ALICE),
            ("random", "b", BOB),
            ("general", "c", BOB),
        ] {
            let _ = app
                .call_as_account(who, who, |s| {
                    s.post_message(channel.into(), id.into(), format!("{channel} {id}"), 0)
                })
                .expect("post");
        }
        let general = app.view(|s| s.channel("general".into())).expect("channel");
        let keys: Vec<_> = general.iter().map(|m| m.key.as_str()).collect();
        assert_eq!(keys, ["general/a", "general/c"]);
        assert_eq!(general[1].author, hex(&BOB));
        assert!(app
            .call(|s| s.post_message("gen/eral".into(), "x".into(), String::new(), 0))
            .is_err());
    }

    #[test]
    fn only_a_moderator_removes_an_announcement_not_its_author() {
        let mut app = space();
        let founder = founder(&app);
        app.call_as_account(ALICE, ALICE, |s| {
            s.announce("a1".into(), "offsite friday".into(), 1)
        })
        .expect("announce");
        assert!(
            app.call_as_account(ALICE, ALICE, |s| s.announce("a1".into(), "moved".into(), 2))
                .is_err(),
            "an announcement is never edited"
        );
        assert!(
            app.call_as_account(ALICE, ALICE, |s| s.remove_announcement("a1".into()))
                .is_err(),
            "its author is not a moderator"
        );
        assert!(app
            .call_as_account(BOB, BOB, |s| s.remove_announcement("a1".into()))
            .is_err());
        assert!(app
            .call_as_account(founder, founder, |s| s.remove_announcement("a1".into()))
            .expect("the founder moderates"));
        assert!(app.view(|s| s.announcements()).expect("list").is_empty());
    }

    #[test]
    fn a_moderator_removes_every_author_s_announcement_at_an_id() {
        let mut app = space();
        let founder = founder(&app);
        for (who, text) in [(ALICE, "from alice"), (BOB, "from bob")] {
            app.call_as_account(who, who, |s| s.announce("a1".into(), text.into(), 1))
                .expect("announce");
        }
        let authors: Vec<_> = app
            .view(|s| s.announcements())
            .expect("list")
            .into_iter()
            .map(|a| (a.author, a.text))
            .collect();
        assert_eq!(authors.len(), 2);
        assert!(authors.contains(&(hex(&ALICE), "from alice".to_owned())));
        assert!(authors.contains(&(hex(&BOB), "from bob".to_owned())));

        assert!(app
            .call_as_account(founder, founder, |s| s.remove_announcement("a1".into()))
            .expect("the founder moderates"));
        assert!(app.view(|s| s.announcements()).expect("list").is_empty());
        assert!(!app
            .call_as_account(founder, founder, |s| s.remove_announcement("a1".into()))
            .expect("nothing left"));
    }

    #[test]
    fn only_a_moderator_appoints_one() {
        let mut app = space();
        let founder = founder(&app);
        assert!(app
            .call_as_account(BOB, BOB, |s| s.add_moderator(AccountId::from(BOB)))
            .is_err());
        app.call_as_account(founder, founder, |s| s.add_moderator(AccountId::from(BOB)))
            .expect("appoint");
        assert!(app
            .view(|s| s.moderators())
            .expect("moderators")
            .contains(&hex(&BOB)));

        app.call_as_account(ALICE, ALICE, |s| s.announce("a1".into(), "x".into(), 1))
            .expect("announce");
        assert!(app
            .call_as_account(BOB, BOB, |s| s.remove_announcement("a1".into()))
            .expect("an appointed moderator removes"));
    }

    #[test]
    fn a_page_and_its_nested_revisions_belong_to_its_author() {
        let mut app = space();
        app.call_as_account(ALICE, ALICE, |s| {
            s.create_page("home".into(), "Home".into(), "welcome".into())
        })
        .expect("create");
        app.call_as_account(ALICE, ALICE, |s| {
            s.add_revision("home".into(), "r2".into(), "welcome back".into())
        })
        .expect("the author revises");
        app.call_as_account(ALICE, ALICE, |s| {
            s.rename_page("home".into(), "Start".into())
        })
        .expect("the author renames");

        assert!(app
            .call_as_account(BOB, BOB, |s| s.rename_page("home".into(), "Mine".into()))
            .is_err());
        assert!(
            app.call_as_account(BOB, BOB, |s| {
                s.add_revision("home".into(), "r3".into(), "defaced".into())
            })
            .is_err(),
            "Bob holds no page at `home`, so Alice's nested revisions are out of reach"
        );
        assert!(
            !app.call_as_account(BOB, BOB, |s| s.delete_page("home".into()))
                .expect("Bob deletes only his own pages"),
            "Bob holds no page at `home`"
        );

        // Bob reads Alice's page by id alone, and a page of his own at the same
        // id changes neither hers nor which one the id reads: Alice's account
        // is the lower.
        app.call_as_account(BOB, BOB, |s| {
            s.create_page("home".into(), "Bob's".into(), "his".into())
        })
        .expect("Bob files his own page at the id");
        app.call_as_account(BOB, BOB, |s| {
            s.rename_page("home".into(), "Bob's home".into())
        })
        .expect("and renames his own");
        app.set_account(BOB);
        let page = app
            .view(|s| s.page("home".into()))
            .expect("page")
            .expect("home");
        assert_eq!(page.title, "Start");
        assert_eq!(page.author, hex(&ALICE));
        assert_eq!(
            page.revisions,
            [
                ("r1".to_owned(), "welcome".to_owned()),
                ("r2".to_owned(), "welcome back".to_owned())
            ]
        );

        assert!(app
            .call_as_account(ALICE, ALICE, |s| s.delete_page("home".into()))
            .expect("the author deletes"));
        let left = app
            .view(|s| s.page("home".into()))
            .expect("page")
            .expect("Bob's page is left");
        assert_eq!(
            (left.author, left.title),
            (hex(&BOB), "Bob's home".to_owned())
        );
    }

    #[test]
    fn evidence_is_keyed_by_its_content_and_found_by_kind() {
        let mut app = space();
        let log = app
            .call_as_account(ALICE, ALICE, |s| {
                s.file_evidence("log".into(), "boot ok".into())
            })
            .expect("file");
        let again = app
            .call_as_account(BOB, BOB, |s| {
                s.file_evidence("log".into(), "boot ok".into())
            })
            .expect("file");
        assert_eq!(log, again, "equal content, one entry");
        let _ = app
            .call(|s| s.file_evidence("photo".into(), "img".into()))
            .expect("file");
        let _ = app
            .call(|s| s.file_evidence("log".into(), "disk full".into()))
            .expect("file");

        let logs = app
            .view(|s| s.evidence_of_kind("log".into()))
            .expect("query");
        let bodies: Vec<_> = logs.iter().map(|e| e.body.as_str()).collect();
        assert_eq!(bodies, ["boot ok", "disk full"]);
        assert_eq!(
            app.view(|s| s.evidence(log.clone()))
                .expect("get")
                .expect("present")
                .body,
            "boot ok"
        );
        assert!(app.view(|s| s.evidence("zz".into())).is_err());
    }
}
