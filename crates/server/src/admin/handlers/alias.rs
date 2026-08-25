use axum::routing::{get, post, Router};
use calimero_primitives::application::ApplicationId;
use calimero_primitives::context::ContextId;
use calimero_primitives::identity::DeviceId;

mod create_alias;
mod delete_alias;
mod list_aliases;
mod lookup_alias;

pub fn service() -> Router {
    let create_routes = Router::new()
        .route("/context", post(create_alias::handler::<ContextId>))
        .route("/application", post(create_alias::handler::<ApplicationId>))
        .route("/device", post(create_alias::handler::<DeviceId>));

    let lookup_routes = Router::new()
        .route("/context/:name", post(lookup_alias::handler::<ContextId>))
        .route(
            "/application/:name",
            post(lookup_alias::handler::<ApplicationId>),
        )
        .route("/device/:name", post(lookup_alias::handler::<DeviceId>));

    let delete_routes = Router::new()
        .route("/context/:name", post(delete_alias::handler::<ContextId>))
        .route(
            "/application/:name",
            post(delete_alias::handler::<ApplicationId>),
        )
        .route("/device/:name", post(delete_alias::handler::<DeviceId>));

    let list_routes = Router::new()
        .route("/context", get(list_aliases::handler::<ContextId>))
        .route("/application", get(list_aliases::handler::<ApplicationId>))
        .route("/device", get(list_aliases::handler::<DeviceId>));

    Router::new()
        .nest("/create", create_routes)
        .nest("/lookup", lookup_routes)
        .nest("/delete", delete_routes)
        .nest("/list", list_routes)
}
