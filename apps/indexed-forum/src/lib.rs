//! A forum built from `Moderated<IndexedMap>` for posts, [`AuthoredSortedMap`]
//! for comments, an [`AuthoredMap`] of votes and a `Frozen<String>` charter.
//! Every list view is a seek, and each field covers what the others can't.
//!
//! Read [`indexed-issue-tracker`](../../indexed-issue-tracker) first. It covers
//! `IndexedMap` on its own. This app shows the harder shapes and how
//! `IndexedMap` combines with the other collections.
//!
//! # Posts: `Moderated<IndexedMap>`, with compound, multi-valued and optional indexes
//!
//! `Authored<C>` puts an owner stamp on every entry of any keyed collection
//! `C`, and leaves reads to `C`. Here `C` is an `IndexedMap`, so a post is
//! owned like a comment and found like an issue in the tracker.
//!
//! * `board_feed` is `(board, created_at)`, so a board's newest page is a
//!   reverse seek, and "what is new since I last looked" is a range after one
//!   timestamp.
//! * `board_tag_feed` is `(board, tags, created_at)`. `tags` holds several
//!   values, so a post gets one row per tag. Each row starts with the board and
//!   ends with the time, which makes "newest `rust` posts in `dev`" a single
//!   seek. A post filed under `rust` in another board is never read.
//! * `board_pinned` is `(board, pinned_at)`. An unpinned post has
//!   `pinned_at = None`, which puts no row in the index. So the pinned strip
//!   costs what it shows, however many posts the board holds.
//! * `author_feed` is `(author, created_at)`: someone's posts, newest first.
//!
//! Board, author and pin counts are index rows counted. No post is loaded.
//!
//! # A post id names its author
//!
//! Storage keys an owned entry by its owner AND its key, so two accounts can
//! each hold an entry at one key. A forum that let the client pick a bare id
//! (`p1`) would have to choose which of them a read by id means, and any rule
//! it picks ("the lowest account", "the earliest `created_at`") is one a
//! member can win on purpose: grind an account, or backdate a write.
//!
//! So `create_post` mints the id: `"<author account hex>-<the caller's id>"`,
//! and returns it. Every read by id (`get_post`, `vote`, `comment`,
//! `comments`) reads only the entry of the account the id names, with
//! `get_by`. A patched peer can still file an entry of its own at someone
//! else's id, but no read by id ever sees it, and the feeds drop it: a row
//! counts only when its owner stamp is the account its id names.
//!
//! # Comments: `AuthoredSortedMap`, keyed so one post's thread is one slice
//!
//! A comment's key is `"<post>/<created_at>/<account>/<id>"`. `prefix("<post>/")`
//! is a seek, and the zero-padded time orders the slice. The storage layer
//! gates edits and removals on every node, so only a comment's author can
//! change it. A patched peer can't get around that.
//!
//! # Votes: an `AuthoredMap` keyed by post, not a number on the post
//!
//! A score kept on the post would be a register. Two people voting at once
//! would each write `score + 1`, and one vote would be lost. A plain set of
//! voter accounts would keep both, but anyone could add anyone's account to
//! it, or take one out.
//!
//! Keys are per owner, so a vote is the voter's own entry at the post's id:
//! one entry per account per post, written and removed only by that account,
//! on every node. A post's score is the number of entries at its id, read in
//! one bucket with `entries_at`. The score isn't a field of the post, so it
//! can't be an index key, and this app has no "top posts" index.
//!
//! # Where the gate is enforced
//!
//! By storage, on every node, for posts, comments and votes alike. Each is
//! stamped with the account that wrote it, and a node applying a peer's write
//! refuses an edit or removal by anyone else, including a peer running patched
//! code.
//! The checks in `edit_post` and the rest only turn that refusal into a
//! readable error before anything is written.
//!
//! A post also carries its author as a field, because an index key must come
//! from the value. The stamp is the truth: views show the stamp, and
//! `author_feed` drops any row whose field disagrees with it, which is what a
//! patched peer writing a post under someone else's name would produce.
//!
//! # What syncs
//!
//! Only the entries. The post indexes and the comment key order are both
//! node-local, and votes need no index. A change from a peer lands without
//! the indexes knowing, so the first post query after it rebuilds them.
//! `workflows/indexed-forum.yml` runs that across two nodes.

use core::ops::Bound;

use calimero_sdk::abi::AbiType;
use calimero_sdk::borsh::{BorshDeserialize, BorshSerialize};
use calimero_sdk::serde::Serialize;
use calimero_sdk::{app, env, AccountId};
use calimero_storage::collections::{
    AuthoredMap, AuthoredSortedMap, Frozen, IndexedMap, LwwRegister, Moderated,
};
use thiserror::Error;

/// One post, and what it is looked up by.
///
/// A post is one map entry, and concurrent writes to one entry resolve
/// last-write-wins on the whole entry: a retag and a pin made at once on two
/// devices keep one of the two. Only the author writes a post, so that race is
/// between one person's own devices.
#[derive(BorshSerialize, BorshDeserialize, AbiType, app::Mergeable, app::Indexed)]
#[borsh(crate = "calimero_sdk::borsh")]
#[index(board_feed(board, created_at))]
#[index(board_tag_feed(board, tags, created_at))]
#[index(board_pinned(board, pinned_at))]
#[index(author_feed(author, created_at))]
pub struct Post {
    pub board: LwwRegister<String>,
    /// The author's account, hex-encoded.
    pub author: LwwRegister<String>,
    /// One `board_tag_feed` row per tag.
    pub tags: LwwRegister<Vec<String>>,
    /// `None` leaves the post out of `board_pinned` entirely.
    pub pinned_at: LwwRegister<Option<u64>>,
    pub created_at: LwwRegister<u64>,
    pub title: LwwRegister<String>,
    pub body: LwwRegister<String>,
}

#[app::state(emits = for<'a> Event<'a>)]
pub struct Forum {
    /// The founder's charter: written once in `init`, changeable by nobody.
    charter: Frozen<String>,
    /// `"<author>-<id>"` -> post. The author edits and deletes; a moderator
    /// removes.
    posts: Moderated<IndexedMap<String, Post>>,
    /// `"<post>/<created_at:020>/<account>/<id>"` -> comment body.
    comments: AuthoredSortedMap<String, LwwRegister<String>>,
    /// Post id -> one entry per account that upvoted it, each its voter's own.
    votes: AuthoredMap<String, LwwRegister<bool>>,
}

#[app::event]
pub enum Event<'a> {
    Posted { id: &'a str, board: &'a str },
    PostChanged { id: &'a str },
    PostDeleted { id: &'a str },
    Commented { post: &'a str, key: &'a str },
    Voted { post: &'a str },
}

#[derive(Debug, Error, Serialize)]
#[serde(crate = "calimero_sdk::serde")]
#[serde(tag = "kind", content = "data")]
pub enum Error<'a> {
    #[error("post {0} already exists")]
    Exists(&'a str),
    #[error("no post {0}")]
    NoPost(&'a str),
    #[error("no comment at {0}")]
    NoComment(&'a str),
    #[error("only the author of post {0} may change it")]
    NotAuthor(&'a str),
    #[error("only a moderator may remove post {0}")]
    NotModerator(&'a str),
    #[error("a `/` in {0} would move a comment into another post's thread")]
    HasSeparator(&'a str),
}

/// A post as a client sees it.
#[derive(Debug, Serialize, AbiType)]
#[serde(crate = "calimero_sdk::serde")]
pub struct PostView {
    pub id: String,
    pub board: String,
    pub author: String,
    pub title: String,
    pub body: String,
    pub tags: Vec<String>,
    pub created_at: u64,
    pub pinned: bool,
    pub votes: u64,
    pub comments: u64,
    pub voted_by_me: bool,
}

/// A comment as a client sees it.
#[derive(Debug, Serialize, AbiType)]
#[serde(crate = "calimero_sdk::serde")]
pub struct CommentView {
    pub key: String,
    /// Read back from the entry's owner stamp, not from the key.
    pub author: String,
    pub body: String,
    pub created_at: u64,
}

/// A board's size, read from index rows only.
#[derive(Debug, Serialize, AbiType)]
#[serde(crate = "calimero_sdk::serde")]
pub struct BoardStats {
    pub posts: u64,
    pub pinned: u64,
}

fn hex(bytes: [u8; 32]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn caller() -> String {
    hex(env::account_id())
}

/// The account a post id names: the 64 hex characters before its first `-`.
fn named_account(id: &str) -> Option<AccountId> {
    let (named, _) = id.split_once('-')?;
    if named.len() != 64 {
        return None;
    }
    let mut bytes = [0; 32];
    for (i, byte) in bytes.iter_mut().enumerate() {
        *byte = u8::from_str_radix(named.get(2 * i..2 * i + 2)?, 16).ok()?;
    }
    Some(AccountId::from(bytes))
}

/// The account whose entry, among `holders` of one key, is `row`.
///
/// A key is per owner, so an ordered read or an index query hands back rows
/// without saying whose each is. Two owners' entries are told apart by their
/// bytes: every value here is built from `LwwRegister`s, which carry the
/// write's timestamp and writer, so no two entries are byte-identical.
fn stamp_of<V: BorshSerialize>(holders: Vec<(AccountId, V)>, row: &V) -> Option<AccountId> {
    let row = calimero_sdk::borsh::to_vec(row).ok()?;
    holders
        .into_iter()
        .find(|(_, held)| calimero_sdk::borsh::to_vec(held).is_ok_and(|bytes| bytes == row))
        .map(|(owner, _)| owner)
}

/// Frozen at `init`: no node lets anyone change it afterwards.
const CHARTER: &str = "be kind and stay on topic";

fn thread_prefix(post: &str) -> String {
    format!("{post}/")
}

#[app::logic]
impl Forum {
    #[app::init]
    pub fn init() -> Forum {
        Forum {
            charter: Frozen::new(CHARTER.to_owned()),
            // The founder is the first moderator.
            posts: Moderated::new(),
            comments: AuthoredSortedMap::new(),
            votes: AuthoredMap::new(),
        }
    }

    /// The caller's own account, as the contract sees it.
    pub fn me(&self) -> app::Result<String> {
        Ok(caller())
    }

    /// The charter the forum was founded with.
    pub fn charter(&self) -> app::Result<String> {
        Ok(self.charter.get()?.clone())
    }

    /// Who may remove any post.
    pub fn moderators(&self) -> app::Result<Vec<String>> {
        Ok(self
            .posts
            .moderators()
            .into_iter()
            .map(|account| hex(*account.as_bytes()))
            .collect())
    }

    /// Remove the post at `id` as a moderator: its author's, and any entry a
    /// patched peer filed at the same id. Every node checks the remover against
    /// the moderators as of the removal.
    ///
    /// Its votes and comments stay, each its own author's to remove; every
    /// read of them goes through a live post, so they are never shown.
    pub fn moderate_post(&mut self, id: String) -> app::Result<()> {
        let holders = self.posts.entries_at(&id)?;
        if holders.is_empty() {
            app::bail!(Error::NoPost(&id));
        }
        let me = AccountId::from(env::account_id());
        if !self.posts.is_moderator(&me) {
            app::bail!(Error::NotModerator(&id));
        }
        for (owner, _) in holders {
            let _ = self.posts.remove_by(&owner, &id)?;
        }
        app::emit!(Event::PostDeleted { id: &id });
        Ok(())
    }

    // ── posts ──────────────────────────────────────────────────────────────

    /// File a post and return its id, `"<caller's account>-<id>"`. `id` is
    /// the caller's to choose and only ever collides with their own posts.
    ///
    /// `created_at` is the client's clock: it orders the post in every feed it
    /// appears in, and nothing else. A writer can put any number there, so it
    /// decides no winner.
    pub fn create_post(
        &mut self,
        id: String,
        board: String,
        title: String,
        body: String,
        tags: Vec<String>,
        created_at: u64,
    ) -> app::Result<String> {
        if id.contains('/') {
            app::bail!(Error::HasSeparator(&id));
        }
        let id = format!("{}-{id}", caller());
        if self.posts.contains(&id)? {
            app::bail!(Error::Exists(&id));
        }
        let post = Post {
            board: LwwRegister::new(board.clone()),
            author: LwwRegister::new(caller()),
            tags: LwwRegister::new(tags),
            pinned_at: LwwRegister::new(None),
            created_at: LwwRegister::new(created_at),
            title: LwwRegister::new(title),
            body: LwwRegister::new(body),
        };
        self.posts.insert(id.clone(), post)?;
        app::emit!(Event::Posted {
            id: &id,
            board: &board
        });
        Ok(id)
    }

    pub fn edit_post(&mut self, id: String, title: String, body: String) -> app::Result<()> {
        self.change_own_post(&id, |post| {
            post.title.set(title);
            post.body.set(body);
        })
    }

    /// Replace a post's tags. It leaves the old tags' feeds and joins the new
    /// ones: `modify` writes only the index rows that differ.
    pub fn retag(&mut self, id: String, tags: Vec<String>) -> app::Result<()> {
        self.change_own_post(&id, |post| post.tags.set(tags))
    }

    /// Pin a post to the top of its board, ordered by when it was pinned.
    pub fn pin(&mut self, id: String, at: u64) -> app::Result<()> {
        self.change_own_post(&id, |post| post.pinned_at.set(Some(at)))
    }

    /// Unpin a post. `None` removes its `board_pinned` row.
    pub fn unpin(&mut self, id: String) -> app::Result<()> {
        self.change_own_post(&id, |post| post.pinned_at.set(None))
    }

    /// Delete the caller's post at `id`.
    ///
    /// Its votes and comments stay: each belongs to its own author, and only
    /// they may remove it. Every read of them goes through a live post, so the
    /// orphans are never shown. Filing the same id again brings them back, so
    /// a re-post takes a new id.
    pub fn delete_post(&mut self, id: String) -> app::Result<()> {
        self.check_author(&id)?;
        let _ = self.posts.remove(&id)?;
        app::emit!(Event::PostDeleted { id: &id });
        Ok(())
    }

    /// The post at `id`: the entry of the account the id names, and no other.
    pub fn get_post(&self, id: String) -> app::Result<Option<PostView>> {
        let Some((author, post)) = self.holder(&id)? else {
            return Ok(None);
        };
        Ok(Some(self.view_of(id, Some(author), &post)?))
    }

    /// One page of a board, newest first.
    pub fn board_feed(
        &self,
        board: String,
        offset: usize,
        limit: usize,
    ) -> app::Result<Vec<PostView>> {
        let page = self
            .posts
            .query("board_feed")
            .eq(&board)
            .desc()
            .skip(offset)
            .limit(limit)
            .entries()?;
        self.views(page)
    }

    /// One page of a board's posts carrying `tag`, newest first.
    ///
    /// The board and the tag are both pinned, so this reads only the rows it
    /// returns, even though the tag is one element of a list.
    pub fn board_tag_feed(
        &self,
        board: String,
        tag: String,
        offset: usize,
        limit: usize,
    ) -> app::Result<Vec<PostView>> {
        let page = self
            .posts
            .query("board_tag_feed")
            .eq(&board)
            .eq(&tag)
            .desc()
            .skip(offset)
            .limit(limit)
            .entries()?;
        self.views(page)
    }

    /// A board's posts created after `after`, oldest first: what a client
    /// polls for new posts.
    pub fn board_since(&self, board: String, after: u64) -> app::Result<Vec<PostView>> {
        let fresh = self
            .posts
            .query("board_feed")
            .eq(&board)
            .range((Bound::Excluded(after), Bound::Unbounded))
            .entries()?;
        self.views(fresh)
    }

    /// A board's pinned posts, most recently pinned first.
    pub fn pinned(&self, board: String) -> app::Result<Vec<PostView>> {
        let pinned = self
            .posts
            .query("board_pinned")
            .eq(&board)
            .desc()
            .entries()?;
        self.views(pinned)
    }

    /// Someone's posts across every board, newest first.
    ///
    /// Only rows whose owner stamp is that author: the `author` field is what
    /// the index is keyed by, and a patched peer could write it with anyone's
    /// name. The stamp it could not forge.
    pub fn author_feed(&self, author: String, limit: usize) -> app::Result<Vec<PostView>> {
        let page = self
            .posts
            .query("author_feed")
            .eq(&author)
            .desc()
            .limit(limit)
            .entries()?;
        let mut views = self.views(page)?;
        views.retain(|view| view.author == author);
        Ok(views)
    }

    /// Index rows counted; no post is loaded. A count cannot check whose each
    /// row is, so it also counts any entry a patched peer filed at someone
    /// else's id, which the feeds drop.
    pub fn board_stats(&self, board: String) -> app::Result<BoardStats> {
        Ok(BoardStats {
            posts: self.posts.query("board_feed").eq(&board).count()? as u64,
            pinned: self.posts.query("board_pinned").eq(&board).count()? as u64,
        })
    }

    // ── votes ──────────────────────────────────────────────────────────────

    /// Upvote a post and return its score. Voting twice is one vote, a
    /// concurrent vote from someone else is never lost, and nobody can vote,
    /// or unvote, in someone else's name: the vote is the caller's own entry.
    pub fn vote(&mut self, post: String) -> app::Result<u64> {
        if !self.post_exists(&post)? {
            app::bail!(Error::NoPost(&post));
        }
        if !self.votes.contains(&post)? {
            self.votes.insert(post.clone(), LwwRegister::new(true))?;
        }
        app::emit!(Event::Voted { post: &post });
        self.score(&post)
    }

    /// Take the caller's vote back and return the post's score.
    pub fn unvote(&mut self, post: String) -> app::Result<u64> {
        let _ = self.votes.remove(&post)?;
        self.score(&post)
    }

    // ── comments ───────────────────────────────────────────────────────────

    /// Comment on a post. `id` is the caller's to choose, and only ever
    /// collides with their own comments: the key carries their account.
    pub fn comment(
        &mut self,
        post: String,
        id: String,
        body: String,
        created_at: u64,
    ) -> app::Result<String> {
        if id.contains('/') {
            app::bail!(Error::HasSeparator(&id));
        }
        if !self.post_exists(&post)? {
            app::bail!(Error::NoPost(&post));
        }
        let key = format!("{post}/{created_at:020}/{}/{id}", caller());
        self.comments.insert(key.clone(), body.into())?;
        app::emit!(Event::Commented {
            post: &post,
            key: &key
        });
        Ok(key)
    }

    /// Replace the caller's comment at `key`. Keys are per owner, so nobody
    /// else can even address it here, and storage refuses anyone but its
    /// author on every node.
    pub fn edit_comment(&mut self, key: String, body: String) -> app::Result<()> {
        self.comments.update(&key, body.into())?;
        Ok(())
    }

    /// Take the caller's comment at `key` back. Only its author may: another
    /// account holds no comment at the key, so gets `NoComment`.
    pub fn delete_comment(&mut self, key: String) -> app::Result<()> {
        if self.comments.remove(&key)?.is_none() {
            app::bail!(Error::NoComment(&key));
        }
        Ok(())
    }

    /// One page of a post's thread, oldest first. A prefix seek: the other
    /// posts' comments are never read.
    pub fn comments(
        &self,
        post: String,
        offset: usize,
        limit: usize,
    ) -> app::Result<Vec<CommentView>> {
        if !self.post_exists(&post)? {
            app::bail!(Error::NoPost(&post));
        }
        let prefix = thread_prefix(&post);
        let mut thread = Vec::new();
        for (key, body) in self
            .comments
            .prefix(prefix.as_bytes())?
            .skip(offset)
            .take(limit)
        {
            let author = stamp_of(self.comments.entries_at(&key)?, &body)
                .map_or_else(String::new, |owner| hex(*owner.as_bytes()));
            let created_at = key
                .strip_prefix(&prefix)
                .and_then(|rest| rest.split('/').next())
                .and_then(|at| at.parse().ok())
                .unwrap_or_default();
            thread.push(CommentView {
                key,
                author,
                body: body.get().clone(),
                created_at,
            });
        }
        Ok(thread)
    }
}

impl Forum {
    /// A readable error for what storage would refuse anyway: `NoPost` if
    /// there is no post at `id`, `NotAuthor` if it is someone else's.
    fn check_author(&self, id: &String) -> app::Result<()> {
        if !self.post_exists(id)? {
            app::bail!(Error::NoPost(id));
        }
        if named_account(id) != Some(AccountId::from(env::account_id())) {
            app::bail!(Error::NotAuthor(id));
        }
        Ok(())
    }

    /// Whether the account `id` names holds a post there. A key-only
    /// `contains` asks about the caller's own entry only.
    fn post_exists(&self, id: &String) -> app::Result<bool> {
        Ok(self.holder(id)?.is_some())
    }

    /// The post at `id`: the entry of the account the id names. An entry any
    /// other account holds at the id is a patched peer's, and never read.
    fn holder(&self, id: &String) -> app::Result<Option<(AccountId, Post)>> {
        let Some(author) = named_account(id) else {
            return Ok(None);
        };
        Ok(self.posts.get_by(&author, id)?.map(|post| (author, post)))
    }

    /// How many accounts upvoted `post`: one entry each, in one bucket.
    fn score(&self, post: &String) -> app::Result<u64> {
        Ok(self.votes.entries_at(post)?.len() as u64)
    }

    fn change_own_post(&mut self, id: &String, f: impl FnOnce(&mut Post)) -> app::Result<()> {
        self.check_author(id)?;
        self.posts.modify(id, f)?;
        app::emit!(Event::PostChanged { id });
        Ok(())
    }

    /// `author` is the entry's owner stamp, or `None` if it could not be
    /// told, which shows as `""`.
    fn view_of(&self, id: String, author: Option<AccountId>, post: &Post) -> app::Result<PostView> {
        let votes = self.score(&id)?;
        let voted_by_me = self.votes.contains(&id)?;
        let comments = self.comments.prefix(thread_prefix(&id).as_bytes())?.count() as u64;
        Ok(PostView {
            board: post.board.get().clone(),
            author: author.map_or_else(String::new, |owner| hex(*owner.as_bytes())),
            title: post.title.get().clone(),
            body: post.body.get().clone(),
            tags: post.tags.get().clone(),
            created_at: *post.created_at.get(),
            pinned: post.pinned_at.get().is_some(),
            votes,
            comments,
            voted_by_me,
            id,
        })
    }

    /// Views of index rows. A row does not say whose it is, so each row's
    /// author is the holder of its id whose entry it is, and a row whose
    /// author is not the account its id names is dropped: a patched peer's
    /// entry at someone else's id. A page with such rows comes back short.
    fn views(&self, entries: Vec<(String, Post)>) -> app::Result<Vec<PostView>> {
        let mut views = Vec::with_capacity(entries.len());
        for (id, post) in entries {
            let author = stamp_of(self.posts.entries_at(&id)?, &post);
            if author.is_some() && author == named_account(&id) {
                views.push(self.view_of(id, author, &post)?);
            }
        }
        Ok(views)
    }
}

#[cfg(test)]
mod tests {
    use calimero_sdk::testing::TestHost;

    use super::*;

    const ALICE: [u8; 32] = [0xA1; 32];
    const BOB: [u8; 32] = [0xB0; 32];

    /// The id `create_post` mints for `who`'s post `id`.
    fn pid(who: [u8; 32], id: &str) -> String {
        format!("{}-{id}", hex(who))
    }

    /// Alice files four posts in `dev` and one in `ops`.
    fn forum() -> TestHost<Forum> {
        let mut app = TestHost::new(Forum::init);
        for (id, board, tags, at) in [
            ("p1", "dev", vec!["rust", "wasm"], 10),
            ("p2", "dev", vec!["rust"], 20),
            ("p3", "dev", vec!["js"], 30),
            ("p4", "dev", vec![], 40),
            ("o1", "ops", vec!["rust"], 50),
        ] {
            let minted = app
                .call_as_account(ALICE, ALICE, |s| {
                    s.create_post(
                        id.into(),
                        board.into(),
                        format!("post {id}"),
                        String::new(),
                        tags.into_iter().map(str::to_owned).collect(),
                        at,
                    )
                })
                .expect("post");
            assert_eq!(minted, pid(ALICE, id));
        }
        app.set_account(ALICE);
        app
    }

    /// The caller-chosen part of each view's id.
    fn ids(views: Vec<PostView>) -> Vec<String> {
        views
            .into_iter()
            .map(|v| v.id.split_once('-').expect("minted id").1.to_owned())
            .collect()
    }

    #[test]
    fn a_board_pages_newest_first_and_polls_by_time() {
        let app = forum();
        let page = |offset, limit| {
            ids(app
                .view(|s| s.board_feed("dev".into(), offset, limit))
                .expect("feed"))
        };
        assert_eq!(page(0, 2), ["p4", "p3"]);
        assert_eq!(page(2, 2), ["p2", "p1"]);
        assert!(page(4, 2).is_empty());

        let since = ids(app
            .view(|s| s.board_since("dev".into(), 20))
            .expect("since"));
        assert_eq!(since, ["p3", "p4"]);
    }

    #[test]
    fn a_tag_feed_is_scoped_to_its_board() {
        let mut app = forum();
        let rust = |app: &TestHost<Forum>| {
            ids(app
                .view(|s| s.board_tag_feed("dev".into(), "rust".into(), 0, 10))
                .expect("tag feed"))
        };
        // `o1` is tagged `rust` too, but lives in `ops`.
        assert_eq!(rust(&app), ["p2", "p1"]);

        app.call_as_account(ALICE, ALICE, |s| {
            s.retag(pid(ALICE, "p3"), vec!["rust".into(), "js".into()])
        })
        .expect("retag");
        app.call_as_account(ALICE, ALICE, |s| {
            s.retag(pid(ALICE, "p1"), vec!["wasm".into()])
        })
        .expect("retag");
        assert_eq!(rust(&app), ["p3", "p2"]);
    }

    #[test]
    fn pins_and_counts_come_from_the_index() {
        let mut app = forum();
        app.call_as_account(ALICE, ALICE, |s| s.pin(pid(ALICE, "p1"), 100))
            .expect("pin");
        app.call_as_account(ALICE, ALICE, |s| s.pin(pid(ALICE, "p3"), 200))
            .expect("pin");
        assert_eq!(
            ids(app.view(|s| s.pinned("dev".into())).expect("pinned")),
            ["p3", "p1"]
        );

        app.call_as_account(ALICE, ALICE, |s| s.unpin(pid(ALICE, "p3")))
            .expect("unpin");
        app.call_as_account(ALICE, ALICE, |s| s.delete_post(pid(ALICE, "p4")))
            .expect("delete");
        let stats = app.view(|s| s.board_stats("dev".into())).expect("stats");
        assert_eq!((stats.posts, stats.pinned), (3, 1));
        assert_eq!(
            ids(app.view(|s| s.author_feed(hex(ALICE), 2)).expect("author")),
            ["o1", "p3"]
        );
    }

    /// The founder: whoever ran `init`, and the first moderator.
    fn founder(app: &TestHost<Forum>) -> [u8; 32] {
        let moderators = app.view(|s| s.moderators()).expect("moderators");
        assert_eq!(moderators.len(), 1, "one founding moderator");
        let bytes: Vec<u8> = (0..32)
            .map(|i| u8::from_str_radix(&moderators[0][2 * i..2 * i + 2], 16).expect("hex"))
            .collect();
        bytes.try_into().expect("32 bytes")
    }

    #[test]
    fn the_charter_is_frozen_at_init() {
        let app = forum();
        assert_eq!(app.view(|s| s.charter()).expect("charter"), CHARTER);
    }

    #[test]
    fn a_moderator_removes_anyone_s_post_and_nobody_else_can() {
        let mut app = forum();
        let founder = founder(&app);
        assert_ne!(founder, ALICE, "the founder is not the author here");
        let p1 = pid(ALICE, "p1");

        assert!(app
            .call_as_account(BOB, BOB, |s| s.moderate_post(p1.clone()))
            .is_err());
        assert!(
            app.call_as_account(ALICE, ALICE, |s| s.moderate_post(p1.clone()))
                .is_err(),
            "authoring a post does not make you a moderator"
        );

        app.call_as_account(founder, founder, |s| s.moderate_post(p1.clone()))
            .expect("the founder moderates");
        assert!(app.view(|s| s.get_post(p1.clone())).expect("get").is_none());
        let dev = app.view(|s| s.board_stats("dev".into())).expect("stats");
        assert_eq!(dev.posts, 3, "the board's index follows the removal");
    }

    #[test]
    fn only_the_author_may_change_a_post() {
        let mut app = forum();
        let p1 = pid(ALICE, "p1");
        assert!(app
            .call_as_account(BOB, BOB, |s| s.pin(p1.clone(), 1))
            .is_err());
        assert!(app
            .call_as_account(BOB, BOB, |s| s.retag(p1.clone(), vec![]))
            .is_err());
        assert!(app
            .call_as_account(BOB, BOB, |s| s.delete_post(p1.clone()))
            .is_err());
        assert!(app
            .call_as_account(ALICE, ALICE, |s| s.create_post(
                "p1".into(),
                "dev".into(),
                "again".into(),
                String::new(),
                vec![],
                1
            ))
            .is_err());
        let stats = app.view(|s| s.board_stats("dev".into())).expect("stats");
        assert_eq!((stats.posts, stats.pinned), (4, 0));
    }

    /// Everything Bob does to Alice's post names it by an id that names Alice,
    /// and each read has to find her entry.
    #[test]
    fn another_account_reads_and_joins_someone_s_post() {
        let mut app = forum();
        let p1 = pid(ALICE, "p1");
        let bobs = app
            .call_as_account(BOB, BOB, |s| {
                s.create_post(
                    "p1".into(),
                    "dev".into(),
                    "bob's own".into(),
                    String::new(),
                    vec![],
                    1,
                )
            })
            .expect("Bob's `p1` is his own id");
        assert_eq!(bobs, pid(BOB, "p1"));

        app.set_account(BOB);
        let post = app
            .view(|s| s.get_post(p1.clone()))
            .expect("get")
            .expect("Bob reads Alice's post");
        assert_eq!(post.author, hex(ALICE));
        assert_eq!(post.title, "post p1");
        assert_eq!(
            app.view(|s| s.author_feed(hex(ALICE), 10))
                .expect("author")
                .len(),
            5
        );

        assert_eq!(
            app.call_as_account(BOB, BOB, |s| s.vote(p1.clone()))
                .expect("Bob votes on Alice's post"),
            1
        );
        let _ = app
            .call_as_account(BOB, BOB, |s| {
                s.comment(p1.clone(), "c1".into(), "nice".into(), 5)
            })
            .expect("Bob comments on Alice's post");
        app.set_account(ALICE);
        let thread = app.view(|s| s.comments(p1.clone(), 0, 10)).expect("thread");
        assert_eq!(thread.len(), 1);
        assert_eq!(thread[0].author, hex(BOB));
    }

    /// What a patched peer does: skip `create_post` and file its own entry at
    /// an id that names Alice. Mallory's account sorts below Alice's, which is
    /// exactly what a "lowest account wins" read would hand the post to, and
    /// what a member can grind for. No read by id sees it, and the feeds drop
    /// it.
    #[test]
    fn an_entry_at_someone_else_s_id_is_never_read() {
        const MALLORY: [u8; 32] = [0x01; 32];
        let mut app = forum();
        let p1 = pid(ALICE, "p1");
        app.call_as_account(MALLORY, MALLORY, |s| {
            let forged = Post {
                board: LwwRegister::new("dev".to_owned()),
                author: LwwRegister::new(hex(ALICE)),
                tags: LwwRegister::new(vec!["rust".to_owned()]),
                pinned_at: LwwRegister::new(None),
                created_at: LwwRegister::new(1_000),
                title: LwwRegister::new("forged".to_owned()),
                body: LwwRegister::new(String::new()),
            };
            s.posts.insert(p1.clone(), forged)
        })
        .expect("storage keeps Mallory's entry apart from Alice's");

        app.set_account(BOB);
        let post = app
            .view(|s| s.get_post(p1.clone()))
            .expect("get")
            .expect("p1");
        assert_eq!((post.title.as_str(), post.author), ("post p1", hex(ALICE)));
        for feed in [
            app.view(|s| s.board_feed("dev".into(), 0, 10))
                .expect("feed"),
            app.view(|s| s.board_tag_feed("dev".into(), "rust".into(), 0, 10))
                .expect("tag feed"),
            app.view(|s| s.author_feed(hex(ALICE), 10)).expect("author"),
        ] {
            assert!(feed.iter().all(|v| v.title != "forged"));
        }
    }

    #[test]
    fn a_vote_is_its_voter_s_own_entry() {
        let mut app = forum();
        let p1 = pid(ALICE, "p1");
        assert_eq!(
            app.call_as_account(ALICE, ALICE, |s| s.vote(p1.clone()))
                .expect("vote"),
            1
        );
        assert_eq!(
            app.call_as_account(ALICE, ALICE, |s| s.vote(p1.clone()))
                .expect("vote again"),
            1
        );
        assert_eq!(
            app.call_as_account(BOB, BOB, |s| s.vote(p1.clone()))
                .expect("bob votes"),
            2
        );
        // Bob holds one vote per post, however he writes it.
        assert!(app
            .call_as_account(BOB, BOB, |s| s
                .votes
                .insert(p1.clone(), LwwRegister::new(true)))
            .is_err());
        assert_eq!(
            app.call_as_account(ALICE, ALICE, |s| s.unvote(p1.clone()))
                .expect("unvote"),
            1
        );
        // Nor can he take anyone else's back: his unvote is his own.
        assert_eq!(
            app.call_as_account(ALICE, ALICE, |s| s.vote(p1.clone()))
                .expect("vote"),
            2
        );
        assert_eq!(
            app.call_as_account(BOB, BOB, |s| s.unvote(p1.clone()))
                .expect("bob unvotes"),
            1
        );
        assert!(app
            .call_as_account(BOB, BOB, |s| s.vote("nope".into()))
            .is_err());

        app.set_account(ALICE);
        let post = app
            .view(|s| s.get_post(p1.clone()))
            .expect("get")
            .expect("p1");
        assert_eq!((post.votes, post.voted_by_me), (1, true));
    }

    #[test]
    fn a_thread_is_one_post_s_comments_in_time_order() {
        let mut app = forum();
        let (p1, p2) = (pid(ALICE, "p1"), pid(ALICE, "p2"));
        let late = app
            .call_as_account(BOB, BOB, |s| {
                s.comment(p1.clone(), "c1".into(), "second".into(), 200)
            })
            .expect("bob comments");
        let _ = app
            .call_as_account(ALICE, ALICE, |s| {
                s.comment(p1.clone(), "c1".into(), "first".into(), 100)
            })
            .expect("alice comments");
        let _ = app
            .call_as_account(ALICE, ALICE, |s| {
                s.comment(p2.clone(), "c1".into(), "elsewhere".into(), 150)
            })
            .expect("other thread");
        assert!(app
            .call_as_account(ALICE, ALICE, |s| {
                s.comment("nope".into(), "c1".into(), "lost".into(), 1)
            })
            .is_err());

        let thread = app.view(|s| s.comments(p1.clone(), 0, 10)).expect("thread");
        let bodies: Vec<_> = thread.iter().map(|c| c.body.as_str()).collect();
        assert_eq!(bodies, ["first", "second"]);
        assert_eq!(thread[1].author, hex(BOB));
        assert_eq!(thread[1].created_at, 200);
        assert_eq!(
            app.view(|s| s.get_post(p1.clone()))
                .expect("get")
                .expect("p1")
                .comments,
            2
        );

        // Storage, not this app, refuses Alice here.
        assert!(app
            .call_as_account(ALICE, ALICE, |s| s
                .edit_comment(late.clone(), "mine now".into()))
            .is_err());
        assert!(app
            .call_as_account(ALICE, ALICE, |s| s.delete_comment(late.clone()))
            .is_err());
        app.call_as_account(BOB, BOB, |s| s.delete_comment(late.clone()))
            .expect("bob deletes his own");
        assert_eq!(
            app.view(|s| s.comments(p1.clone(), 0, 10))
                .expect("thread")
                .len(),
            1
        );
    }
}
