//! A forum on the component SDK: posts found by board, tag, pin and author through table
//! indexes, comments as one post's slice of an index, votes counted by a maintained count,
//! and a charter no method writes. Read `apps/indexed-issue-tracker` first for tables on
//! their own.
//!
//! v1 built this from storage write policies (`Moderated<IndexedMap>`, `Frozen<String>`,
//! `AuthoredSortedMap`, `AuthoredMap`), checked by every node applying a peer's write. A
//! component app runs on the app's leader and its writes are the log's, so each policy is
//! a check in the method, against `env::caller()`, before anything is written:
//!
//! | v1 policy | here |
//! |---|---|
//! | `Frozen<String>` charter | written in `init`; no method writes it |
//! | `Moderated<..>` posts | `post.author == caller` to change or delete; a row of `moderators` (the app owner, from `init`) to remove anyone's |
//! | `AuthoredSortedMap` comments | `comment.author == caller` to edit or delete; anyone else gets `NoComment` |
//! | `AuthoredMap` votes | a vote is the row `(post, caller)`: one per account, written and removed only as the caller |
//! | `#[index(board_tag_feed(board, tags, created_at))]` over a list | one `postTags` row per tag, indexed `(board, tag, createdAt)` |
//! | `pinned_at: Option<u64>` leaving no row | the `(board, pinnedAt)` rows of unpinned posts sort first (none before any integer); `pinned` starts its range at 0 |
//! | counts of index rows | maintained counts: `boardPins (board, pinned)`, `comments (post)`, `score (post)` |
//!
//! A post id names its author: `createPost` mints `"<caller>-<id>"`, so two accounts'
//! `p1` are two posts. The author is also a field of the post, and the checks read it.

use calimero_app_sdk::env::Caller;
use calimero_app_sdk::prelude::*;

/// One post.
#[derive(Record)]
#[table(
    key = "id",
    index(board_feed = ["board", "created_at"], board_pinned = ["board", "pinned_at"], author_feed = ["author", "created_at"]),
    count(board_pins = ["board", "pinned"])
)]
pub struct Post {
    /// `"<author>-<id>"`, minted by `createPost`.
    pub id: String,
    pub board: String,
    /// The account that filed it.
    pub author: String,
    pub title: String,
    pub body: String,
    /// The client's clock: it orders the post in the feeds, and nothing else.
    pub created_at: u64,
    pub pinned: bool,
    /// `None` while unpinned.
    pub pinned_at: Option<u64>,
}

/// One tag of one post: what the tag feed is an index of.
#[derive(Record)]
#[table(key = ["post", "tag"], index(board_tag_feed = ["board", "tag", "created_at"], by_post = "post"))]
pub struct PostTag {
    pub post: String,
    pub tag: String,
    pub board: String,
    pub created_at: u64,
}

/// One comment. Its key, `"<post>/<createdAt:020>/<author>/<id>"`, orders a thread by time.
#[derive(Record)]
#[table(key = "key", index(thread = "post"), count(per_post = "post"))]
pub struct Comment {
    pub key: String,
    pub post: String,
    pub author: String,
    pub body: String,
    pub created_at: u64,
}

/// One account's upvote of one post.
#[derive(Record)]
#[table(key = ["post", "voter"], count(score = "post"))]
pub struct Vote {
    pub post: String,
    pub voter: String,
}

/// One account that may remove any post.
#[derive(Record)]
#[table(key = "account")]
pub struct Moderator {
    pub account: String,
}

/// A post as a client sees it.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PostView {
    pub id: String,
    pub board: String,
    pub author: String,
    pub title: String,
    pub body: String,
    pub tags: Vec<String>,
    #[serde(with = "calimero_app_sdk::json::u64_str")]
    pub created_at: u64,
    pub pinned: bool,
    #[serde(with = "calimero_app_sdk::json::u64_str")]
    pub votes: u64,
    #[serde(with = "calimero_app_sdk::json::u64_str")]
    pub comments: u64,
    pub voted_by_me: bool,
}

/// A comment as a client sees it.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommentView {
    pub key: String,
    pub author: String,
    pub body: String,
    #[serde(with = "calimero_app_sdk::json::u64_str")]
    pub created_at: u64,
}

/// A board's size, read from maintained counts; no post is loaded.
#[derive(serde::Serialize)]
pub struct BoardStats {
    #[serde(with = "calimero_app_sdk::json::u64_str")]
    pub posts: u64,
    #[serde(with = "calimero_app_sdk::json::u64_str")]
    pub pinned: u64,
}

#[app::state(version = 1)]
pub struct Forum {
    /// `"charter"` -> the charter; written in `init` only.
    charter: UnorderedMap<String, String>,
    posts: Table<Post>,
    post_tags: Table<PostTag>,
    comments: Table<Comment>,
    votes: Table<Vote>,
    moderators: Table<Moderator>,
}

#[app::error]
pub enum Error {
    /// The caller already has a post with this id.
    Exists {
        id: String,
    },
    NoPost {
        id: String,
    },
    NoComment {
        key: String,
    },
    /// Only the author of post `id` may change it.
    NotAuthor {
        id: String,
    },
    /// Only a moderator may remove post `id`.
    NotModerator {
        id: String,
    },
    /// A `/` in `value` would move a comment into another post's thread.
    HasSeparator {
        value: String,
    },
}

#[app::event]
pub enum Event {
    Posted { id: String, board: String },
    PostChanged { id: String },
    PostDeleted { id: String },
    Commented { post: String, key: String },
    Voted { post: String },
}

/// Written in `init`; no method changes it.
const CHARTER: &str = "be kind and stay on topic";
/// The most rows one query reads, offset included.
const MAX_ROWS: u32 = 1000;

fn caller() -> app::Result<String> {
    match env::caller() {
        Caller::Account(a) | Caller::ApiKey { account: a, .. } => Ok(a),
        _ => Err(AppError::login_required()),
    }
}

/// A page of `limit` after `offset` rows, both capped so the read stays within 1000 rows.
fn window(offset: u32, limit: u32) -> (usize, u32) {
    let offset = offset.min(MAX_ROWS);
    (
        offset as usize,
        (offset + limit.min(MAX_ROWS)).min(MAX_ROWS),
    )
}

fn no_separator(value: &str) -> app::Result<()> {
    if value.contains('/') {
        return Err(Error::HasSeparator {
            value: value.to_owned(),
        }
        .into());
    }
    Ok(())
}

#[app::logic]
impl Forum {
    /// Writes the charter and makes the app owner the first moderator.
    #[app::init]
    pub fn init() -> app::Result<Self> {
        let mut s = Self::load();
        s.charter
            .insert(&"charter".to_owned(), &CHARTER.to_owned())?;
        s.moderators.insert(Moderator {
            account: env::app_owner(),
        })?;
        Ok(s)
    }

    /// The caller's own account, as the app sees it.
    #[app::view]
    pub fn me(&self) -> app::Result<String> {
        caller()
    }

    /// The charter the forum was founded with.
    #[app::view]
    pub fn charter(&self) -> app::Result<String> {
        Ok(self.charter.get(&"charter".to_owned())?.unwrap_or_default())
    }

    /// Who may remove any post.
    #[app::view]
    pub fn moderators(&self) -> app::Result<Vec<String>> {
        let page = self.moderators.scan(FindOpts::default().limit(MAX_ROWS))?;
        Ok(page.items.into_iter().map(|m| m.account).collect())
    }

    /// Removes the post at `id`, whoever wrote it. Moderators only. Its votes and
    /// comments stay; every read of them goes through a live post.
    pub fn moderate_post(&mut self, id: String) -> app::Result<()> {
        if !self.posts.contains(&id)? {
            return Err(Error::NoPost { id }.into());
        }
        if !self.moderators.contains(&caller()?)? {
            return Err(Error::NotModerator { id }.into());
        }
        self.remove_post(&id)?;
        env::emit(Event::PostDeleted { id }, Audience::All)?;
        Ok(())
    }

    // ── posts ──────────────────────────────────────────────────────────────

    /// Files a post and returns its id, `"<caller>-<id>"`: `id` is the caller's to
    /// choose and only ever collides with their own posts.
    pub fn create_post(
        &mut self,
        id: String,
        board: String,
        title: String,
        body: String,
        tags: Vec<String>,
        created_at: u64,
    ) -> app::Result<String> {
        no_separator(&id)?;
        let author = caller()?;
        let id = format!("{author}-{id}");
        if self.posts.contains(&id)? {
            return Err(Error::Exists { id }.into());
        }
        let mut tags = tags;
        tags.sort();
        tags.dedup();
        for tag in &tags {
            self.post_tags.insert(PostTag {
                post: id.clone(),
                tag: tag.clone(),
                board: board.clone(),
                created_at,
            })?;
        }
        self.posts.insert(Post {
            id: id.clone(),
            board: board.clone(),
            author,
            title,
            body,
            created_at,
            pinned: false,
            pinned_at: None,
        })?;
        env::emit(
            Event::Posted {
                id: id.clone(),
                board,
            },
            Audience::All,
        )?;
        Ok(id)
    }

    /// Replaces the title and body of the caller's post.
    pub fn edit_post(&mut self, id: String, title: String, body: String) -> app::Result<()> {
        self.change_own_post(id, |post| {
            post.title = title;
            post.body = body;
        })
    }

    /// Replaces the tags of the caller's post: it leaves the old tags' feeds and joins
    /// the new ones.
    pub fn retag(&mut self, id: String, tags: Vec<String>) -> app::Result<()> {
        let post = self.own_post(&id)?;
        self.drop_tags(&id)?;
        let mut tags = tags;
        tags.sort();
        tags.dedup();
        for tag in &tags {
            self.post_tags.insert(PostTag {
                post: id.clone(),
                tag: tag.clone(),
                board: post.board.clone(),
                created_at: post.created_at,
            })?;
        }
        env::emit(Event::PostChanged { id }, Audience::All)?;
        Ok(())
    }

    /// Pins the caller's post to the top of its board, ordered by `at`.
    pub fn pin(&mut self, id: String, at: u64) -> app::Result<()> {
        self.change_own_post(id, |post| {
            post.pinned = true;
            post.pinned_at = Some(at);
        })
    }

    /// Unpins the caller's post.
    pub fn unpin(&mut self, id: String) -> app::Result<()> {
        self.change_own_post(id, |post| {
            post.pinned = false;
            post.pinned_at = None;
        })
    }

    /// Deletes the caller's post. Its votes and comments stay, each its own author's;
    /// every read of them goes through a live post, so a re-post takes a new id.
    pub fn delete_post(&mut self, id: String) -> app::Result<()> {
        let _post = self.own_post(&id)?;
        self.remove_post(&id)?;
        env::emit(Event::PostDeleted { id }, Audience::All)?;
        Ok(())
    }

    /// The post at `id`, or `null`.
    #[app::view]
    pub fn get_post(&self, id: String) -> app::Result<Option<PostView>> {
        self.posts
            .get(&id)?
            .map(|post| self.view_of(post))
            .transpose()
    }

    /// One page of a board, newest first.
    #[app::view]
    pub fn board_feed(&self, board: String, offset: u32, limit: u32) -> app::Result<Vec<PostView>> {
        let (skip, read) = window(offset, limit);
        let page = self.posts.find(
            "boardFeed",
            &[board.to_key_part()],
            FindOpts::default().reverse().limit(read),
        )?;
        self.views(page.items.into_iter().skip(skip))
    }

    /// One page of a board's posts carrying `tag`, newest first: one range of the tag
    /// index, so other boards' posts with the tag are never read.
    #[app::view]
    pub fn board_tag_feed(
        &self,
        board: String,
        tag: String,
        offset: u32,
        limit: u32,
    ) -> app::Result<Vec<PostView>> {
        let (skip, read) = window(offset, limit);
        let rows = self.post_tags.find(
            "boardTagFeed",
            &[board.to_key_part(), tag.to_key_part()],
            FindOpts::default().reverse().limit(read),
        )?;
        let mut posts = Vec::new();
        for row in rows.items.into_iter().skip(skip) {
            if let Some(post) = self.posts.get(&row.post)? {
                posts.push(post);
            }
        }
        self.views(posts)
    }

    /// A board's posts created after `after`, oldest first: what a client polls.
    #[app::view]
    pub fn board_since(&self, board: String, after: u64) -> app::Result<Vec<PostView>> {
        let page = self.posts.find(
            "boardFeed",
            &[board.to_key_part()],
            FindOpts::default()
                .from(vec![KeyPart::Int(i128::from(after) + 1)])
                .limit(MAX_ROWS),
        )?;
        self.views(page.items)
    }

    /// A board's pinned posts, most recently pinned first.
    #[app::view]
    pub fn pinned(&self, board: String) -> app::Result<Vec<PostView>> {
        // Unpinned posts' rows hold `None`, which sorts before every integer.
        let page = self.posts.find(
            "boardPinned",
            &[board.to_key_part()],
            FindOpts::default()
                .from(vec![KeyPart::Int(0)])
                .reverse()
                .limit(MAX_ROWS),
        )?;
        self.views(page.items)
    }

    /// Someone's posts across every board, newest first.
    #[app::view]
    pub fn author_feed(&self, author: String, limit: u32) -> app::Result<Vec<PostView>> {
        let page = self.posts.find(
            "authorFeed",
            &[author.to_key_part()],
            FindOpts::default().reverse().limit(limit.min(MAX_ROWS)),
        )?;
        self.views(page.items)
    }

    /// A board's post and pin counts, from maintained counts.
    #[app::view]
    pub fn board_stats(&self, board: String) -> app::Result<BoardStats> {
        Ok(BoardStats {
            posts: self.posts.count("boardPins", &[board.to_key_part()])?,
            pinned: self
                .posts
                .count("boardPins", &[board.to_key_part(), true.to_key_part()])?,
        })
    }

    // ── votes ──────────────────────────────────────────────────────────────

    /// Upvotes a post and returns its score. Voting twice is one vote: the vote is the
    /// row `(post, caller)`.
    pub fn vote(&mut self, post: String) -> app::Result<u64> {
        let voter = caller()?;
        if !self.posts.contains(&post)? {
            return Err(Error::NoPost { id: post }.into());
        }
        if !self.votes.contains(&(post.as_str(), voter.as_str()))? {
            self.votes.insert(Vote {
                post: post.clone(),
                voter,
            })?;
        }
        let score = self.score(&post)?;
        env::emit(Event::Voted { post }, Audience::All)?;
        Ok(score)
    }

    /// Takes the caller's vote back (nobody else's) and returns the post's score.
    pub fn unvote(&mut self, post: String) -> app::Result<u64> {
        let voter = caller()?;
        self.votes.remove(&(post.as_str(), voter.as_str()))?;
        self.score(&post)
    }

    // ── comments ───────────────────────────────────────────────────────────

    /// Comments on a post and returns the comment's key. `id` is the caller's to
    /// choose: the key carries their account, so it only collides with their own.
    pub fn comment(
        &mut self,
        post: String,
        id: String,
        body: String,
        created_at: u64,
    ) -> app::Result<String> {
        no_separator(&id)?;
        let author = caller()?;
        if !self.posts.contains(&post)? {
            return Err(Error::NoPost { id: post }.into());
        }
        let key = format!("{post}/{created_at:020}/{author}/{id}");
        self.comments.upsert(Comment {
            key: key.clone(),
            post: post.clone(),
            author,
            body,
            created_at,
        })?;
        env::emit(
            Event::Commented {
                post,
                key: key.clone(),
            },
            Audience::All,
        )?;
        Ok(key)
    }

    /// Replaces the caller's comment at `key`. Another account's comment is not the
    /// caller's to address: `NoComment`.
    pub fn edit_comment(&mut self, key: String, body: String) -> app::Result<()> {
        let _own = self.own_comment(&key)?;
        self.comments.update(&key, |c| c.body = body)?;
        Ok(())
    }

    /// Takes the caller's comment at `key` back; anyone else's is `NoComment`.
    pub fn delete_comment(&mut self, key: String) -> app::Result<()> {
        let _own = self.own_comment(&key)?;
        self.comments.remove(&key)?;
        Ok(())
    }

    /// One page of a post's thread, oldest first: one range of the thread index.
    #[app::view]
    pub fn comments(&self, post: String, offset: u32, limit: u32) -> app::Result<Vec<CommentView>> {
        if !self.posts.contains(&post)? {
            return Err(Error::NoPost { id: post }.into());
        }
        let (skip, read) = window(offset, limit);
        let page = self.comments.find(
            "thread",
            &[post.to_key_part()],
            FindOpts::default().limit(read),
        )?;
        Ok(page
            .items
            .into_iter()
            .skip(skip)
            .map(|c| CommentView {
                key: c.key,
                author: c.author,
                body: c.body,
                created_at: c.created_at,
            })
            .collect())
    }

    // ── helpers ────────────────────────────────────────────────────────────

    /// The post at `id` if the caller wrote it: `NoPost`, else `NotAuthor`.
    fn own_post(&self, id: &str) -> app::Result<Post> {
        let me = caller()?;
        let Some(post) = self.posts.get(id)? else {
            return Err(Error::NoPost { id: id.to_owned() }.into());
        };
        if post.author != me {
            return Err(Error::NotAuthor { id: id.to_owned() }.into());
        }
        Ok(post)
    }

    fn own_comment(&self, key: &str) -> app::Result<Comment> {
        let me = caller()?;
        match self.comments.get(key)? {
            Some(c) if c.author == me => Ok(c),
            _ => Err(Error::NoComment {
                key: key.to_owned(),
            }
            .into()),
        }
    }

    fn change_own_post(&mut self, id: String, change: impl FnOnce(&mut Post)) -> app::Result<()> {
        let _post = self.own_post(&id)?;
        self.posts.update(&id, change)?;
        env::emit(Event::PostChanged { id }, Audience::All)?;
        Ok(())
    }

    fn remove_post(&mut self, id: &str) -> app::Result<()> {
        self.drop_tags(id)?;
        self.posts.remove(id)?;
        Ok(())
    }

    /// A post's tags, sorted: its `postTags` rows, not a field of the post (every record
    /// field must be a key-part type, and a list is not one).
    fn tags_of(&self, id: &str) -> app::Result<Vec<String>> {
        let rows = self.post_tags.find(
            "byPost",
            &[id.to_key_part()],
            FindOpts::default().limit(MAX_ROWS),
        )?;
        Ok(rows.items.into_iter().map(|r| r.tag).collect())
    }

    fn drop_tags(&mut self, id: &str) -> app::Result<()> {
        let rows = self.post_tags.find(
            "byPost",
            &[id.to_key_part()],
            FindOpts::default().limit(MAX_ROWS),
        )?;
        for row in rows.items {
            self.post_tags
                .remove(&(row.post.as_str(), row.tag.as_str()))?;
        }
        Ok(())
    }

    fn score(&self, post: &str) -> app::Result<u64> {
        Ok(self.votes.count("score", &[post.to_key_part()])?)
    }

    fn view_of(&self, post: Post) -> app::Result<PostView> {
        let voted_by_me = match caller() {
            Ok(me) => self.votes.contains(&(post.id.as_str(), me.as_str()))?,
            Err(_) => false,
        };
        Ok(PostView {
            tags: self.tags_of(&post.id)?,
            votes: self.score(&post.id)?,
            comments: self.comments.count("perPost", &[post.id.to_key_part()])?,
            voted_by_me,
            id: post.id,
            board: post.board,
            author: post.author,
            title: post.title,
            body: post.body,
            created_at: post.created_at,
            pinned: post.pinned,
        })
    }

    fn views(&self, posts: impl IntoIterator<Item = Post>) -> app::Result<Vec<PostView>> {
        posts.into_iter().map(|p| self.view_of(p)).collect()
    }
}
