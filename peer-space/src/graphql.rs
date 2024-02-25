use async_graphql::{Context, EmptySubscription, InputObject, Object, Schema, SimpleObject, ID};
use std::sync::Arc;
use tokio::sync::Mutex;

#[derive(SimpleObject, Clone)]
struct Comment {
    text: String,
    user: String,
}

#[derive(SimpleObject, Clone)]
struct Post {
    id: ID,
    title: String,
    content: String,
    comments: Vec<Comment>,
}

type Storage = Arc<Mutex<Vec<Post>>>;

pub struct QueryRoot;

#[Object]
impl QueryRoot {
    async fn posts<'a>(&self, ctx: &Context<'a>) -> async_graphql::Result<Vec<Post>> {
        let stored_posts = ctx.data_unchecked::<Storage>().lock().await.clone();
        Ok(stored_posts)
    }
}
#[derive(InputObject)]
struct CreateCommentInput {
    post: i32,
    user: String,
    text: String,
}

#[derive(InputObject)]
struct CreatePostInput {
    title: String,
    content: String,
}

pub struct MutationRoot;

#[Object]
impl MutationRoot {
    async fn create_post<'a>(
        &self,
        ctx: &Context<'a>,
        input: CreatePostInput,
    ) -> async_graphql::Result<Post> {
        let mut stored_posts = ctx.data_unchecked::<Storage>().lock().await;
        let post = Post {
            id: stored_posts.len().into(),
            title: input.title,
            content: input.content,
            comments: vec![],
        };
        stored_posts.push(post.clone());
        Ok(post)
    }

    async fn create_comment<'a>(
        &self,
        ctx: &Context<'a>,
        input: CreateCommentInput,
    ) -> async_graphql::Result<Post> {
        let mut stored_posts = ctx.data_unchecked::<Storage>().lock().await;
        let idx: usize = input.post as usize;
        if let Some(post) = stored_posts.get_mut(idx) {
            let comment = Comment {
                text: input.text,
                user: input.user,
            };
            post.comments.push(comment);
            Ok(post.clone())
        } else {
            Err(async_graphql::Error::new("Post not found"))
        }
    }
}

pub fn generate_schema() -> Schema<QueryRoot, MutationRoot, EmptySubscription> {
    let storage = Storage::default();
    Schema::build(QueryRoot, MutationRoot, EmptySubscription)
        .data(storage)
        .finish()
}
