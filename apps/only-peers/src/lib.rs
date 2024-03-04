use borsh::{BorshDeserialize, BorshSerialize};
use calimero_sdk::env;
use serde::Serialize;

mod code_generated_from_calimero_sdk_macros;

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

impl OnlyPeers {
    pub fn post(&self, id: usize) -> Option<&Post> {
        env::log(&format!("Getting post with id: {:?}", id));

        self.posts.get(id)
    }

    pub fn posts(&self) -> &Vec<Post> {
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

        self.posts.last().unwrap()
    }

    pub fn create_comment(
        &mut self,
        post_id: usize,
        user: String, // todo! expose executor identity to app context
        text: String,
    ) -> &Post {
        env::log(&format!(
            "Creating comment under post with id: {:?} as user: {:?} with text: {:?}",
            post, user, text
        ));

        let post = self.posts.get_mut(post_id).unwrap();

        post.comments.push(Comment { user, text });

        post
    }
}
