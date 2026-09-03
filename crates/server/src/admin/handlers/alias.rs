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
        .route("/context/{name}", post(lookup_alias::handler::<ContextId>))
        .route(
            "/application/{name}",
            post(lookup_alias::handler::<ApplicationId>),
        )
        .route("/device/{name}", post(lookup_alias::handler::<DeviceId>));

    let delete_routes = Router::new()
        .route("/context/{name}", post(delete_alias::handler::<ContextId>))
        .route(
            "/application/{name}",
            post(delete_alias::handler::<ApplicationId>),
        )
        .route("/device/{name}", post(delete_alias::handler::<DeviceId>));

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

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use axum::Extension;
    use calimero_context_client::client::ContextClient;
    use calimero_node_primitives::client::{AliasExists, NodeClient};
    use calimero_primitives::alias::Alias;
    use calimero_primitives::identity::DeviceId;
    use calimero_store::db::InMemoryDB;
    use calimero_store::Store;
    use calimero_utils_actix::LazyRecipient;
    use tempfile::TempDir;
    use tower::ServiceExt;

    use super::service;
    use crate::{AdminState, NodeReadiness};

    /// A `NodeClient` over a fresh in-memory store, for exercising the alias
    /// CRUD the create/lookup/list/delete handlers delegate to.
    async fn device_node_client() -> (NodeClient, TempDir) {
        let store = Store::new(Arc::new(InMemoryDB::owned()));
        let (event_sender, _rx) = tokio::sync::broadcast::channel(16);
        crate::test_support::test_node_client(
            &store,
            crate::test_support::stub_node_manager(vec![]),
            event_sender,
        )
        .await
    }

    #[actix::test]
    async fn device_alias_create_lookup_delete_roundtrip() {
        let (node_client, _blob_dir) = device_node_client().await;

        let alias: Alias<DeviceId> = "laptop".parse().unwrap();
        let device_id = DeviceId::from([0x11; 32]);

        node_client
            .create_alias(alias, None, device_id)
            .expect("create device alias");
        assert_eq!(
            node_client.lookup_alias(alias, None).unwrap(),
            Some(device_id)
        );

        node_client
            .delete_alias(alias, None)
            .expect("delete device alias");
        assert_eq!(node_client.lookup_alias(alias, None).unwrap(), None);
    }

    #[actix::test]
    async fn device_alias_create_rejects_duplicate_name() {
        let (node_client, _blob_dir) = device_node_client().await;

        let alias: Alias<DeviceId> = "laptop".parse().unwrap();
        node_client
            .create_alias(alias, None, DeviceId::from([0x11; 32]))
            .expect("first create succeeds");

        let err = node_client
            .create_alias(alias, None, DeviceId::from([0x22; 32]))
            .expect_err("duplicate alias name must be rejected");
        assert!(err.downcast_ref::<AliasExists>().is_some());
    }

    #[actix::test]
    async fn device_alias_list_returns_created_entries() {
        let (node_client, _blob_dir) = device_node_client().await;

        let laptop: Alias<DeviceId> = "laptop".parse().unwrap();
        let phone: Alias<DeviceId> = "phone".parse().unwrap();
        node_client
            .create_alias(laptop, None, DeviceId::from([0x11; 32]))
            .unwrap();
        node_client
            .create_alias(phone, None, DeviceId::from([0x22; 32]))
            .unwrap();

        let mut listed: Vec<_> = node_client
            .list_aliases::<DeviceId>(None)
            .expect("list device aliases")
            .into_iter()
            .map(|(alias, value, _scope)| (alias.to_string(), value))
            .collect();
        listed.sort_by(|a, b| a.0.cmp(&b.0));

        assert_eq!(
            listed,
            vec![
                ("laptop".to_owned(), DeviceId::from([0x11; 32])),
                ("phone".to_owned(), DeviceId::from([0x22; 32])),
            ]
        );
    }

    /// Every alias route carries at most one path parameter, and the handlers
    /// must bind it. axum rejects an extractor whose arity disagrees with the
    /// matched route, so a handler asking for more parameters than the route
    /// declares answers 500 instead of running.
    #[actix::test]
    async fn alias_routes_bind_their_path_parameters() {
        let store = Store::new(Arc::new(InMemoryDB::owned()));
        let (event_sender, _rx) = tokio::sync::broadcast::channel(16);
        let (node_client, _blob_dir) = crate::test_support::test_node_client(
            &store,
            crate::test_support::stub_node_manager(vec![]),
            event_sender,
        )
        .await;
        let ctx_client =
            ContextClient::new(store.clone(), node_client.clone(), LazyRecipient::new());
        let state = Arc::new(AdminState::new(
            store,
            ctx_client,
            node_client,
            Arc::new(NodeReadiness::new()),
            #[cfg(feature = "mock-attestation")]
            false,
        ));

        for (method, uri) in [
            ("POST", "/lookup/device/laptop"),
            ("POST", "/delete/device/laptop"),
            ("GET", "/list/device"),
        ] {
            let response = service()
                .layer(Extension(Arc::clone(&state)))
                .oneshot(
                    Request::builder()
                        .method(method)
                        .uri(uri)
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();

            assert_eq!(response.status(), StatusCode::OK, "{method} {uri}");
        }
    }
}
