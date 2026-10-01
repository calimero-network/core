//! Authorization matrix rows for what a device observes through the server: the
//! SSE and WebSocket subscribe routes and the scope every listing route reads.

use std::time::Duration;

use axum::body::Body;
use axum::http::Request;
use axum::{Extension, Router};
use calimero_context_client::client::ContextClient;
use calimero_governance_store::authz_matrix::{
    assert_covered, assert_matches, Actor, ActorState, GatedOp, Home, Observed, OpTable, Outcome,
    World,
};
use calimero_node_primitives::client::NodeClient;
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
use crate::auth::{AuthenticatedAccount, AuthenticatedDevice};
use crate::config::ServerConfig;
use crate::sse::SseConfig;
use crate::ws::WsConfig;
use ActorState::*;

/// Live members of the subject, by any path, on any live device of theirs.
const SUBJECT_MEMBERS: &[ActorState] = &[
    Owner,
    DirectAdmin,
    DirectMember,
    InheritedAdmin,
    InheritedMember,
    ReadmittedAfterKick,
    SecondDevice,
];

const TABLES: &[OpTable] = &[
    OpTable {
        op: GatedOp::SseSubscribe,
        allow: SUBJECT_MEMBERS,
        // An account caller is judged by the deny-list-blind walk and its device
        // is not consulted.
        gap: &[Kicked, Left, RevokedDevice, DescopedDevice],
    },
    OpTable {
        op: GatedOp::WsSubscribe,
        allow: SUBJECT_MEMBERS,
        gap: &[Kicked, Left, RevokedDevice, DescopedDevice],
    },
    OpTable {
        op: GatedOp::ListSubgroups,
        allow: SUBJECT_MEMBERS,
        // Descendants are reached by the deny-list-blind walk, and a revocation
        // is read at each group rather than at the namespace.
        gap: &[Kicked, Left, RevokedDevice, DescopedDevice],
    },
];

const ROWS: &[GatedOp] = &[
    GatedOp::SseSubscribe,
    GatedOp::WsSubscribe,
    GatedOp::ListSubgroups,
];

async fn row(op: GatedOp, world: &World, actor: &Actor) -> Outcome {
    match op {
        GatedOp::SseSubscribe => sse_subscribe(world, actor).await,
        GatedOp::WsSubscribe => ws_subscribe(world, actor).await,
        GatedOp::ListSubgroups => list_subgroups(world, actor).await,
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
    let (node_client, ctx_client, _blob_dir) = clients(&world.store).await;
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

/// Whether the subject is in the scope every listing route filters by.
async fn list_subgroups(world: &World, actor: &Actor) -> Outcome {
    let (_node_client, ctx_client, _blob_dir) = clients(&world.store).await;
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
