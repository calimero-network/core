use axum::routing::{post, Router};
use calimero_primitives::application::ApplicationId;
use calimero_primitives::context::ContextId;
use calimero_primitives::identity::PublicKey;

mod create_alias;
mod delete_alias;
mod lookup_alias;

pub fn service() -> Router {
    let create_routes = Router::new()
        .route("/context", post(create_alias::handler::<ContextId>))
        .route("/application", post(create_alias::handler::<ApplicationId>))
        .route(
            "/identity/:context",
            post(create_alias::handler::<PublicKey>),
        );

    let lookup_routes = Router::new()
        .route("/context/:name", post(lookup_alias::handler::<ContextId>))
        .route(
            "/application/:name",
            post(lookup_alias::handler::<ApplicationId>),
        )
        .route(
            "/identity/:context/:name",
            post(lookup_alias::handler::<PublicKey>),
        );

    let delete_routes = Router::new()
        .route("/context/:name", post(delete_alias::handler::<ContextId>))
        .route(
            "/application/:name",
            post(delete_alias::handler::<ApplicationId>),
        )
        .route(
            "/identity/:context/:name",
            post(delete_alias::handler::<PublicKey>),
        );

    Router::new()
        .nest("/create", create_routes)
        .nest("/lookup", lookup_routes)
        .nest("/delete", delete_routes)
}
