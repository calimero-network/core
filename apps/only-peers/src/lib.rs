use calimero_sdk::borsh::{BorshDeserialize, BorshSerialize};
use calimero_sdk::serde::Serialize;
use calimero_sdk::{app, env};

#[app::state]
#[derive(Default, BorshSerialize, BorshDeserialize)]
struct OnlyPeers {
    posts: Vec<Post>,
}

#[derive(Default, Serialize, BorshSerialize, BorshDeserialize)]
struct Post {
    id: usize,
    title: String,
    content: String,
    comments: Vec<Comment>,
}

#[derive(Default, Serialize, BorshSerialize, BorshDeserialize)]
struct Comment {
    text: String,
    user: String,
}

#[app::logic]
impl OnlyPeers {
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

        post.comments.push(Comment { user, text });

        post.comments.last()
    }
}
