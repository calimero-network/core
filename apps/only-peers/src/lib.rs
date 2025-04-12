use calimero_sdk::borsh::{BorshDeserialize, BorshSerialize};
use calimero_sdk::serde::Serialize;
use calimero_sdk::{app, env};
use calimero_storage::collections::Vector;

#[app::state(emits = for<'a> Event<'a>)]
#[derive(BorshDeserialize, BorshSerialize, Default)]
#[borsh(crate = "calimero_sdk::borsh")]
pub struct OnlyPeers {
    posts: Vector<Post>,
}

#[derive(BorshDeserialize, BorshSerialize, Default, Serialize)]
#[borsh(crate = "calimero_sdk::borsh")]
#[serde(crate = "calimero_sdk::serde")]
pub struct Post {
    id: usize,
    title: String,
    content: String,
    comments: Vector<Comment>,
}

#[derive(BorshDeserialize, BorshSerialize, Clone, Default, Serialize)]
#[borsh(crate = "calimero_sdk::borsh")]
#[serde(crate = "calimero_sdk::serde")]
pub struct Comment {
    text: String,
    user: String,
}

#[app::event]
pub enum Event<'a> {
    PostCreated {
        id: usize,
        title: &'a str,
        content: &'a str,
    },
    CommentCreated {
        post_id: usize,
        user: &'a str,
        text: &'a str,
    },
}

#[app::logic]
impl OnlyPeers {
    #[app::init]
    pub fn init() -> OnlyPeers {
        OnlyPeers::default()
    }

    pub fn post(&self, id: usize) -> app::Result<Option<Post>> {
        app::log!("Getting post with id: {:?}", id);

        Ok(self.posts.get(id)?)
    }

    pub fn posts(&self) -> app::Result<Vec<Post>> {
        app::log!("Getting all posts");

        Ok(self.posts.iter()?.collect())
    }

    pub fn create_post(&mut self, title: String, content: String) -> app::Result<Post> {
        app::log!(
            "Creating post with title: {:?} and content: {:?}",
            title,
            content
        );

        app::emit!(Event::PostCreated {
            id: self.posts.len()?,
            // todo! should we maybe only emit an ID, and let notified clients fetch the post?
            title: &title,
            content: &content,
        });

        self.posts.push(Post {
            id: self.posts.len()?,
            title,
            content,
            comments: Vector::new(),
        })?;

        match self.posts.last()? {
            Some(post) => Ok(post),
            None => env::unreachable(),
        }
    }

    pub fn create_comment(
        &mut self,
        post_id: usize,
        user: String, // todo! expose executor identity to app context
        text: String,
    ) -> app::Result<Option<Comment>> {
        app::log!(
            "Creating comment under post with id: {:?} as user: {:?} with text: {:?}",
            post_id,
            user,
            text
        );

        let Some(mut post) = self.posts.get(post_id)? else {
            return Ok(None);
        };

        app::emit!(Event::CommentCreated {
            post_id,
            // todo! should we maybe only emit an ID, and let notified clients fetch the comment?
            user: &user,
            text: &text,
        });

        let comment = Comment { user, text };

        post.comments.push(comment.clone())?;

        self.posts.update(post_id, post)?;

        Ok(Some(comment))
    }
}
