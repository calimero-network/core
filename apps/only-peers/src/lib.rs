use calimero_sdk::borsh::{BorshDeserialize, BorshSerialize};
use calimero_sdk::serde::Serialize;
use calimero_sdk::{app, env};

#[app::state(emits = for<'a> Event<'a>)]
#[derive(BorshDeserialize, BorshSerialize, Default)]
#[borsh(crate = "calimero_sdk::borsh")]
pub struct OnlyPeers {
    posts: Vec<Post>,
}

#[derive(BorshDeserialize, BorshSerialize, Default, Serialize)]
#[borsh(crate = "calimero_sdk::borsh")]
#[serde(crate = "calimero_sdk::serde")]
pub struct Post {
    id: usize,
    title: String,
    content: String,
    comments: Vec<Comment>,
}

#[derive(BorshDeserialize, BorshSerialize, Default, Serialize)]
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

    pub fn post(&self, id: usize) -> Option<&Post> {
        env::log(&format!("Getting post with id: {:?}", id));

        self.posts.get(id)
    }

    pub fn posts(&self) -> &[Post] {
        env::log("Getting all posts");

        &self.posts
    }

    pub fn create_post(&mut self, title: String, content: String) -> &Post {
        env::log(&format!(
            "Creating post with title: {:?} and content: {:?}",
            title, content
        ));

        app::emit!(Event::PostCreated {
            id: self.posts.len(),
            // todo! should we maybe only emit an ID, and let notified clients fetch the post?
            title: &title,
            content: &content,
        });

        self.posts.push(Post {
            id: self.posts.len(),
            title,
            content,
            comments: Vec::new(),
        });

        match self.posts.last() {
            Some(post) => post,
            None => env::unreachable(),
        }
    }

    pub fn create_comment(
        &mut self,
        post_id: usize,
        user: String, // todo! expose executor identity to app context
        text: String,
    ) -> Option<&Comment> {
        env::log(&format!(
            "Creating comment under post with id: {:?} as user: {:?} with text: {:?}",
            post_id, user, text
        ));

        let post = self.posts.get_mut(post_id)?;

        app::emit!(Event::CommentCreated {
            post_id,
            // todo! should we maybe only emit an ID, and let notified clients fetch the comment?
            user: &user,
            text: &text,
        });

        post.comments.push(Comment { user, text });

        post.comments.last()
    }
}
