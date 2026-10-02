//! Authorization matrix rows for what a device observes through the server: the
//! SSE and WebSocket subscribe routes and the scope every listing route reads.

use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::routing::get;
use axum::{Extension, Router};
use calimero_context_client::client::ContextClient;
use calimero_context_config::types::ContextGroupId;
use calimero_governance_store::authz_matrix::{
    assert_covered, assert_matches, Actor, ActorState, GatedOp, Home, Observed, OpTable, Outcome,
    World, SUBJECT_MEMBERS,
};
use calimero_node_primitives::client::NodeClient;
use calimero_server_primitives::admin::ListNamespaceGroupsApiResponse;
use calimero_store::Store;
use calimero_utils_actix::LazyRecipient;
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tempfile::TempDir;
use tokio::net::TcpListener;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;
use tower::ServiceExt;

use crate::admin::caller_scope::list_scope;
use crate::admin::handlers::namespaces::list_namespace_groups;
use crate::auth::{AuthenticatedAccount, AuthenticatedDevice};
use crate::config::ServerConfig;
use crate::sse::SseConfig;
use crate::ws::WsConfig;
use crate::{AdminState, NodeReadiness};
use ActorState::*;

const TABLES: &[OpTable] = &[
    OpTable {
        op: GatedOp::SseSubscribe,
        allow: SUBJECT_MEMBERS,
        // An account caller is judged by the deny-list-blind walk.
        gap: &[Kicked, Left],
    },
    OpTable {
        op: GatedOp::WsSubscribe,
        allow: SUBJECT_MEMBERS,
        gap: &[Kicked, Left],
    },
    OpTable {
        op: GatedOp::ListSubgroups,
        // Who may learn the Restricted subgroup exists: its members and the
        // namespace's admins.
        allow: &[
            Owner,
            NamespaceAdmin,
            InheritedAdmin,
            InheritedMember,
            Kicked,
            Left,
            ReadmittedAfterKick,
            SecondDevice,
        ],
        // The route lists every child of a namespace in scope, Restricted or not.
        gap: &[DirectAdmin, DirectMember],
    },
    OpTable {
        op: GatedOp::SubgroupInScope,
        allow: SUBJECT_MEMBERS,
        // Descendants are reached by the deny-list-blind walk.
        gap: &[Kicked, Left],
    },
];

const ROWS: &[GatedOp] = &[
    GatedOp::SseSubscribe,
    GatedOp::WsSubscribe,
    GatedOp::ListSubgroups,
    GatedOp::SubgroupInScope,
];

async fn row(op: GatedOp, world: &World, actor: &Actor) -> Outcome {
    match op {
        GatedOp::SseSubscribe => sse_subscribe(world, actor).await,
        GatedOp::WsSubscribe => ws_subscribe(world, actor).await,
        GatedOp::ListSubgroups => list_subgroups(world, actor).await,
        GatedOp::SubgroupInScope => subgroup_in_scope(world, actor).await,
        other => unreachable!("{other:?} has no row in this crate"),
    }
}

/// A node and context client over `store`; the `TempDir` holds the blob store.
async fn clients(store: &Store) -> (NodeClient, ContextClient, TempDir) {
    let (event_sender, _rx) = tokio::sync::broadcast::channel(16);
    let (node_client, blob_dir) = crate::test_support::test_node_client(
        store,
        crate::test_support::stub_node_manager(vec![]),
        event_sender,
    )
    .await;
    let ctx_client = ContextClient::new(store.clone(), node_client.clone(), LazyRecipient::new());
    (node_client, ctx_client, blob_dir)
}

fn server_config(websocket: Option<WsConfig>, sse: Option<SseConfig>) -> ServerConfig {
    ServerConfig::new(
        vec!["/ip4/127.0.0.1/tcp/2528".parse().expect("a listen address")],
        libp2p::identity::Keypair::generate_ed25519(),
        None,
        None,
        websocket,
        sse,
    )
}

/// The extensions the auth guard injects for a request that proved a device.
fn as_device(app: Router, actor: &Actor) -> Router {
    app.layer(Extension(AuthenticatedAccount(actor.account)))
        .layer(Extension(AuthenticatedDevice(actor.device)))
}

/// Open an SSE session and subscribe it to the subject's context.
async fn sse_subscribe(world: &World, actor: &Actor) -> Outcome {
    let store = world.fork();
    let (node_client, ctx_client, _blob_dir) = clients(&store).await;
    let config = server_config(None, Some(SseConfig::new(true)));
    let (path, router) =
        crate::sse::service(&config, node_client, ctx_client, store, true).expect("SSE is enabled");
    let app = as_device(Router::new().nest(path, router), actor);

    let stream = app
        .clone()
        .oneshot(Request::get("/sse").body(Body::empty()).expect("a request"))
        .await
        .expect("the SSE route answers");
    let mut frames = stream.into_body().into_data_stream();
    let first = frames
        .next()
        .await
        .expect("the connect frame")
        .expect("a readable frame");
    let session_id = String::from_utf8_lossy(&first)
        .lines()
        .find_map(|line| line.strip_prefix("data:"))
        .and_then(|data| serde_json::from_str::<Value>(data.trim()).ok())
        .and_then(|connect| connect["session_id"].as_str().map(str::to_owned))
        .expect("the connect frame names the session");

    let request = json!({
        "id": session_id,
        "method": "subscribe",
        "params": { "contextIds": [world.context] },
    });
    let response = app
        .oneshot(
            Request::post("/sse/subscription")
                .header("content-type", "application/json")
                .body(Body::from(request.to_string()))
                .expect("a request"),
        )
        .await
        .expect("the subscription route answers");
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("read the response");
    let body: Value = serde_json::from_slice(&body).expect("a JSON response");
    let contexts = body["result"]["contexts"]
        .as_array()
        .expect("the response lists the subscribed contexts");
    (!contexts.is_empty()).into()
}

/// Open a WebSocket and subscribe it to the subject's context.
async fn ws_subscribe(world: &World, actor: &Actor) -> Outcome {
    let (node_client, ctx_client, _blob_dir) = clients(&world.fork()).await;
    let config = server_config(Some(WsConfig::new(true)), None);
    let (path, route) =
        crate::ws::service(&config, node_client, ctx_client, true).expect("WebSocket is enabled");
    let app = as_device(Router::new().route(&path, route), actor);
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("a local address");
    let server = tokio::spawn(async move { axum::serve(listener, app).await });

    let (socket, _response) = connect_async(format!("ws://{addr}/ws"))
        .await
        .expect("the upgrade succeeds");
    let (mut write, mut read) = socket.split();
    let request = json!({
        "id": 1,
        "method": "subscribe",
        "params": { "contextIds": [world.context], "groupIds": [] },
    });
    write
        .send(Message::Text(request.to_string().into()))
        .await
        .expect("send the subscribe");
    let reply = tokio::time::timeout(Duration::from_secs(5), async {
        while let Some(frame) = read.next().await {
            if let Message::Text(text) = frame.expect("a readable frame") {
                let value: Value = serde_json::from_str(&text).expect("a JSON frame");
                if value["id"] == json!(1) {
                    return value;
                }
            }
        }
        panic!("the socket closed before answering the subscribe")
    })
    .await
    .expect("the subscribe is answered");
    server.abort();
    let contexts = reply["result"]["contextIds"]
        .as_array()
        .expect("the reply lists the subscribed contexts");
    (!contexts.is_empty()).into()
}

/// List the namespace's subgroups through its admin route. Allowed when the
/// listing discloses the Restricted subgroup.
async fn list_subgroups(world: &World, actor: &Actor) -> Outcome {
    let store = world.fork();
    let (node_client, ctx_client, _blob_dir) = clients(&store).await;
    let state = Arc::new(AdminState::new(
        store,
        ctx_client,
        node_client,
        Arc::new(NodeReadiness::new()),
        [0; 32],
        #[cfg(feature = "mock-attestation")]
        false,
    ));
    let app = as_device(
        Router::new()
            .route(
                "/namespaces/{namespace_id}/groups",
                get(list_namespace_groups::handler),
            )
            .layer(Extension(state)),
        actor,
    );
    let uri = format!(
        "/namespaces/{}/groups",
        hex::encode(world.namespace.to_bytes())
    );
    let response = app
        .oneshot(Request::get(uri).body(Body::empty()).expect("a request"))
        .await
        .expect("the listing route answers");
    match response.status() {
        StatusCode::NOT_FOUND => Outcome::Refuse,
        StatusCode::OK => {
            let body = axum::body::to_bytes(response.into_body(), usize::MAX)
                .await
                .expect("read the response");
            let listed: ListNamespaceGroupsApiResponse =
                serde_json::from_slice(&body).expect("a listing");
            let named = |group: ContextGroupId| {
                let id = hex::encode(group.to_bytes());
                listed.data.iter().any(|entry| entry.group_id == id)
            };
            assert!(named(world.open_chain), "a listing names the Open subgroup");
            named(world.restricted).into()
        }
        other => panic!("the listing route answered {other}"),
    }
}

/// Whether the subject is in the caller's scope, which `get_group_info`,
/// `list_group_contexts` and the context reads refuse outside of.
async fn subgroup_in_scope(world: &World, actor: &Actor) -> Outcome {
    let (_node_client, ctx_client, _blob_dir) = clients(&world.fork()).await;
    list_scope(
        &ctx_client,
        None,
        Some(&AuthenticatedAccount(actor.account)),
        Some(actor.device),
    )
    .expect("the scope resolves")
    .admits(Some(&world.subject))
    .into()
}

#[test]
fn every_operation_homed_here_has_a_row_and_a_table() {
    assert_covered(Home::Server, ROWS, TABLES);
}

#[actix::test]
async fn authorization_matrix() {
    let world = World::shared();
    let mut observed = Observed::default();
    for op in ROWS {
        for state in ActorState::ALL {
            observed.record(*op, *state, row(*op, world, world.actor(*state)).await);
        }
    }
    assert_matches("server", TABLES, &observed);
}
