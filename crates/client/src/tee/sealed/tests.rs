//! The client against the node's own sealed transport (`calimero-server`),
//! served in process, and against the wire format's published vectors.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use axum::body::{to_bytes, Body, Bytes};
use axum::extract::Request;
use axum::http::{HeaderMap, Method, StatusCode, Uri};
use axum::response::sse::{Event, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;
use calimero_server::sealed::{intercept, SealedOptions, SealedTransport as NodeTransport};
use eyre::Result;
use futures_util::StreamExt;
use serde_json::{json, Value};
use tokio::net::TcpListener;
use tower::{Layer, ServiceExt};
use url::Url;

use super::{
    finish, noise_builder, open_response, RequestHead, SealedRefusal, SealedTransport,
    StaleTransportKey, SEALED_CONTENT_TYPE,
};
use crate::connection::ConnectionInfo;
use crate::storage::JwtToken;
use crate::tee::tests::{attest_route, Attestation, Refuse, ReportData};
use crate::tee::Attestor;
use crate::traits::{ClientAuthenticator, ClientStorage};

/// A node: its transport key, sealing required, and a few routes.
struct Node {
    transport: Arc<NodeTransport>,
    router: Router,
}

impl Node {
    fn new(attest_calls: &Arc<AtomicUsize>) -> Self {
        let transport = Arc::new(NodeTransport::generate(
            &SealedOptions::new(true, None),
            &mut prometheus_client::registry::Registry::default(),
        ));
        let attestation = Attestation {
            transport_public_key: Some(transport.public_key()),
            calls: Arc::clone(attest_calls),
            ..Attestation::default()
        };
        let routes = attest_route(attestation)
            .route("/admin-api/echo", post(echo).get(echo))
            .route("/admin-api/big", get(|| async { big_body() }))
            .route("/admin-api/empty", get(|| async { StatusCode::NO_CONTENT }))
            .route(
                "/admin-api/events",
                get(|| async {
                    Sse::new(futures_util::stream::iter(["one", "two"].map(|data| {
                        Ok::<_, core::convert::Infallible>(Event::default().data(data))
                    })))
                }),
            );
        let service =
            axum::middleware::from_fn_with_state(Arc::clone(&transport), intercept).layer(routes);
        Self {
            transport,
            router: Router::new().fallback_service(service),
        }
    }
}

async fn echo(method: Method, uri: Uri, headers: HeaderMap, body: Bytes) -> impl IntoResponse {
    axum::Json(json!({
        "method": method.as_str(),
        "path": uri.to_string(),
        "authorization": headers.get("authorization").and_then(|v| v.to_str().ok()),
        "body": String::from_utf8_lossy(&body),
    }))
}

fn big_body() -> Vec<u8> {
    (0..200_000u32).map(|i| (i % 251) as u8).collect()
}

/// What sits in front of the node: forwards everything to the node running
/// now, and can restart it or flip a bit in what the node sends back.
#[derive(Clone)]
struct Front {
    node: Arc<Mutex<Router>>,
    transport_key: Arc<Mutex<[u8; 32]>>,
    tamper: Arc<AtomicBool>,
    attest_calls: Arc<AtomicUsize>,
    url: Url,
}

impl Front {
    async fn start() -> Self {
        let attest_calls = Arc::new(AtomicUsize::new(0));
        let node = Node::new(&attest_calls);
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = Url::parse(&format!("http://{}/", listener.local_addr().unwrap())).unwrap();
        let front = Self {
            node: Arc::new(Mutex::new(node.router)),
            transport_key: Arc::new(Mutex::new(node.transport.public_key())),
            tamper: Arc::new(AtomicBool::new(false)),
            attest_calls,
            url,
        };
        let forward = front.clone();
        let app = Router::new().fallback(move |request: Request| {
            let front = forward.clone();
            async move { front.forward(request).await }
        });
        drop(tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        }));
        front
    }

    async fn forward(&self, request: Request) -> Response {
        let sealed = request.uri().path() == "/sealed/v2";
        let node = self.node.lock().unwrap().clone();
        let response = node.oneshot(request).await.unwrap();
        if !(sealed && self.tamper.load(Ordering::SeqCst)) {
            return response;
        }
        let (parts, body) = response.into_parts();
        let mut bytes = to_bytes(body, usize::MAX).await.unwrap().to_vec();
        let last = bytes.len() - 1;
        bytes[last] ^= 1;
        Response::from_parts(parts, Body::from(bytes))
    }

    /// The node restarts: a new transport key, and every session forgotten.
    fn restart(&self) {
        let node = Node::new(&self.attest_calls);
        *self.transport_key.lock().unwrap() = node.transport.public_key();
        *self.node.lock().unwrap() = node.router;
    }

    fn transport_key(&self) -> [u8; 32] {
        *self.transport_key.lock().unwrap()
    }

    fn attested(&self) -> SealedTransport {
        SealedTransport::attested(
            self.url.clone(),
            reqwest::Client::new(),
            Attestor::new(ReportData),
        )
    }

    fn at(&self, path: &str) -> Url {
        self.url.join(path).unwrap()
    }
}

async fn send(
    sealed: &SealedTransport,
    request: reqwest::RequestBuilder,
) -> Result<reqwest::Response> {
    sealed.execute(request.build()?).await
}

#[tokio::test]
async fn a_sealed_request_reaches_the_route_and_its_response_opens() {
    let front = Front::start().await;
    let sealed = front.attested();
    let http = reqwest::Client::new();

    let response = send(
        &sealed,
        http.post(front.at("admin-api/echo?x=1"))
            .header("authorization", "Bearer secret")
            .body("hello"),
    )
    .await
    .unwrap();
    assert_eq!(response.status(), 200);
    let echoed: Value = response.json().await.unwrap();
    assert_eq!(echoed["method"], "POST");
    assert_eq!(echoed["path"], "/admin-api/echo?x=1");
    assert_eq!(echoed["authorization"], "Bearer secret");
    assert_eq!(echoed["body"], "hello");

    // The node requires sealing, so the same request unsealed is refused.
    let unsealed = http.get(front.at("admin-api/echo")).send().await.unwrap();
    assert_eq!(unsealed.status(), 403);
}

#[tokio::test]
async fn a_body_split_across_frames_is_put_back_together() {
    let front = Front::start().await;
    let sealed = front.attested();
    let response = send(
        &sealed,
        reqwest::Client::new().get(front.at("admin-api/big")),
    )
    .await
    .unwrap();
    assert_eq!(response.bytes().await.unwrap(), big_body());
}

#[tokio::test]
async fn an_event_stream_opens_frame_by_frame() {
    let front = Front::start().await;
    let sealed = front.attested();
    let response = send(
        &sealed,
        reqwest::Client::new().get(front.at("admin-api/events")),
    )
    .await
    .unwrap();
    assert_eq!(
        response.headers()["content-type"].to_str().unwrap(),
        "text/event-stream"
    );
    let mut events = String::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        events.push_str(&String::from_utf8_lossy(&chunk.unwrap()));
    }
    assert_eq!(events, "data: one\n\ndata: two\n\n");
}

#[tokio::test]
async fn a_response_without_a_body_has_none() {
    let front = Front::start().await;
    let sealed = front.attested();
    let response = send(
        &sealed,
        reqwest::Client::new().get(front.at("admin-api/empty")),
    )
    .await
    .unwrap();
    assert_eq!(response.status(), 204);
    assert!(response.bytes().await.unwrap().is_empty());
}

#[tokio::test]
async fn one_session_carries_many_requests() {
    let front = Front::start().await;
    let sealed = front.attested();
    let http = reqwest::Client::new();
    let requests = (0..8).map(|i| {
        let sealed = &sealed;
        let request = http.post(front.at("admin-api/echo")).body(format!("{i}"));
        async move { send(sealed, request).await.unwrap().status() }
    });
    for status in futures_util::future::join_all(requests).await {
        assert_eq!(status, 200);
    }
    assert_eq!(
        front.attest_calls.load(Ordering::SeqCst),
        1,
        "attested once, one session between them"
    );
}

#[tokio::test]
async fn a_restarted_node_is_attested_again_and_the_request_goes_through() {
    let front = Front::start().await;
    let sealed = front.attested();
    let http = reqwest::Client::new();
    let old_key = front.transport_key();

    assert_eq!(
        send(&sealed, http.get(front.at("admin-api/echo")))
            .await
            .unwrap()
            .status(),
        200
    );
    front.restart();
    assert_ne!(front.transport_key(), old_key);

    // The session is unknown to the new node, the old key is stale: the client
    // attests the new key through its verifier and resends.
    assert_eq!(
        send(&sealed, http.get(front.at("admin-api/echo")))
            .await
            .unwrap()
            .status(),
        200
    );
    assert_eq!(front.attest_calls.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn a_fixed_key_surfaces_a_restart_as_stale() {
    let front = Front::start().await;
    let sealed = SealedTransport::with_key(
        front.url.clone(),
        reqwest::Client::new(),
        front.transport_key(),
    );
    let http = reqwest::Client::new();
    assert_eq!(
        send(&sealed, http.get(front.at("admin-api/echo")))
            .await
            .unwrap()
            .status(),
        200
    );
    front.restart();
    let err = send(&sealed, http.get(front.at("admin-api/echo")))
        .await
        .unwrap_err();
    assert!(err.downcast_ref::<StaleTransportKey>().is_some(), "{err}");
}

#[tokio::test]
async fn nothing_is_sent_to_a_node_whose_quote_is_refused() {
    let front = Front::start().await;
    let sealed = SealedTransport::attested(
        front.url.clone(),
        reqwest::Client::new(),
        Attestor::new(Refuse),
    );
    let err = send(
        &sealed,
        reqwest::Client::new()
            .post(front.at("admin-api/echo"))
            .body("secret"),
    )
    .await
    .unwrap_err();
    assert!(format!("{err:#}").contains("did not verify"), "{err:#}");
}

#[tokio::test]
async fn a_tampered_response_does_not_open() {
    let front = Front::start().await;
    let sealed = front.attested();
    let http = reqwest::Client::new();
    assert!(send(&sealed, http.get(front.at("admin-api/big")))
        .await
        .is_ok());

    front.tamper.store(true, Ordering::SeqCst);
    // The head frame opens; the flipped bit is in the last frame, so the body
    // errors rather than handing anything unauthenticated on.
    let response = send(&sealed, http.get(front.at("admin-api/big"))).await;
    let body = match response {
        Ok(response) => response.bytes().await.map(|_| ()),
        Err(err) => panic!("the head should open: {err}"),
    };
    assert!(body.is_err());
}

#[tokio::test]
async fn a_request_outside_the_node_is_refused_rather_than_sent_in_the_clear() {
    let front = Front::start().await;
    let sealed = SealedTransport::attested(
        front.url.join("node/").unwrap(),
        reqwest::Client::new(),
        Attestor::new(ReportData),
    );
    let err = send(
        &sealed,
        reqwest::Client::new().get("https://elsewhere.example/node/admin-api/echo"),
    )
    .await
    .unwrap_err();
    assert!(err.to_string().contains("outside the sealed node"), "{err}");
    let err = send(
        &sealed,
        reqwest::Client::new().get(front.at("nodeish/admin-api/echo")),
    )
    .await
    .unwrap_err();
    assert!(err.to_string().contains("outside the sealed node"), "{err}");
}

#[tokio::test]
async fn a_refusal_from_in_front_of_the_node_is_reported_by_status() {
    // Something in front answers the envelope with its own error page.
    let app = Router::new().fallback(|| async { (StatusCode::BAD_GATEWAY, "upstream down") });
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = Url::parse(&format!("http://{}/", listener.local_addr().unwrap())).unwrap();
    drop(tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    }));
    let sealed = SealedTransport::with_key(url.clone(), reqwest::Client::new(), [9; 32]);
    let err = send(
        &sealed,
        reqwest::Client::new().get(url.join("admin-api/echo").unwrap()),
    )
    .await
    .unwrap_err();
    let refusal = err.downcast_ref::<SealedRefusal>().unwrap();
    assert_eq!(refusal.status, 502);
    assert_eq!(refusal.code, None);
}

#[derive(Clone)]
struct Tokens;

#[async_trait]
impl ClientStorage for Tokens {
    async fn load_tokens(&self, _: &str) -> Result<Option<JwtToken>> {
        Ok(Some(JwtToken::new("secret-token".to_owned())))
    }

    async fn save_tokens(&self, _: &str, _: &JwtToken) -> Result<()> {
        Ok(())
    }
}

#[derive(Clone)]
struct NoAuth;

#[async_trait]
impl ClientAuthenticator for NoAuth {
    async fn authenticate(&self, _: &Url) -> Result<JwtToken> {
        eyre::bail!("not in these tests")
    }

    async fn refresh_tokens(&self, _: &str) -> Result<JwtToken> {
        eyre::bail!("not in these tests")
    }

    async fn handle_auth_failure(&self, _: &Url) -> Result<JwtToken> {
        eyre::bail!("not in these tests")
    }

    async fn check_auth_required(&self, _: &Url) -> Result<bool> {
        Ok(true)
    }

    fn get_auth_method(&self) -> &'static str {
        "none"
    }
}

#[tokio::test]
async fn a_connection_seals_everything_it_sends_its_token_included() {
    let front = Front::start().await;
    let connection =
        ConnectionInfo::new(front.url.clone(), Some("node".to_owned()), NoAuth, Tokens)
            .with_sealed_transport(front.attested());

    // The node refuses unsealed requests, so this answering at all means it
    // was sealed; the token rode inside the envelope.
    let echoed: Value = connection.get("admin-api/echo").await.unwrap();
    assert_eq!(echoed["authorization"], "Bearer secret-token");
    let echoed: Value = connection
        .post("admin-api/echo", json!({"a": 1}))
        .await
        .unwrap();
    assert_eq!(echoed["body"], "{\"a\":1}");

    assert!(
        connection.auth_header().await.is_err(),
        "no token for a WebSocket, which cannot be sealed"
    );
}

// The published vectors, repeated from `calimero-server`'s sealed tests: the
// node's side and this one are separate implementations of one wire format.
const TRANSPORT_PUBLIC_VECTOR: &str =
    "0faa684ed28867b97f4a6a2dee5df8ce974e76b7018e3f22a1c4cf2678570f20";
const HANDSHAKE_REQUEST_VECTOR: &str = "020faa684ed28867b97f4a6a2dee5df8ce974e76b7018e3f22a1c4cf2678570f207b0d47d93427f8311160781c7c733fd89f88970aef490d8aa0ee19a4cb8a1b1461fba335b4f3e9be8a34a61900e5b4be";
const HANDSHAKE_RESPONSE_VECTOR: &str = "02ff2ee45601ec1b67310c7790404585ae697331eee1c1f8cf2419731c1fff3e6b7d6339b7296bdc4778248f57126a10ee5b7fb39b0cb104dcbfcaaaf44d0e6017379638a4";
const SEALED_REQUEST_VECTOR: &str = "025555555555555555555555555555555500000000000000019fa4301264a48bdf6811cfb1d7c3dad713ede5ea1820608358ff2779cee1605efc3d97879f24d6cf302a7877beda552ca6714ef9d5496f978fdcdde522435cf663e4be5f60170e8eab74161a913608fe268e5352fcd3090d62e2a12c92b06b51885d43c4d83dcfe544";
const SEALED_RESPONSE_VECTOR: &str = "0000004f4873ec5a589a1e3585360440100bc00d20522fe35bac2221dcfe80341ce3069481d4794b801776f4a76188b81887eb83cfccd2981229af48a3820df924261b3ff12d4b29340ca07c392ed4e0ee4bed0000001cfa20a7633655179808da19e2fcbc81bc219262fc4f5406582834f549000000111426897864d7a92eec9261fe2993415592";
const CLIENT_EPHEMERAL: [u8; 32] = [0x33; 32];

#[tokio::test]
async fn the_wire_format_matches_the_published_vectors() {
    let transport: [u8; 32] = hex::decode(TRANSPORT_PUBLIC_VECTOR)
        .unwrap()
        .try_into()
        .unwrap();
    let mut noise = noise_builder()
        .unwrap()
        .remote_public_key(&transport)
        .fixed_ephemeral_key_for_testing_only(&CLIENT_EPHEMERAL)
        .build_initiator()
        .unwrap();
    let mut message_1 = [0u8; 64];
    let len = noise.write_message(&[], &mut message_1).unwrap();
    let handshake = [&[2u8][..], &transport, &message_1[..len]].concat();
    assert_eq!(hex::encode(handshake), HANDSHAKE_REQUEST_VECTOR);

    let session = finish(&mut noise, &hex::decode(HANDSHAKE_RESPONSE_VECTOR).unwrap()).unwrap();
    let (request_id, header, envelope) = session
        .seal(
            &RequestHead {
                method: "POST".to_owned(),
                path: "/jsonrpc".to_owned(),
                headers: vec![("content-type".to_owned(), "application/json".to_owned())],
            },
            b"{}",
        )
        .unwrap();
    assert_eq!(request_id, 1);
    assert_eq!(hex::encode(envelope), SEALED_REQUEST_VECTOR);

    let sealed_response = http::Response::builder()
        .header("content-type", SEALED_CONTENT_TYPE)
        .body(reqwest::Body::from(
            hex::decode(SEALED_RESPONSE_VECTOR).unwrap(),
        ))
        .unwrap();
    let response = open_response(
        reqwest::Response::from(sealed_response),
        Arc::new(session),
        header,
        request_id,
    )
    .await
    .unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(response.bytes().await.unwrap(), &b"{\"ok\":true}"[..]);
}
