//! A forum built from three collections: `Authored<IndexedMap>` for posts,
//! [`AuthoredSortedMap`] for comments and [`UnorderedSet`] for votes. Every
//! list view is a seek, and each collection covers what the other two can't.
//!
//! Read [`indexed-issue-tracker`](../../indexed-issue-tracker) first. It covers
//! `IndexedMap` on its own. This app shows the harder shapes and how
//! `IndexedMap` combines with the other collections.
//!
//! # Posts: `Authored<IndexedMap>`, with compound, multi-valued and optional indexes
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
//! # Comments: `AuthoredSortedMap`, keyed so one post's thread is one slice
//!
//! A comment's key is `"<post>/<created_at>/<account>/<id>"`. `prefix("<post>/")`
//! is a seek, and the zero-padded time orders the slice. The storage layer
//! gates edits and removals on every node, so only a comment's author can
//! change it. A patched peer can't get around that.
//!
//! # Votes: `UnorderedSet` per post, not a number on the post
//!
//! A score kept on the post would be a register. Two people voting at once
//! would each write `score + 1`, and one vote would be lost. A set of voter
//! accounts merges by union, so every vote survives, and the score is the
//! set's size. The same reason leaves the score out of the post's indexes:
//! an index key must come from the value, and nothing in the value can hold a
//! converging count. So this app has no "top posts" index.
//!
//! # Where the gate is enforced
//!
//! By storage, on every node, for posts and comments alike. Each is stamped
//! with the account that wrote it, and a node applying a peer's write refuses
//! an edit or removal by anyone else, including a peer running patched code.
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
//! the indexes knowing, so the first post query after it rebuilds them. `workflows/indexed-forum.yml` runs that across two nodes.

use core::ops::Bound;

use calimero_sdk::abi::AbiType;
use calimero_sdk::borsh::{BorshDeserialize, BorshSerialize};
use calimero_sdk::serde::Serialize;
use calimero_sdk::{app, env};
use calimero_storage::collections::{
    AuthoredSortedMap, Frozen, IndexedMap, LwwRegister, Moderated, UnorderedMap, UnorderedSet,
};
use thiserror::Error;

/// One post, and what it is looked up by.
///
/// Every field is an `LwwRegister`, so the derived merge works field by field:
/// a concurrent retag and pin both survive.
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
    /// Post id -> post. The author edits and deletes; a moderator removes.
    posts: Moderated<IndexedMap<String, Post>>,
    /// `"<post>/<created_at:020>/<account>/<id>"` -> comment body.
    comments: AuthoredSortedMap<String, LwwRegister<String>>,
    /// Post id -> the accounts that upvoted it.
    votes: UnorderedMap<String, UnorderedSet<String>>,
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
            votes: UnorderedMap::new(),
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

    /// Remove someone's post as a moderator. Every node checks the remover
    /// against the moderators as of the removal.
    pub fn moderate_post(&mut self, id: String) -> app::Result<()> {
        if !self.posts.contains(&id)? {
            app::bail!(Error::NoPost(&id));
        }
        let me = calimero_sdk::AccountId::from(env::account_id());
        if !self.posts.is_moderator(&me) {
            app::bail!(Error::NotModerator(&id));
        }
        let _ = self.posts.remove(&id)?;
        let _ = self.votes.remove(&id)?;
        app::emit!(Event::PostDeleted { id: &id });
        Ok(())
    }

    // ── posts ──────────────────────────────────────────────────────────────

    /// File a post. `created_at` is the client's clock: it orders the post in
    /// every feed it appears in.
    pub fn create_post(
        &mut self,
        id: String,
        board: String,
        title: String,
        body: String,
        tags: Vec<String>,
        created_at: u64,
    ) -> app::Result<()> {
        if id.contains('/') {
            app::bail!(Error::HasSeparator(&id));
        }
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
        Ok(())
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

    /// Delete a post and its votes.
    ///
    /// Its comments stay: each belongs to its own author, and only they may
    /// remove it. Every comment read goes through a live post, so the orphans
    /// are never shown.
    pub fn delete_post(&mut self, id: String) -> app::Result<()> {
        self.check_author(&id)?;
        let _ = self.posts.remove(&id)?;
        let _ = self.votes.remove(&id)?;
        app::emit!(Event::PostDeleted { id: &id });
        Ok(())
    }

    pub fn get_post(&self, id: String) -> app::Result<Option<PostView>> {
        let Some(post) = self.posts.get(&id)? else {
            return Ok(None);
        };
        Ok(Some(self.view_of(id, &post)?))
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
        let mut genuine = Vec::with_capacity(page.len());
        for (id, post) in page {
            if self.author_of(&id)? == author {
                genuine.push((id, post));
            }
        }
        self.views(genuine)
    }

    /// Index rows counted; no post is loaded.
    pub fn board_stats(&self, board: String) -> app::Result<BoardStats> {
        Ok(BoardStats {
            posts: self.posts.query("board_feed").eq(&board).count()? as u64,
            pinned: self.posts.query("board_pinned").eq(&board).count()? as u64,
        })
    }

    // ── votes ──────────────────────────────────────────────────────────────

    /// Upvote a post. Voting twice is one vote; a concurrent vote from someone
    /// else is never lost.
    pub fn vote(&mut self, post: String) -> app::Result<u64> {
        if !self.posts.contains(&post)? {
            app::bail!(Error::NoPost(&post));
        }
        let mut voters = self.votes.entry(post.clone())?.or_default()?;
        let _ = voters.insert(caller())?;
        let count = voters.len()? as u64;
        drop(voters);
        app::emit!(Event::Voted { post: &post });
        Ok(count)
    }

    pub fn unvote(&mut self, post: String) -> app::Result<u64> {
        let Some(mut voters) = self.votes.get_mut(&post)? else {
            return Ok(0);
        };
        let _ = voters.remove(&caller())?;
        Ok(voters.len()? as u64)
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
        if !self.posts.contains(&post)? {
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

    /// Replace a comment. Storage refuses anyone but its author, on every node.
    pub fn edit_comment(&mut self, key: String, body: String) -> app::Result<()> {
        self.comments.update(&key, body.into())?;
        Ok(())
    }

    /// Take a comment back. Only its author may.
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
        if !self.posts.contains(&post)? {
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
            let author = self
                .comments
                .owner_of(&key)?
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
    /// A readable error for what storage would refuse anyway.
    fn check_author(&self, id: &String) -> app::Result<()> {
        if !self.posts.contains(id)? {
            app::bail!(Error::NoPost(id));
        }
        if !self.posts.owned_by_me(id)? {
            app::bail!(Error::NotAuthor(id));
        }
        Ok(())
    }

    fn change_own_post(&mut self, id: &String, f: impl FnOnce(&mut Post)) -> app::Result<()> {
        self.check_author(id)?;
        self.posts.modify(id, f)?;
        app::emit!(Event::PostChanged { id });
        Ok(())
    }

    /// The post's owner stamp, hex-encoded.
    fn author_of(&self, id: &String) -> app::Result<String> {
        Ok(self
            .posts
            .owner_of(id)?
            .map_or_else(String::new, |owner| hex(*owner.as_bytes())))
    }

    fn view_of(&self, id: String, post: &Post) -> app::Result<PostView> {
        let me = caller();
        let (votes, voted_by_me) = match self.votes.get(&id)? {
            Some(voters) => (voters.len()? as u64, voters.contains(&me)?),
            None => (0, false),
        };
        let comments = self.comments.prefix(thread_prefix(&id).as_bytes())?.count() as u64;
        Ok(PostView {
            board: post.board.get().clone(),
            author: self.author_of(&id)?,
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

    fn views(&self, entries: Vec<(String, Post)>) -> app::Result<Vec<PostView>> {
        entries
            .into_iter()
            .map(|(id, post)| self.view_of(id, &post))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use calimero_sdk::testing::TestHost;

    use super::*;

    const ALICE: [u8; 32] = [0xA1; 32];
    const BOB: [u8; 32] = [0xB0; 32];

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
            app.call_as_account(ALICE, ALICE, |s| {
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
        }
        app.set_account(ALICE);
        app
    }

    fn ids(views: Vec<PostView>) -> Vec<String> {
        views.into_iter().map(|v| v.id).collect()
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
            s.retag("p3".into(), vec!["rust".into(), "js".into()])
        })
        .expect("retag");
        app.call_as_account(ALICE, ALICE, |s| s.retag("p1".into(), vec!["wasm".into()]))
            .expect("retag");
        assert_eq!(rust(&app), ["p3", "p2"]);
    }

    #[test]
    fn pins_and_counts_come_from_the_index() {
        let mut app = forum();
        app.call_as_account(ALICE, ALICE, |s| s.pin("p1".into(), 100))
            .expect("pin");
        app.call_as_account(ALICE, ALICE, |s| s.pin("p3".into(), 200))
            .expect("pin");
        assert_eq!(
            ids(app.view(|s| s.pinned("dev".into())).expect("pinned")),
            ["p3", "p1"]
        );

        app.call_as_account(ALICE, ALICE, |s| s.unpin("p3".into()))
            .expect("unpin");
        app.call_as_account(ALICE, ALICE, |s| s.delete_post("p4".into()))
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

        assert!(app
            .call_as_account(BOB, BOB, |s| s.moderate_post("p1".into()))
            .is_err());
        assert!(
            app.call_as_account(ALICE, ALICE, |s| s.moderate_post("p1".into()))
                .is_err(),
            "authoring a post does not make you a moderator"
        );

        app.call_as_account(founder, founder, |s| s.moderate_post("p1".into()))
            .expect("the founder moderates");
        assert!(app
            .view(|s| s.get_post("p1".into()))
            .expect("get")
            .is_none());
        let dev = app.view(|s| s.board_stats("dev".into())).expect("stats");
        assert_eq!(dev.posts, 3, "the board's index follows the removal");
    }

    #[test]
    fn only_the_author_may_change_a_post() {
        let mut app = forum();
        assert!(app
            .call_as_account(BOB, BOB, |s| s.pin("p1".into(), 1))
            .is_err());
        assert!(app
            .call_as_account(BOB, BOB, |s| s.retag("p1".into(), vec![]))
            .is_err());
        assert!(app
            .call_as_account(BOB, BOB, |s| s.delete_post("p1".into()))
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

    #[test]
    fn votes_are_a_set_of_accounts() {
        let mut app = forum();
        assert_eq!(
            app.call_as_account(ALICE, ALICE, |s| s.vote("p1".into()))
                .expect("vote"),
            1
        );
        assert_eq!(
            app.call_as_account(ALICE, ALICE, |s| s.vote("p1".into()))
                .expect("vote again"),
            1
        );
        assert_eq!(
            app.call_as_account(BOB, BOB, |s| s.vote("p1".into()))
                .expect("bob votes"),
            2
        );
        assert_eq!(
            app.call_as_account(ALICE, ALICE, |s| s.unvote("p1".into()))
                .expect("unvote"),
            1
        );
        assert!(app
            .call_as_account(BOB, BOB, |s| s.vote("nope".into()))
            .is_err());

        app.set_account(BOB);
        let post = app
            .view(|s| s.get_post("p1".into()))
            .expect("get")
            .expect("p1");
        assert_eq!((post.votes, post.voted_by_me), (1, true));
    }

    #[test]
    fn a_thread_is_one_post_s_comments_in_time_order() {
        let mut app = forum();
        let late = app
            .call_as_account(BOB, BOB, |s| {
                s.comment("p1".into(), "c1".into(), "second".into(), 200)
            })
            .expect("bob comments");
        let _ = app
            .call_as_account(ALICE, ALICE, |s| {
                s.comment("p1".into(), "c1".into(), "first".into(), 100)
            })
            .expect("alice comments");
        let _ = app
            .call_as_account(ALICE, ALICE, |s| {
                s.comment("p2".into(), "c1".into(), "elsewhere".into(), 150)
            })
            .expect("other thread");
        assert!(app
            .call_as_account(ALICE, ALICE, |s| {
                s.comment("nope".into(), "c1".into(), "lost".into(), 1)
            })
            .is_err());

        let thread = app
            .view(|s| s.comments("p1".into(), 0, 10))
            .expect("thread");
        let bodies: Vec<_> = thread.iter().map(|c| c.body.as_str()).collect();
        assert_eq!(bodies, ["first", "second"]);
        assert_eq!(thread[1].author, hex(BOB));
        assert_eq!(thread[1].created_at, 200);
        assert_eq!(
            app.view(|s| s.get_post("p1".into()))
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
            app.view(|s| s.comments("p1".into(), 0, 10))
                .expect("thread")
                .len(),
            1
        );
    }
}
