//! A team wiki: pages named by a slug, each owned by its author, with a sorted tag set,
//! per-person drafts and content-addressed attachments.
//!
//! * **Seeded first state.** `init` runs once, as the app's owner, when the app is created,
//!   and writes the `home` page with the owner as its author. Whatever `init` writes is the
//!   app's first state, so every member's first read already sees it; it emits no event.
//! * **Slugs as keys.** A page lives under the slug of its title (`Slug`, below): titles
//!   that differ only in case or spacing name one page, so `"Getting  Started "` and
//!   `getting-started` reach the same page. The first to create a slug owns it
//!   (`PageExists` for anyone after).
//! * **Author-owned pages. Rule: only a page's author revises, renames, tags, attaches to
//!   or deletes it** (`NotAuthor { slug }` for anyone else, an explicit check against
//!   `env::caller()`). Every revision is kept under `(slug, number)`; renaming changes the
//!   title, never the slug, so links keep working.
//! * **A sorted tag set.** `tags` is a `Set<String>` holding every tag in use; a `Set` keeps
//!   its elements in order and its own count, so autocomplete (`tagsStartingWith`), an A-Z
//!   index slice (`tagsBetween`), paging (`tags`) and the first and last tag read a slice of
//!   the set, never all of it. A tag leaves the set when its last page drops it.
//! * **Drafts. Rule: an account writes only its own draft.** `drafts` is a `PerUser`: the
//!   setters take no account, the value is keyed by the caller's account, so there is no
//!   way to name anybody else's. Anyone may read anyone's draft (`draftOf`) to review it.
//! * **Attachments. Rule: content is stored under its SHA-256 and never changed or
//!   removed**, in a `Frozen` store. Attaching equal content again, to any page, returns the
//!   same hash and stores nothing; deleting a page unlinks its attachments but keeps them.
//!
//! Access: writes need an org member (or above), so guests read but never write.

use calimero_sdk::prelude::*;
use calimero_sdk::StorageError;
use sha2::{Digest, Sha256};

/// A page's name in storage: its title trimmed, lowercased, each run of whitespace one `-`.
///
/// Storage keys are tuples of typed parts (string, int, bytes, bool, none), so a key type
/// must say which part it is by implementing `ToKeyPart` (every `ToKeyPart` is a `ToKey`,
/// the bound `UnorderedMap` and `Table::get` take). A struct that does not is rejected at
/// compile time:
///
/// ```ignore
/// struct Point { x: f64, y: f64 }
/// pages: UnorderedMap<Point, String>   // ERROR: `Point: ToKeyPart` is not satisfied
/// ```
///
/// The fix is a thin newtype that maps itself to a key part. `Slug` wraps a `String` and adds
/// domain meaning while staying a string part, so slugs sort and compare as their strings
/// do. `FromKeyPart` is the way back, which listings in key order (`pages`, `history`,
/// `pagesTagged`) need. The same shape works for an id newtype over `[u8; 32]` (a bytes
/// part) or a `u64` (an int part).
#[derive(Clone, PartialEq, Eq)]
pub struct Slug(String);

impl Slug {
    fn new(title: &str) -> Self {
        Self(
            title
                .split_whitespace()
                .collect::<Vec<_>>()
                .join("-")
                .to_lowercase(),
        )
    }

    fn as_str(&self) -> &str {
        &self.0
    }
}

impl ToKeyPart for Slug {
    fn to_key_part(&self) -> KeyPart {
        KeyPart::Str(self.0.clone())
    }
}

impl FromKeyPart for Slug {
    fn from_key_part(part: &KeyPart) -> Option<Self> {
        String::from_key_part(part).map(Self)
    }
}

/// An attachment's address: the SHA-256 of its UTF-8 bytes, what `sha256sum` prints for the
/// file. The default `CborSha256` hashes the stored CBOR (a header, then the bytes), which
/// nobody outside the app computes; this one lets a reader check content against its link.
pub struct PlainSha256;

impl ContentHash<String> for PlainSha256 {
    fn hash(value: &String) -> Result<[u8; 32], StorageError> {
        Ok(Sha256::digest(value.as_bytes()).into())
    }
}

/// A page as stored under its slug; the bodies are in `revisions`.
#[derive(serde::Serialize, serde::Deserialize)]
struct Page {
    title: String,
    author: String,
    /// The latest revision's number; revisions run from 1.
    revision: u32,
    /// Sorted, as `tagged` holds them.
    tags: Vec<String>,
    /// Attachment hashes, in the order they were attached.
    attachments: Vec<String>,
}

/// A page as a client sees it: the latest revision's body.
#[derive(serde::Serialize, AbiType)]
#[serde(rename_all = "camelCase")]
pub struct PageView {
    pub slug: String,
    pub title: String,
    pub author: String,
    pub revision: u32,
    pub body: String,
    pub tags: Vec<String>,
    pub attachments: Vec<String>,
}

/// One line of the page list.
#[derive(serde::Serialize, AbiType)]
pub struct PageSummary {
    pub slug: String,
    pub title: String,
    pub author: String,
}

/// One revision of a page.
#[derive(serde::Serialize, AbiType)]
pub struct Revision {
    pub revision: u32,
    pub body: String,
}

/// The tag index's header: how many tags, and the first and last in order.
#[derive(serde::Serialize, AbiType)]
pub struct TagStats {
    #[serde(with = "calimero_sdk::json::u64_str")]
    pub count: u64,
    pub first: Option<String>,
    pub last: Option<String>,
}

/// A person's unpublished page.
#[derive(serde::Serialize, serde::Deserialize, AbiType)]
pub struct Draft {
    pub title: String,
    pub body: String,
}

#[app::state(version = 1)]
pub struct Wiki {
    /// Every page, by slug: `Slug: ToKeyPart`, so every map operation takes it.
    pages: UnorderedMap<Slug, Page>,
    /// Each revision's body under `(slug, number)`.
    revisions: UnorderedMap<(Slug, u32), String>,
    /// Every tag some page carries, in order.
    tags: Set<String>,
    /// `(tag, slug)` for each tag of each page: the pages of a tag are one prefix.
    tagged: Set<(String, Slug)>,
    /// Each account's one draft.
    drafts: PerUser<Draft>,
    /// Attachment content under its SHA-256.
    attachments: Frozen<String, PlainSha256>,
}

#[app::error]
pub enum Error {
    /// `title` has no characters a slug keeps, or its slug is over `MAX_SLUG_BYTES`.
    BadTitle { title: String },
    /// Someone already created the page `slug`.
    PageExists { slug: String },
    /// There is no page `slug`.
    NoPage { slug: String },
    /// Only the page's author may do this.
    NotAuthor { slug: String },
    /// A tag is empty once trimmed, or over `MAX_TAG_BYTES`.
    BadTag { tag: String },
    /// The caller has no draft.
    NoDraft,
    /// `hash` is not 64 hex digits.
    BadHash { hash: String },
    /// Nothing is stored under `hash`.
    NoAttachment { hash: String },
}

#[app::event]
pub enum Event {
    /// The page was created, revised, renamed, tagged, untagged, given an attachment or
    /// deleted: read it again. A write that changes nothing emits nothing.
    PageChanged { slug: String },
    /// New content stored; equal content attached again emits nothing.
    AttachmentStored { hash: String },
}

const HOME_TITLE: &str = "Home";
const HOME_BODY: &str =
    "Welcome to the team wiki. Anyone in the org may start a page; only its author changes it.";
/// The longest slug kept; a key part this long still leaves room in the 1 KiB key.
const MAX_SLUG_BYTES: usize = 200;
/// The longest tag kept.
const MAX_TAG_BYTES: usize = 64;
/// The most rows one listing returns.
const MAX_ROWS: u32 = 1000;
/// The most suggestions `tagsStartingWith` returns.
const SUGGESTIONS: u32 = 10;

/// The caller's account (guests included).
fn caller() -> app::Result<String> {
    match env::caller() {
        Caller::Account(a) | Caller::ApiKey { account: a, .. } => Ok(a),
        _ => Err(AppError::login_required()),
    }
}

/// `title`'s slug; `BadTitle` when it is empty or too long.
fn slug_of(title: &str) -> app::Result<Slug> {
    let slug = Slug::new(title);
    if slug.as_str().is_empty() || slug.as_str().len() > MAX_SLUG_BYTES {
        app::bail!(Error::BadTitle {
            title: title.to_owned()
        });
    }
    Ok(slug)
}

/// `tag` trimmed and lowercased; `BadTag` when it is empty or too long.
fn tag_of(tag: &str) -> app::Result<String> {
    let clean = tag.trim().to_lowercase();
    if clean.is_empty() || clean.len() > MAX_TAG_BYTES {
        app::bail!(Error::BadTag {
            tag: tag.to_owned()
        });
    }
    Ok(clean)
}

fn changed(slug: &Slug) -> app::Result<()> {
    env::emit(
        Event::PageChanged {
            slug: slug.0.clone(),
        },
        Audience::All,
    )?;
    Ok(())
}

#[app::logic]
impl Wiki {
    /// Seeds the `home` page, authored by the app owner (`init` runs as the owner).
    #[app::init]
    pub fn init() -> app::Result<Self> {
        let mut wiki = Self::load();
        let slug = Slug::new(HOME_TITLE);
        wiki.write_page(&slug, &env::app_owner(), HOME_TITLE, HOME_BODY)?;
        Ok(wiki)
    }

    /// The caller's account, as `author` fields and `draftOf` name it.
    #[app::view]
    pub fn me(&self) -> app::Result<String> {
        caller()
    }

    // ── pages ──────────────────────────────────────────────────────────────

    /// Creates the page `title` names, as revision 1, and returns its slug. `PageExists`
    /// when the slug is taken, whoever took it.
    pub fn create_page(&mut self, title: String, body: String) -> app::Result<String> {
        let author = guards::require_member()?;
        let slug = slug_of(&title)?;
        if self.pages.contains(&slug)? {
            app::bail!(Error::PageExists { slug: slug.0 });
        }
        self.write_page(&slug, &author, &title, &body)?;
        changed(&slug)?;
        Ok(slug.0)
    }

    /// Adds a revision with `body` to the caller's page and returns its number.
    pub fn revise_page(&mut self, slug: String, body: String) -> app::Result<u32> {
        let (slug, page) = self.own_page(&slug)?;
        self.revise(slug, page, body)
    }

    /// Retitles the caller's page; the slug stays, so links to it keep working.
    pub fn rename_page(&mut self, slug: String, title: String) -> app::Result<()> {
        let (slug, mut page) = self.own_page(&slug)?;
        page.title = title;
        let _old = self.pages.insert(&slug, &page)?;
        changed(&slug)
    }

    /// Deletes the caller's page with its revisions and tags; `false` when there is no page.
    /// Its attachments stay stored: content is never removed.
    pub fn delete_page(&mut self, slug: String) -> app::Result<bool> {
        let _member = guards::require_member()?;
        if !self.pages.contains(&Slug::new(&slug))? {
            return Ok(false);
        }
        let (slug, page) = self.own_page(&slug)?;
        for tag in &page.tags {
            self.drop_tag(&slug, tag)?;
        }
        for revision in 1..=page.revision {
            let _body = self.revisions.remove(&(slug.clone(), revision))?;
        }
        let _page = self.pages.remove(&slug)?;
        changed(&slug)?;
        Ok(true)
    }

    /// The page `slug` (any spelling of its title) at its latest revision, or `null`.
    #[app::view]
    pub fn page(&self, slug: String) -> app::Result<Option<PageView>> {
        let slug = Slug::new(&slug);
        let Some(page) = self.pages.get(&slug)? else {
            return Ok(None);
        };
        let body = self
            .revisions
            .get(&(slug.clone(), page.revision))?
            .unwrap_or_default();
        Ok(Some(PageView {
            slug: slug.0,
            title: page.title,
            author: page.author,
            revision: page.revision,
            body,
            tags: page.tags,
            attachments: page.attachments,
        }))
    }

    /// Every revision of `slug`, oldest first (at most `MAX_ROWS`): one prefix of `revisions`.
    #[app::view]
    pub fn history(&self, slug: String) -> app::Result<Vec<Revision>> {
        let prefix = [Slug::new(&slug).to_key_part()];
        let page = self
            .revisions
            .prefix(&prefix, FindOpts::default().limit(MAX_ROWS))?;
        Ok(page
            .items
            .into_iter()
            .map(|((_, revision), body)| Revision { revision, body })
            .collect())
    }

    /// The pages in slug order (at most `MAX_ROWS`).
    #[app::view]
    pub fn pages(&self) -> app::Result<Vec<PageSummary>> {
        let page = self.pages.page(FindOpts::default().limit(MAX_ROWS))?;
        Ok(page
            .items
            .into_iter()
            .map(|(slug, p)| PageSummary {
                slug: slug.0,
                title: p.title,
                author: p.author,
            })
            .collect())
    }

    // ── tags ───────────────────────────────────────────────────────────────

    /// Tags the caller's page; `true` when the tag is new to it (`PageChanged` only then).
    pub fn tag_page(&mut self, slug: String, tag: String) -> app::Result<bool> {
        let (slug, mut page) = self.own_page(&slug)?;
        let tag = tag_of(&tag)?;
        if !self.tagged.insert(&(tag.clone(), slug.clone()))? {
            return Ok(false);
        }
        let _new = self.tags.insert(&tag)?;
        page.tags.push(tag);
        page.tags.sort();
        let _old = self.pages.insert(&slug, &page)?;
        changed(&slug)?;
        Ok(true)
    }

    /// Drops a tag from the caller's page; `true` when the page had it (`PageChanged`
    /// only then). The tag leaves the tag set with its last page.
    pub fn untag_page(&mut self, slug: String, tag: String) -> app::Result<bool> {
        let (slug, mut page) = self.own_page(&slug)?;
        let tag = tag_of(&tag)?;
        let Some(at) = page.tags.iter().position(|t| *t == tag) else {
            return Ok(false);
        };
        let _tag = page.tags.remove(at);
        self.drop_tag(&slug, &tag)?;
        let _old = self.pages.insert(&slug, &page)?;
        changed(&slug)?;
        Ok(true)
    }

    /// The slugs of the pages tagged `tag`, in order: one prefix of `tagged`.
    #[app::view]
    pub fn pages_tagged(&self, tag: String) -> app::Result<Vec<String>> {
        let prefix = [tag.trim().to_lowercase().to_key_part()];
        let page = self
            .tagged
            .prefix(&prefix, FindOpts::default().limit(MAX_ROWS))?;
        Ok(page.items.into_iter().map(|(_, slug)| slug.0).collect())
    }

    /// Up to `SUGGESTIONS` tags starting with `prefix`, in order (autocomplete): the read
    /// covers only the tags under it.
    #[app::view]
    pub fn tags_starting_with(&self, prefix: String) -> app::Result<Vec<String>> {
        let prefix = [prefix.trim().to_lowercase().to_key_part()];
        let page = self
            .tags
            .starts_with(&prefix, FindOpts::default().limit(SUGGESTIONS))?;
        Ok(page.items)
    }

    /// Tags within `[start, end)` (an index slice such as `a` to `n`): the set's own bounds,
    /// no full scan.
    #[app::view]
    pub fn tags_between(&self, start: String, end: String) -> app::Result<Vec<String>> {
        if start >= end {
            return Ok(Vec::new());
        }
        let page = self
            .tags
            .range(&start, &end, FindOpts::default().limit(MAX_ROWS))?;
        Ok(page.items)
    }

    /// `limit` tags after the first `offset`, in order; empty once `offset` reaches the end.
    /// The skipped tags are still read; to walk a large set, prefer `tagsBetween` from the
    /// last tag held.
    #[app::view]
    pub fn tags(&self, offset: u32, limit: u32) -> app::Result<Vec<String>> {
        // `?` on every tag, the skipped ones too: a failed read is never skipped over.
        let end = offset as usize + limit.min(MAX_ROWS) as usize;
        let mut out = Vec::new();
        for (i, tag) in self.tags.iter().enumerate().take(end) {
            let tag = tag?;
            if i >= offset as usize {
                out.push(tag);
            }
        }
        Ok(out)
    }

    /// How many tags there are and the first and last, read from the set's count and ends.
    #[app::view]
    pub fn tag_stats(&self) -> app::Result<TagStats> {
        Ok(TagStats {
            count: self.tags.len()?,
            first: self.tags.first()?,
            last: self.tags.last()?,
        })
    }

    // ── drafts: each account writes only its own ─────────────────────────

    /// Saves the caller's draft, replacing the one they had.
    pub fn save_draft(&mut self, title: String, body: String) -> app::Result<()> {
        let _member = guards::require_member()?;
        let _slug = slug_of(&title)?;
        let _account = self.drafts.set(&Draft { title, body })?;
        Ok(())
    }

    /// The caller's own draft.
    #[app::view]
    pub fn my_draft(&self) -> app::Result<Option<Draft>> {
        self.drafts.get()
    }

    /// `account`'s draft (an account as `me` shows it), for review.
    #[app::view]
    pub fn draft_of(&self, account: String) -> app::Result<Option<Draft>> {
        Ok(self.drafts.get_for(&account)?)
    }

    /// Drops the caller's draft; `false` when there was none.
    pub fn discard_draft(&mut self) -> app::Result<bool> {
        let _member = guards::require_member()?;
        Ok(self.drafts.remove()?.is_some())
    }

    /// Publishes the caller's draft and returns the page's slug: a new page, or a new
    /// revision (and the draft's title) of the caller's own page. The draft is then gone.
    pub fn publish_draft(&mut self) -> app::Result<String> {
        let author = guards::require_member()?;
        let Some(draft) = self.drafts.get()? else {
            app::bail!(Error::NoDraft);
        };
        let slug = slug_of(&draft.title)?;
        if self.pages.contains(&slug)? {
            let (slug, mut page) = self.own_page(slug.as_str())?;
            page.title = draft.title;
            let _revision = self.revise(slug, page, draft.body)?;
        } else {
            self.write_page(&slug, &author, &draft.title, &draft.body)?;
            changed(&slug)?;
        }
        let _draft = self.drafts.remove()?;
        Ok(slug.0)
    }

    // ── attachments: content-addressed, never changed ─────────────────────

    /// Stores `content` under its SHA-256, links it to the caller's page and returns the
    /// hash. Equal content again, to this page or another, stores nothing and returns the
    /// same hash.
    pub fn attach(&mut self, slug: String, content: String) -> app::Result<String> {
        let (slug, mut page) = self.own_page(&slug)?;
        let (hash, fresh) = self.attachments.insert(&content)?;
        if fresh {
            env::emit(
                Event::AttachmentStored { hash: hash.clone() },
                Audience::All,
            )?;
        }
        if !page.attachments.contains(&hash) {
            page.attachments.push(hash.clone());
            let _old = self.pages.insert(&slug, &page)?;
            changed(&slug)?;
        }
        Ok(hash)
    }

    /// The content under `hash`; `BadHash` unless it is 64 hex digits.
    #[app::view]
    pub fn attachment(&self, hash: String) -> app::Result<String> {
        if hash.len() != 64 || !hash.bytes().all(|b| b.is_ascii_hexdigit()) {
            app::bail!(Error::BadHash { hash });
        }
        let hash = hash.to_ascii_lowercase();
        match self.attachments.get(&hash)? {
            Some(content) => Ok(content),
            None => Err(Error::NoAttachment { hash }.into()),
        }
    }

    // ── helpers ────────────────────────────────────────────────────────────

    /// The caller's page `slug`: `NoPage` when there is none, `NotAuthor` when it is
    /// someone else's. Every write to a page goes through here.
    fn own_page(&self, slug: &str) -> app::Result<(Slug, Page)> {
        let me = guards::require_member()?;
        let slug = Slug::new(slug);
        let Some(page) = self.pages.get(&slug)? else {
            app::bail!(Error::NoPage { slug: slug.0 });
        };
        if page.author != me {
            app::bail!(Error::NotAuthor { slug: slug.0 });
        }
        Ok((slug, page))
    }

    /// Writes a new page as revision 1.
    fn write_page(
        &mut self,
        slug: &Slug,
        author: &str,
        title: &str,
        body: &str,
    ) -> app::Result<()> {
        let page = Page {
            title: title.to_owned(),
            author: author.to_owned(),
            revision: 1,
            tags: Vec::new(),
            attachments: Vec::new(),
        };
        let _old = self.pages.insert(slug, &page)?;
        let _old = self
            .revisions
            .insert(&(slug.clone(), 1), &body.to_owned())?;
        Ok(())
    }

    /// Stores `body` as `page`'s next revision and returns its number.
    fn revise(&mut self, slug: Slug, mut page: Page, body: String) -> app::Result<u32> {
        page.revision += 1;
        let revision = page.revision;
        let _old = self.revisions.insert(&(slug.clone(), revision), &body)?;
        let _old = self.pages.insert(&slug, &page)?;
        changed(&slug)?;
        Ok(revision)
    }

    /// Removes `(tag, slug)` from the index, and `tag` from the tag set when no page is left
    /// under it.
    fn drop_tag(&mut self, slug: &Slug, tag: &str) -> app::Result<()> {
        let _held = self.tagged.remove(&(tag.to_owned(), slug.clone()))?;
        let left = self
            .tagged
            .prefix(&[tag.to_key_part()], FindOpts::default().limit(1))?;
        if left.items.is_empty() {
            let _was = self.tags.remove(&tag.to_owned())?;
        }
        Ok(())
    }
}
