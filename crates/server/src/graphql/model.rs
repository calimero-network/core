use async_graphql::{Context, InputObject, Object, SimpleObject};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::graphql;

pub struct AppQuery {
    pub sender: crate::Sender,
}

#[derive(SimpleObject, Clone, Serialize, Deserialize)]
pub struct Post {
    id: usize,
    title: String,
    content: String,
    comments: Vec<Comment>,
}

#[derive(SimpleObject, Clone, Serialize, Deserialize)]
struct Comment {
    text: String,
    user: String,
}

#[Object]
impl AppQuery {
    async fn posts<'a>(&self, _ctx: &Context<'a>) -> async_graphql::Result<Vec<Post>> {
        graphql::call(&self.sender, "posts".to_string(), b"{}".to_vec()).await
    }

    async fn post<'a>(&self, _ctx: &Context<'a>, id: i32) -> async_graphql::Result<Option<Post>> {
        graphql::call(
            &self.sender,
            "post".to_string(),
            serde_json::to_vec(&json!({ "id": id }))?,
        )
        .await
    }
}

#[derive(InputObject)]
struct CreateCommentInput {
    post_id: usize,
    user: String,
    text: String,
}

#[derive(InputObject)]
struct CreatePostInput {
    title: String,
    content: String,
}

pub struct AppMutation {
    pub sender: crate::Sender,
}

#[Object]
impl AppMutation {
    async fn create_post<'a>(
        &self,
        _ctx: &Context<'a>,
        input: CreatePostInput,
    ) -> async_graphql::Result<Post> {
        graphql::call_mut(
            &self.sender,
            "create_post".to_string(),
            serde_json::to_vec(&json!({
                "title": input.title,
                "content": input.content,
            }))?,
        )
        .await
    }

    async fn create_comment<'a>(
        &self,
        _ctx: &Context<'a>,
        input: CreateCommentInput,
    ) -> async_graphql::Result<Option<Comment>> {
        graphql::call_mut(
            &self.sender,
            "create_comment".to_string(),
            serde_json::to_vec(&json!({
                "post_id": input.post_id,
                "user": input.user,
                "text": input.text,
            }))?,
        )
        .await
    }
}
