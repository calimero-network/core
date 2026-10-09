use core::time::Duration;

use axum::http::header::{AUTHORIZATION, ORIGIN};
use axum::response::sse::{Event, Sse};
use axum::routing::{get, post};
use axum::Router;
use futures_util::stream as futures_stream;
use tower::{Layer, ServiceExt};

use super::session::{builder, respond_with, MAX_SESSIONS, PROLOGUE};
use super::*;
use crate::proxy_identity;

const TRANSPORT_SECRET: [u8; 32] = [0x22; 32];
const MIB: usize = 1024 * 1024;
const NEAR_MAX: usize = MAX_SEALED_BYTES - MIB; // a body each request alone may send

fn transport() -> Arc<SealedTransport> {
    transport_with(&SealedOptions::default())
}

fn transport_with(options: &SealedOptions) -> Arc<SealedTransport> {
    Arc::new(SealedTransport::from_secret(
        Zeroizing::new(TRANSPORT_SECRET),
        options,
        SealedMetrics::default(),
    ))
}

fn public_of(secret: &[u8; 32]) -> [u8; 32] {
    MontgomeryPoint::mul_base_clamped(*secret).0
}

async fn echo(headers: HeaderMap, uri: Uri, body: Bytes) -> impl IntoResponse {
    let header = |name: HeaderName| {
        headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .unwrap_or("none")
            .to_owned()
    };
    (
        [
            ("x-echo-auth", header(AUTHORIZATION)),
            ("x-echo-uri", uri.to_string()),
            ("x-echo-host", header(HOST)),
            ("x-echo-origin", header(ORIGIN)),
            (
                "x-echo-account",
                header(proxy_identity::ACCOUNT_HEADER.clone()),
            ),
            (
                "x-echo-permissions",
                header(HeaderName::from_static("x-auth-permissions")),
            ),
        ],
        body,
    )
}

fn app(
    transport: Arc<SealedTransport>,
) -> impl tower::Service<Request, Response = Response, Error = core::convert::Infallible, Future: Send>
{
    let router = Router::new()
        .route("/echo", post(echo))
        .route("/node/echo", post(echo))
        .route("/admin-api/health", get(|| async { "alive" }))
        .route("/admin-api/contexts", get(echo).delete(echo))
        .route("/admin-api/contexts/{context_id}/intents", post(echo))
        .route("/admin-api/groups/{group_id}/context-intents", post(echo))
        .route(
            "/admin-api/groups/{group_id}/governance-intents",
            post(echo),
        )
        .route("/admin-api/groups/{group_id}/contexts", post(echo))
        .route(
            "/huge-head",
            get(|| async { [("x-huge", "h".repeat(MAX_FRAME_DATA + 1))] }),
        )
        .route(
            "/stream",
            get(|| async {
                Sse::new(futures_stream::iter(["one", "two"].map(|data| {
                    Ok::<_, core::convert::Infallible>(Event::default().data(data))
                })))
            }),
        )
        .route("/big", get(|| async { vec![7u8; 3 * MAX_FRAME_DATA + 5] }));
    axum::middleware::from_fn_with_state(transport, intercept).layer(router)
}

async fn send(transport: &Arc<SealedTransport>, path: &str, body: Vec<u8>) -> (StatusCode, Bytes) {
    let response = app(Arc::clone(transport))
        .oneshot(
            Request::post(path)
                .header(HOST, "tee-node.example")
                .header(CONTENT_TYPE, SEALED_CONTENT_TYPE)
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    (
        status,
        to_bytes(response.into_body(), usize::MAX).await.unwrap(),
    )
}

async fn get_unsealed(transport: &Arc<SealedTransport>, path: &str) -> (StatusCode, Bytes) {
    let response = app(Arc::clone(transport))
        .oneshot(Request::get(path).body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    (
        status,
        to_bytes(response.into_body(), usize::MAX).await.unwrap(),
    )
}

fn error_code(body: &[u8]) -> String {
    let value: serde_json::Value = serde_json::from_slice(body).unwrap();
    value["error"]["code"].as_str().unwrap().to_owned()
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

/// Noise message 1 from a fresh initiator to `transport_public`, and the
/// initiator to finish the handshake with.
fn message_1(transport_public: &[u8; 32], payload: &[u8]) -> (Vec<u8>, snow::HandshakeState) {
    let mut initiator = builder()
        .remote_public_key(transport_public)
        .build_initiator()
        .unwrap();
    let mut message = vec![0u8; 96];
    let len = initiator.write_message(payload, &mut message).unwrap();
    message.truncate(len);
    (message, initiator)
}

fn handshake_body(transport_public: &[u8; 32], message_1: &[u8]) -> Vec<u8> {
    [&[VERSION][..], transport_public, message_1].concat()
}

/// The client's side of a session, as mero-js does it.
struct Client {
    session_id: SessionId,
    keys: SessionKeys,
    next_request_id: u64,
}

impl Client {
    async fn open(transport: &Arc<SealedTransport>) -> Self {
        Self::open_at(transport, "").await
    }

    async fn open_at(transport: &Arc<SealedTransport>, mount: &str) -> Self {
        let (message, mut initiator) = message_1(&transport.public_key(), &[]);
        let (status, body) = send(
            transport,
            &format!("{mount}{HANDSHAKE_PATH}"),
            handshake_body(&transport.public_key(), &message),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        Self::finish(&mut initiator, &body)
    }

    fn finish(initiator: &mut snow::HandshakeState, response: &[u8]) -> Self {
        assert_eq!(response[0], VERSION);
        let mut payload = [0u8; 64];
        let len = initiator
            .read_message(&response[1..], &mut payload)
            .unwrap();
        assert_eq!(len, SESSION_ID_LEN + 4);
        let lifetime = u32::from_be_bytes(payload[SESSION_ID_LEN..len].try_into().unwrap());
        assert_eq!(u64::from(lifetime), SESSION_LIFETIME.as_secs());
        let (request, response) = initiator.dangerously_get_raw_split();
        Self {
            session_id: payload[..SESSION_ID_LEN].try_into().unwrap(),
            keys: SessionKeys {
                request: Zeroizing::new(request),
                response: Zeroizing::new(response),
            },
            next_request_id: 1,
        }
    }

    fn header(&self, request_id: u64) -> [u8; HEADER_LEN] {
        let mut header = [0u8; HEADER_LEN];
        header[0] = VERSION;
        header[1..=SESSION_ID_LEN].copy_from_slice(&self.session_id);
        header[1 + SESSION_ID_LEN..].copy_from_slice(&request_id.to_be_bytes());
        header
    }

    /// Seal a request under the next id, returning the id and the envelope.
    fn seal(&mut self, head: &RequestHead, body: &[u8]) -> (u64, Vec<u8>) {
        let request_id = self.next_request_id;
        self.next_request_id += 1;
        (request_id, self.seal_as(request_id, head, body))
    }

    fn seal_as(&self, request_id: u64, head: &RequestHead, body: &[u8]) -> Vec<u8> {
        let header = self.header(request_id);
        let head = serde_json::to_vec(head).unwrap();
        let mut buffer = u32::try_from(head.len()).unwrap().to_be_bytes().to_vec();
        buffer.extend_from_slice(&head);
        buffer.extend_from_slice(body);
        aead_key(&self.keys.request)
            .seal_in_place_append_tag(nonce(request_id, 0), Aad::from(&header), &mut buffer)
            .unwrap();
        [&header[..], &buffer].concat()
    }

    /// Open a sealed response: its head, its body, and whether it ended with
    /// an end frame. Panics on any frame that does not open, in order.
    fn open_response(&self, request_id: u64, sealed: &[u8]) -> (ResponseHead, Vec<u8>, bool) {
        let key = aead_key(&self.keys.response);
        let header = self.header(request_id);
        let mut rest = sealed;
        let mut index = 0u32;
        let mut head = None;
        let mut body = Vec::new();
        let mut ended = false;
        while !rest.is_empty() {
            let (len, tail) = rest.split_first_chunk::<4>().unwrap();
            let (frame, tail) = tail.split_at(usize::try_from(u32::from_be_bytes(*len)).unwrap());
            rest = tail;
            let mut frame = frame.to_vec();
            let plain = key
                .open_in_place(nonce(request_id, index), Aad::from(&header), &mut frame)
                .expect("every frame opens, in order");
            index += 1;
            assert!(plain.len() <= 1 + MAX_FRAME_DATA);
            match plain[0] {
                FRAME_HEAD => head = Some(serde_json::from_slice(&plain[1..]).unwrap()),
                FRAME_DATA => body.extend_from_slice(&plain[1..]),
                FRAME_END => ended = true,
                kind => panic!("unknown frame kind {kind}"),
            }
        }
        (head.expect("a head frame"), body, ended)
    }
}

fn head(method: &str, path: &str, headers: &[(&str, &str)]) -> RequestHead {
    RequestHead {
        method: method.to_owned(),
        path: path.to_owned(),
        headers: headers
            .iter()
            .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
            .collect(),
    }
}

fn response_header<'a>(head: &'a ResponseHead, name: &str) -> Option<&'a str> {
    head.headers
        .iter()
        .find(|(n, _)| n == name)
        .map(|(_, v)| v.as_str())
}

#[tokio::test]
async fn a_sealed_request_reaches_the_router_and_its_response_comes_back_sealed() {
    let transport = transport();
    let mut client = Client::open(&transport).await;
    let (id, sealed) = client.seal(
        &head(
            "POST",
            "/echo?x=1",
            &[
                ("authorization", "Bearer secret-token"),
                ("host", "forged.example"),
            ],
        ),
        b"{\"hello\":1}",
    );

    let (status, body) = send(&transport, SEALED_PATH, sealed).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        !contains(&body, b"secret-token") && !contains(&body, b"hello"),
        "nothing of the exchange is readable on the wire"
    );

    let (head, inner, ended) = client.open_response(id, &body);
    assert!(ended);
    assert_eq!(head.status, 200);
    assert_eq!(inner, b"{\"hello\":1}");
    assert_eq!(
        response_header(&head, "x-echo-auth"),
        Some("Bearer secret-token")
    );
    assert_eq!(response_header(&head, "x-echo-uri"), Some("/echo?x=1"));
    assert_eq!(
        response_header(&head, "x-echo-host"),
        Some("tee-node.example"),
        "the host auth checks a node-bound token against is the outer hop's"
    );
}

async fn echoed_origin(stated: &[(&str, &str)], outer_origin: Option<&str>) -> String {
    let transport = transport();
    let mut client = Client::open(&transport).await;
    let (id, sealed) = client.seal(&head("POST", "/echo", stated), b"");
    let mut request = Request::post(SEALED_PATH)
        .header(HOST, "tee-node.example")
        .header(CONTENT_TYPE, SEALED_CONTENT_TYPE);
    if let Some(origin) = outer_origin {
        request = request.header(ORIGIN, origin);
    }
    let response = app(Arc::clone(&transport))
        .oneshot(request.body(Body::from(sealed)).unwrap())
        .await
        .unwrap();
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let (head, _, _) = client.open_response(id, &body);
    response_header(&head, "x-echo-origin").unwrap().to_owned()
}

#[tokio::test]
async fn a_sealed_request_carries_the_page_origin_of_the_outer_hop() {
    let origin = echoed_origin(
        &[("origin", "http://localhost")],
        Some("https://app.example"),
    )
    .await;
    assert_eq!(origin, "https://app.example");
}

#[tokio::test]
async fn a_sealed_request_from_a_hop_without_an_origin_has_none() {
    let origin = echoed_origin(&[("origin", "http://localhost")], None).await;
    assert_eq!(origin, "none");
}

#[tokio::test]
async fn an_unsealed_request_passes_by_untouched() {
    let response = app(transport())
        .oneshot(Request::post("/echo").body(Body::from("plain")).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    assert_eq!(&body[..], b"plain");
}

/// The point of the session: the transport key authenticates the node but is
/// not enough to open what was sent. An attacker who recorded a handshake and
/// a request and later took the transport key can only run the handshake
/// again, which gets a new ephemeral key from the node and so other keys.
#[tokio::test]
async fn the_transport_key_alone_does_not_open_a_recorded_session() {
    let transport = transport();
    let (recorded_message_1, mut initiator) = message_1(&transport.public_key(), &[]);
    let (_, recorded_message_2) = send(
        &transport,
        HANDSHAKE_PATH,
        handshake_body(&transport.public_key(), &recorded_message_1),
    )
    .await;
    let mut client = Client::finish(&mut initiator, &recorded_message_2);
    let (id, recorded_request) = client.seal(&head("POST", "/echo", &[]), b"the secret");

    let (_, replayed_keys) =
        session::respond(&TRANSPORT_SECRET, &recorded_message_1, &client.session_id).unwrap();
    let envelope = RequestEnvelope::parse(&recorded_request).unwrap();
    assert_eq!(envelope.request_id, id);
    assert!(
        open_request(&replayed_keys, &envelope, &mut recorded_request.clone()).is_err(),
        "the transport key and the recorded handshake do not rebuild the session keys"
    );
    assert!(open_request(&client.keys, &envelope, &mut recorded_request.clone()).is_ok());
}

/// A restarted node holds a new key. It must say so rather than fail opaquely,
/// so the client knows to attest again.
#[tokio::test]
async fn a_handshake_to_another_transport_key_is_refused_as_stale() {
    let other = public_of(&[0x55; 32]);
    let (message, _) = message_1(&other, &[]);
    let (status, body) = send(
        &transport(),
        HANDSHAKE_PATH,
        handshake_body(&other, &message),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(error_code(&body), "stale_transport_key");
}

/// Data sent before the ephemeral exchange would be under the static key
/// alone, without forward secrecy.
#[tokio::test]
async fn a_handshake_carrying_data_is_refused() {
    let transport = transport();
    let (message, _) = message_1(&transport.public_key(), b"early");
    let (status, body) = send(
        &transport,
        HANDSHAKE_PATH,
        handshake_body(&transport.public_key(), &message),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(error_code(&body), "malformed");
}

#[tokio::test]
async fn a_request_in_no_open_session_is_refused_as_unknown() {
    let transport = transport();
    let mut client = Client::open(&transport).await;
    client.session_id = [0x99; SESSION_ID_LEN];
    let (_, sealed) = client.seal(&head("POST", "/echo", &[]), b"");
    let (status, body) = send(&transport, SEALED_PATH, sealed).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(error_code(&body), "unknown_session");
}

/// What a server has pulled from a body, and how much of it it holds at once.
#[derive(Default)]
struct Pulled {
    total: core::sync::atomic::AtomicUsize,
    live: core::sync::atomic::AtomicUsize,
    peak: core::sync::atomic::AtomicUsize,
}

impl Pulled {
    fn total(&self) -> usize {
        self.total.load(core::sync::atomic::Ordering::SeqCst)
    }

    fn peak(&self) -> usize {
        self.peak.load(core::sync::atomic::Ordering::SeqCst)
    }
}

/// A chunk that counts itself out of `live` when the server drops it. It counts
/// handles kept alive, not copies the server makes.
struct Held {
    data: Vec<u8>,
    pulled: Arc<Pulled>,
}

impl AsRef<[u8]> for Held {
    fn as_ref(&self) -> &[u8] {
        &self.data
    }
}

impl Drop for Held {
    fn drop(&mut self) {
        let _previous = self
            .pulled
            .live
            .fetch_sub(self.data.len(), core::sync::atomic::Ordering::SeqCst);
    }
}

fn held(pulled: &Arc<Pulled>, data: Vec<u8>) -> Bytes {
    use core::sync::atomic::Ordering::SeqCst;

    let _previous = pulled.total.fetch_add(data.len(), SeqCst);
    let live = pulled.live.fetch_add(data.len(), SeqCst) + data.len();
    let _previous = pulled.peak.fetch_max(live, SeqCst);
    Bytes::from_owner(Held {
        data,
        pulled: Arc::clone(pulled),
    })
}

/// `prefix`, then `chunks` more of `chunk_len` zeroes, produced as the server
/// polls for them.
fn counted_body(prefix: Vec<u8>, chunks: usize, chunk_len: usize) -> (Body, Arc<Pulled>) {
    let pulled = Arc::new(Pulled::default());
    let counter = Arc::clone(&pulled);
    let tail = futures_stream::iter(0..chunks).map(move |_| vec![0u8; chunk_len]);
    let body = futures_stream::iter([prefix])
        .chain(tail)
        .map(move |chunk| Ok::<_, core::convert::Infallible>(held(&counter, chunk)));
    (Body::from_stream(body), pulled)
}

async fn send_body(
    transport: &Arc<SealedTransport>,
    body: Body,
    declared_len: Option<usize>,
) -> (StatusCode, Bytes) {
    let mut request = Request::post(SEALED_PATH)
        .header(HOST, "tee-node.example")
        .header(CONTENT_TYPE, SEALED_CONTENT_TYPE);
    if let Some(len) = declared_len {
        request = request.header(CONTENT_LENGTH, len);
    }
    let response = app(Arc::clone(transport))
        .oneshot(request.body(body).unwrap())
        .await
        .unwrap();
    let status = response.status();
    (
        status,
        to_bytes(response.into_body(), usize::MAX).await.unwrap(),
    )
}

#[tokio::test]
async fn a_body_in_no_open_session_is_read_to_the_end_but_not_held() {
    let transport = transport();
    let mut client = Client::open(&transport).await;
    client.session_id = [0x99; SESSION_ID_LEN];
    let (_, sealed) = client.seal(&head("POST", "/echo", &[]), b"");
    let prefix = sealed.len();
    let (body, pulled) = counted_body(sealed, 16, MIB);

    let (status, body) = send_body(&transport, body, None).await;

    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(error_code(&body), "unknown_session");
    assert_eq!(
        pulled.total(),
        prefix + 16 * MIB,
        "the refusal waits for the upload, so a client still sending is not reset"
    );
    assert!(
        pulled.peak() <= 2 * MIB,
        "a stranger's body was held: {} bytes at once",
        pulled.peak()
    );
}

#[tokio::test]
async fn a_body_declared_over_the_limit_is_refused_without_reading_it() {
    let transport = transport();
    let mut client = Client::open(&transport).await;
    let (_, sealed) = client.seal(&head("POST", "/echo", &[]), b"");
    let (body, pulled) = counted_body(sealed, 256, MIB);

    let (status, body) = send_body(&transport, body, Some(MAX_SEALED_BYTES + 1)).await;

    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(error_code(&body), "too_large");
    assert_eq!(pulled.total(), 0);
}

#[tokio::test]
async fn a_body_that_outgrows_the_limit_is_refused_as_it_arrives() {
    let transport = transport();
    let mut client = Client::open(&transport).await;
    let (_, sealed) = client.seal(&head("POST", "/echo", &[]), b"");
    let (body, pulled) = counted_body(sealed, 80, MIB);

    let (status, body) = send_body(&transport, body, None).await;

    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(error_code(&body), "too_large");
    assert!(
        pulled.total() <= MAX_SEALED_BYTES + MIB,
        "read {} bytes past a {MAX_SEALED_BYTES} byte limit",
        pulled.total()
    );
}

/// Requests that each send the most one request may and then stall, as many as
/// the node-wide budget holds, returned once the node has read all of them.
async fn fill_the_budget(
    transport: &Arc<SealedTransport>,
    client: &mut Client,
) -> Vec<tokio::task::JoinHandle<(StatusCode, Bytes)>> {
    let mut held = Vec::new();
    for _ in 0..MAX_SEALED_IN_FLIGHT / MAX_SEALED_BYTES {
        let (_, mut prefix) = client.seal(&head("POST", "/echo", &[]), b"");
        prefix.resize(MIB, 0);
        let (body, pulled) = counted_body(prefix, MAX_SEALED_BYTES / MIB - 1, MIB);
        let stalled = body
            .into_data_stream()
            .chain(futures_stream::pending::<Result<Bytes, axum::Error>>());
        let transport = Arc::clone(transport);
        held.push(tokio::spawn(async move {
            send_body(&transport, Body::from_stream(stalled), None).await
        }));
        tokio::time::timeout(Duration::from_secs(60), async {
            while pulled.total() < MAX_SEALED_BYTES {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("a request the budget has room for is read");
    }
    held
}

#[tokio::test]
async fn requests_past_the_node_wide_budget_are_refused_though_each_is_within_its_own_limit() {
    let transport = transport();
    let mut client = Client::open(&transport).await;
    let held = fill_the_budget(&transport, &mut client).await;

    let (_, prefix) = client.seal(&head("POST", "/echo", &[]), b"");
    let sent = prefix.len() + NEAR_MAX;
    let (body, pulled) = counted_body(prefix, NEAR_MAX / MIB, MIB);
    let (status, body) = send_body(&transport, body, None).await;

    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(error_code(&body), "busy");
    assert_eq!(
        pulled.total(),
        sent,
        "the refusal waits for the upload, so a client still sending is not reset"
    );
    assert!(
        held.iter().all(|request| !request.is_finished()),
        "the requests already held are not the ones refused"
    );

    let mut expired = Client::open(&transport).await;
    expired.session_id = [0x99; SESSION_ID_LEN];
    let (_, sealed) = expired.seal(&head("POST", "/echo", &[]), b"");
    let chunks = sealed
        .chunks(7)
        .map(|chunk| Ok::<_, core::convert::Infallible>(Bytes::copy_from_slice(chunk)))
        .collect::<Vec<_>>();
    let body = Body::from_stream(futures_stream::iter(chunks));
    let (status, body) = send_body(&transport, body, None).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(
        error_code(&body),
        "unknown_session",
        "a full budget does not hide the refusal its client renews on"
    );
    held.iter().for_each(tokio::task::JoinHandle::abort);
}

#[tokio::test]
async fn the_budget_comes_back_however_a_request_ends() {
    let transport = transport();
    let mut client = Client::open(&transport).await;
    // More than the budget holds at once, so a request that kept its share
    // would get the last of them refused.
    let past_the_budget = MAX_SEALED_IN_FLIGHT / NEAR_MAX + 1;

    for request in fill_the_budget(&transport, &mut client).await {
        request.abort();
        let _cancelled = request.await;
    }

    // Refused for naming no open session, then stalled while it is discarded.
    let (_, mut prefix) = client.seal(&head("POST", "/echo", &[]), b"");
    prefix[1..=SESSION_ID_LEN].fill(0x99);
    let (body, _) = counted_body(prefix, NEAR_MAX / MIB, MIB);
    let stalled = body
        .into_data_stream()
        .chain(futures_stream::pending::<Result<Bytes, axum::Error>>());
    let stranger = {
        let transport = Arc::clone(&transport);
        tokio::spawn(async move { send_body(&transport, Body::from_stream(stalled), None).await })
    };
    let held = fill_the_budget(&transport, &mut client).await;
    let (_, sealed) = client.seal(&head("POST", "/echo", &[]), b"");
    let (status, _) = send(&transport, SEALED_PATH, sealed).await;
    assert_eq!(
        status,
        StatusCode::SERVICE_UNAVAILABLE,
        "the budget is full, so the refused request kept none of it"
    );
    for request in held {
        request.abort();
        let _cancelled = request.await;
    }
    stranger.abort();

    for _ in 0..past_the_budget {
        let (_, prefix) = client.seal(&head("POST", "/echo", &[]), b"");
        let (body, _) = counted_body(prefix, NEAR_MAX / MIB, MIB);
        let cut = body
            .into_data_stream()
            .chain(futures_stream::iter([Err(axum::Error::new(
                "the client went away",
            ))]));
        let (status, body) = send_body(&transport, Body::from_stream(cut), None).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(error_code(&body), "malformed");
    }

    for _ in 0..past_the_budget {
        let (_, sealed) = client.seal(&head("POST", "/echo", &[]), &vec![0; NEAR_MAX]);
        let (status, _) = send_body(&transport, Body::from(sealed), None).await;
        assert_eq!(status, StatusCode::OK);
    }
}

#[tokio::test]
async fn a_request_head_larger_than_a_frame_is_refused() {
    let transport = transport();
    let mut client = Client::open(&transport).await;
    let huge = "h".repeat(MAX_FRAME_DATA);
    let (_, sealed) = client.seal(&head("POST", "/echo", &[("x-huge", huge.as_str())]), b"");

    let (status, body) = send(&transport, SEALED_PATH, sealed).await;

    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(error_code(&body), "malformed");
}

#[tokio::test(start_paused = true)]
async fn a_body_that_stops_arriving_gives_its_share_back() {
    let transport = transport();
    let mut client = Client::open(&transport).await;

    for request in fill_the_budget(&transport, &mut client).await {
        let (status, body) = tokio::time::timeout(2 * SEALED_READ_TIME, request)
            .await
            .expect("a stalled body is refused, not waited on forever")
            .unwrap();
        assert_eq!(status, StatusCode::REQUEST_TIMEOUT);
        assert_eq!(error_code(&body), "timeout");
    }

    let (_, sealed) = client.seal(&head("POST", "/echo", &[]), &vec![0; NEAR_MAX]);
    let (status, _) = send_body(&transport, Body::from(sealed), None).await;
    assert_eq!(status, StatusCode::OK);
}

// The client renews its session only on this refusal, so a large body must still
// receive it rather than a reset.
#[tokio::test]
async fn a_large_body_in_no_open_session_still_gets_the_refusal_over_a_socket() {
    let transport = transport();
    let mut client = Client::open(&transport).await;
    client.session_id = [0x99; SESSION_ID_LEN];
    let (_, mut sealed) = client.seal(&head("POST", "/echo", &[]), b"");
    sealed.resize(20 * MIB, 0);

    let router = Router::new().route("/echo", post(echo));
    let service =
        axum::middleware::from_fn_with_state(Arc::clone(&transport), intercept).layer(router);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            axum::ServiceExt::<Request>::into_make_service(service),
        )
        .await
    });
    let http = reqwest::Client::builder().no_proxy().build().unwrap();

    for _ in 0..3 {
        let response = http
            .post(format!("http://{addr}{SEALED_PATH}"))
            .header(CONTENT_TYPE, SEALED_CONTENT_TYPE)
            .body(sealed.clone())
            .send()
            .await
            .expect("the refusal reaches the client, not a reset");
        assert_eq!(response.status(), StatusCode::CONFLICT);
        assert_eq!(
            error_code(&response.bytes().await.unwrap()),
            "unknown_session"
        );
    }
    server.abort();
}

#[tokio::test]
async fn a_request_sent_in_small_chunks_still_opens() {
    let transport = transport();
    let mut client = Client::open(&transport).await;
    let (id, sealed) = client.seal(&head("POST", "/echo", &[]), b"payload");
    let chunks = sealed
        .chunks(7)
        .map(|chunk| Ok::<_, core::convert::Infallible>(Bytes::copy_from_slice(chunk)))
        .collect::<Vec<_>>();
    let body = Body::from_stream(futures_stream::iter(chunks));

    let (status, response) = send_body(&transport, body, None).await;

    assert_eq!(status, StatusCode::OK);
    let (_, opened, ended) = client.open_response(id, &response);
    assert_eq!(opened, b"payload");
    assert!(ended);
}

#[tokio::test]
async fn a_tampered_request_does_not_open() {
    let transport = transport();
    let mut client = Client::open(&transport).await;
    let (_, mut sealed) = client.seal(&head("POST", "/echo", &[]), b"payload");
    let last = sealed.len() - 1;
    sealed[last] ^= 1;
    let (status, body) = send(&transport, SEALED_PATH, sealed).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(error_code(&body), "undecryptable");
}

/// A proxy holds every sealed request it forwarded. Sending one again must not
/// run it again.
#[tokio::test]
async fn a_replayed_request_is_refused() {
    let transport = transport();
    let mut client = Client::open(&transport).await;
    let (_, sealed) = client.seal(&head("POST", "/echo", &[]), b"once");

    let (status, _) = send(&transport, SEALED_PATH, sealed.clone()).await;
    assert_eq!(status, StatusCode::OK);
    let (status, body) = send(&transport, SEALED_PATH, sealed).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(error_code(&body), "replayed_request");
}

/// The session id and request id travel in the clear. A forgery naming an id
/// the client has yet to use must not use it up.
#[tokio::test]
async fn a_forged_request_does_not_spend_its_request_id() {
    let transport = transport();
    let client = Client::open(&transport).await;
    let mut forged = client.header(7).to_vec();
    forged.extend_from_slice(&[0u8; 64]);
    let (status, body) = send(&transport, SEALED_PATH, forged).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(error_code(&body), "undecryptable");

    let sealed = client.seal_as(7, &head("POST", "/echo", &[]), b"");
    let (status, _) = send(&transport, SEALED_PATH, sealed).await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn an_envelope_inside_an_envelope_is_refused() {
    let transport = transport();
    let mut client = Client::open(&transport).await;
    for path in [SEALED_PATH, HANDSHAKE_PATH] {
        let (_, sealed) = client.seal(&head("POST", path, &[]), b"");
        let (status, body) = send(&transport, SEALED_PATH, sealed).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(error_code(&body), "malformed");
    }
}

#[tokio::test]
async fn an_event_stream_comes_back_sealed_frame_by_frame() {
    let transport = transport();
    let mut client = Client::open(&transport).await;
    let (id, sealed) = client.seal(&head("GET", "/stream", &[]), b"");
    let (status, body) = send(&transport, SEALED_PATH, sealed).await;
    assert_eq!(status, StatusCode::OK);
    assert!(!contains(&body, b"data:"));

    let (head, events, ended) = client.open_response(id, &body);
    assert!(ended);
    assert_eq!(head.status, 200);
    assert_eq!(
        response_header(&head, "content-type"),
        Some("text/event-stream")
    );
    let events = String::from_utf8(events).unwrap();
    assert!(events.contains("data: one") && events.contains("data: two"));
}

#[tokio::test]
async fn a_large_body_is_split_across_frames() {
    let transport = transport();
    let mut client = Client::open(&transport).await;
    let (id, sealed) = client.seal(&head("GET", "/big", &[]), b"");
    let (_, body) = send(&transport, SEALED_PATH, sealed).await;
    let (head, inner, ended) = client.open_response(id, &body);
    assert!(ended);
    assert_eq!(head.status, 200);
    assert_eq!(inner, vec![7u8; 3 * MAX_FRAME_DATA + 5]);
}

#[tokio::test]
async fn a_websocket_upgrade_is_refused_inside_the_envelope() {
    let transport = transport();
    let mut client = Client::open(&transport).await;
    let (id, sealed) = client.seal(
        &head(
            "GET",
            "/ws",
            &[("connection", "upgrade"), ("upgrade", "websocket")],
        ),
        b"",
    );
    let (status, body) = send(&transport, SEALED_PATH, sealed).await;
    assert_eq!(status, StatusCode::OK);
    let (head, _, _) = client.open_response(id, &body);
    assert_eq!(head.status, 501);
}

/// A response still streaming when its session expires stops without an end
/// frame: its keys are due to be gone, and the client learns it was cut short.
#[tokio::test]
async fn a_stream_outliving_its_session_is_cut_off() {
    let keys = SessionKeys {
        request: Zeroizing::new([1; 32]),
        response: Zeroizing::new([2; 32]),
    };
    let client = Client {
        session_id: [3; SESSION_ID_LEN],
        keys: keys.clone(),
        next_request_id: 1,
    };
    let never_ends = Body::from_stream(futures_stream::pending::<Result<Bytes, std::io::Error>>());
    let sealed = seal_response(
        FrameSealer::new(&keys, client.header(1), 1),
        Response::new(never_ends),
        Instant::now(),
    );
    let body = to_bytes(sealed.into_body(), usize::MAX).await.unwrap();
    let (head, inner, ended) = client.open_response(1, &body);
    assert_eq!(head.status, 200);
    assert!(inner.is_empty());
    assert!(
        !ended,
        "no end frame, so the client knows the stream was cut"
    );
}

#[test]
fn a_session_past_its_lifetime_or_idle_limit_is_gone() {
    let keys = || SessionKeys {
        request: Zeroizing::new([1; 32]),
        response: Zeroizing::new([2; 32]),
    };
    let start = Instant::now();
    let mut sessions = Sessions::default();

    sessions.insert([1; SESSION_ID_LEN], keys(), start);
    assert!(sessions.keys(&[1; SESSION_ID_LEN], start).is_ok());
    let idle = start + Duration::from_secs(11 * 60);
    assert_eq!(
        sessions
            .keys(&[1; SESSION_ID_LEN], idle)
            .err()
            .unwrap()
            .code,
        "unknown_session"
    );

    // Kept busy, a session still ends at its lifetime.
    sessions.insert([2; SESSION_ID_LEN], keys(), start);
    let mut now = start;
    let mut request_id = 1;
    while now < start + SESSION_LIFETIME {
        sessions
            .admit(&[2; SESSION_ID_LEN], request_id, now)
            .unwrap();
        request_id += 1;
        now += Duration::from_secs(5 * 60);
    }
    assert_eq!(
        sessions.keys(&[2; SESSION_ID_LEN], now).err().unwrap().code,
        "unknown_session"
    );
}

#[test]
fn request_ids_below_the_replay_window_are_refused() {
    let mut sessions = Sessions::default();
    let now = Instant::now();
    let id = [4; SESSION_ID_LEN];
    sessions.insert(
        id,
        SessionKeys {
            request: Zeroizing::new([1; 32]),
            response: Zeroizing::new([2; 32]),
        },
        now,
    );
    assert!(sessions.admit(&id, 0, now).is_err(), "ids start at 1");
    for request_id in 2..=5000 {
        sessions.admit(&id, request_id, now).unwrap();
    }
    assert!(
        sessions.admit(&id, 1, now).is_err(),
        "an id older than the window cannot be told apart from a replay"
    );
    assert!(sessions.admit(&id, 4000, now).is_err());
    assert!(sessions.admit(&id, 5001, now).is_ok());
}

/// With sealing required, a client that forgets to seal is refused before its
/// request (and its bearer token) reaches anything, while the requests it needs
/// to learn the key and seal still go through.
#[tokio::test]
async fn with_sealing_required_only_sealed_requests_are_served() {
    let transport = transport_with(&SealedOptions::new(true, None));

    let (status, body) = send(&transport, "/echo", b"plain".to_vec()).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(error_code(&body), "sealed_required");

    let (status, body) = get_unsealed(&transport, "/admin-api/health").await;
    assert_eq!(status, StatusCode::OK, "probes still answer");
    assert_eq!(&body[..], b"alive");

    let mut client = Client::open(&transport).await;
    let (id, sealed) = client.seal(&head("POST", "/echo", &[]), b"sealed");
    let (status, body) = send(&transport, SEALED_PATH, sealed).await;
    assert_eq!(status, StatusCode::OK);
    let (head, inner, _) = client.open_response(id, &body);
    assert_eq!(head.status, 200);
    assert_eq!(inner, b"sealed");
}

const CONTEXT: &str = "0000000000000000000000000000000000000000000000000000000000000abc";

/// Seal `method path` with `body`, and open what came back: status and body.
async fn sealed_call(
    transport: &Arc<SealedTransport>,
    method: &str,
    path: &str,
    body: &[u8],
) -> (u16, Vec<u8>) {
    let mut client = Client::open(transport).await;
    let (id, sealed) = client.seal(&head(method, path, &[]), body);
    let (status, outer) = send(transport, SEALED_PATH, sealed).await;
    assert_eq!(status, StatusCode::OK, "the envelope itself is accepted");
    let (head, inner, _) = client.open_response(id, &outer);
    (head.status, inner)
}

/// A node whose auth a proxy enforces guards nothing itself, and the proxy sees
/// only `POST /sealed/v2`. Through the envelope, then, only what the node
/// serves without a credential is reachable: delegated execution, where the
/// warrant in the body is the credential, and the public probes and TEE routes.
/// Anything the proxy would have guarded is refused inside the envelope.
#[tokio::test]
async fn behind_an_auth_proxy_only_uncredentialed_routes_are_reachable_sealed() {
    let transport = transport_with(&SealedOptions::new(false, None).with_inner_scope(
        InnerScope::Uncredentialed {
            delegated_access: true,
        },
    ));

    let intents = format!("/admin-api/contexts/{CONTEXT}/intents");
    let (status, body) = sealed_call(&transport, "POST", &intents, b"warrant").await;
    assert_eq!(
        status, 200,
        "delegated execution carries its own credential"
    );
    assert_eq!(body, b"warrant");

    let create = format!("/admin-api/groups/{CONTEXT}/context-intents");
    let (status, body) = sealed_call(&transport, "POST", &create, b"creation").await;
    assert_eq!(
        status, 200,
        "delegated context creation carries its own credential too"
    );
    assert_eq!(body, b"creation");

    let govern = format!("/admin-api/groups/{CONTEXT}/governance-intents");
    let (status, body) = sealed_call(&transport, "POST", &govern, b"governance").await;
    assert_eq!(status, 200, "and delegated governance");
    assert_eq!(body, b"governance");

    let (status, body) = sealed_call(&transport, "GET", "/admin-api/health", b"").await;
    assert_eq!(status, 200);
    assert_eq!(body, b"alive");

    let guarded_create = format!("/admin-api/groups/{CONTEXT}/contexts");
    for (method, path) in [
        ("GET", "/admin-api/contexts"),
        ("DELETE", "/admin-api/contexts"),
        ("POST", guarded_create.as_str()),
        ("POST", "/echo"),
        ("POST", "/jsonrpc"),
    ] {
        let (status, body) = sealed_call(&transport, method, path, b"").await;
        assert_eq!(
            status, 403,
            "{method} {path} would bypass the proxy's guard"
        );
        assert_eq!(error_code(&body), "sealed_route_unguarded");
    }
}

#[tokio::test]
async fn delegated_execution_is_reachable_sealed_only_where_it_is_public() {
    let transport = transport_with(&SealedOptions::new(false, None).with_inner_scope(
        InnerScope::Uncredentialed {
            delegated_access: false,
        },
    ));
    let intents = format!("/admin-api/contexts/{CONTEXT}/intents");
    let (status, body) = sealed_call(&transport, "POST", &intents, b"").await;
    assert_eq!(
        status, 403,
        "without delegated_access the proxy guards intents too"
    );
    assert_eq!(error_code(&body), "sealed_route_unguarded");

    let create = format!("/admin-api/groups/{CONTEXT}/context-intents");
    let (status, body) = sealed_call(&transport, "POST", &create, b"").await;
    assert_eq!(status, 403, "and delegated context creation with them");
    assert_eq!(error_code(&body), "sealed_route_unguarded");
}

#[test]
fn the_uncredentialed_scope_matches_paths_exactly() {
    let scoped = |prefix: Option<&str>| {
        transport_with(
            &SealedOptions::new(false, prefix.map(str::to_owned)).with_inner_scope(
                InnerScope::Uncredentialed {
                    delegated_access: true,
                },
            ),
        )
    };
    let root = scoped(None);
    for allowed in [
        "/admin-api/health".to_owned(),
        "/admin-api/tee/attest".to_owned(),
        "/admin-api/tee/info".to_owned(),
        format!("/admin-api/contexts/{CONTEXT}/intents"),
        format!("/admin-api/groups/{CONTEXT}/context-intents"),
        format!("/admin-api/groups/{CONTEXT}/governance-intents"),
    ] {
        assert!(root.allows_inner(&allowed), "{allowed}");
    }
    for refused in [
        "/admin-api/tee/fleet-join".to_owned(),
        "/admin-api/tee/registration-attest".to_owned(),
        "/admin-api/healthz".to_owned(),
        "/admin-api/contexts".to_owned(),
        format!("/admin-api/contexts/{}/intents", CONTEXT.to_uppercase()),
        format!("/admin-api/contexts/{}/intents", &CONTEXT[1..]),
        format!("/admin-api/contexts/{CONTEXT}/intents/extra"),
        format!("/admin-api/contexts/{CONTEXT}/storage"),
        "/admin-api/contexts/../intents".to_owned(),
        format!(
            "/admin-api/groups/{}/context-intents",
            CONTEXT.to_uppercase()
        ),
        format!("/admin-api/groups/{}/context-intents", &CONTEXT[1..]),
        format!("/admin-api/groups/{CONTEXT}0/context-intents"),
        format!("/admin-api/groups/{CONTEXT}/context-intents/extra"),
        format!("/admin-api/groups/{CONTEXT}/context-intents/"),
        format!("/admin-api/groups/{CONTEXT}/governance-intents/x"),
        format!(
            "/admin-api/groups/{}/governance-intents",
            CONTEXT.to_uppercase()
        ),
        format!("/admin-api/groups/{CONTEXT}/members"),
        format!("/admin-api/groups/{CONTEXT}/contexts"),
        format!("/admin-api/groups/{CONTEXT}/intents"),
        format!("/admin-api/contexts/{CONTEXT}/context-intents"),
        "/admin-api/groups/../context-intents".to_owned(),
        "/jsonrpc".to_owned(),
        "/auth/token".to_owned(),
    ] {
        assert!(!root.allows_inner(&refused), "{refused}");
    }

    let prefixed = scoped(Some("/node"));
    assert!(prefixed.allows_inner("/node/admin-api/health"));
    assert!(
        !prefixed.allows_inner("/admin-api/health"),
        "not where the router is"
    );

    let any = transport_with(&SealedOptions::default());
    assert!(
        any.allows_inner("/admin-api/contexts"),
        "embedded auth guards it itself"
    );
}

#[test]
fn with_sealing_required_what_a_client_needs_to_seal_is_served_under_the_prefix() {
    let transport = transport_with(&SealedOptions::new(true, Some("/node".to_owned())));
    assert!(!transport.refuses_unsealed("/node/admin-api/tee/attest"));
    assert!(
        !transport.refuses_unsealed("/node/admin-api/tee/info"),
        "a client reads the release to verify the quote against before it can seal"
    );
    assert!(transport.refuses_unsealed("/node/admin-api/tee/info/extra"));
    assert!(transport.refuses_unsealed("/node/admin-api/tee/fleet-join"));
    assert!(!transport.refuses_unsealed("/node/admin-api/ready"));
    assert!(transport.refuses_unsealed("/node/admin-api/contexts"));
    assert!(transport.refuses_unsealed("/admin-api/tee/attest"));
    assert!(!transport_with(&SealedOptions::default()).refuses_unsealed("/node/jsonrpc"));
}

/// Behind `NODE_PATH_PREFIX` the routers live under the prefix. A sealed
/// request must reach them exactly as a direct request through the same base
/// URL would: under the prefix when the envelope came in under it, and at the
/// root when a proxy stripped it on the way.
#[tokio::test]
async fn a_sealed_request_is_routed_under_the_mount_it_arrived_on() {
    let transport = transport_with(&SealedOptions::new(false, Some("/node".to_owned())));
    for (mount, expected) in [("/node", "/node/echo"), ("", "/echo")] {
        let mut client = Client::open_at(&transport, mount).await;
        let (id, sealed) = client.seal(&head("POST", "/echo", &[]), b"");
        let (status, body) = send(&transport, &format!("{mount}{SEALED_PATH}"), sealed).await;
        assert_eq!(status, StatusCode::OK);
        let (head, _, _) = client.open_response(id, &body);
        assert_eq!(head.status, 200, "{mount:?}");
        assert_eq!(response_header(&head, "x-echo-uri"), Some(expected));
    }
}

#[test]
fn handshakes_beyond_the_rate_limit_are_refused_until_it_refills() {
    let start = Instant::now();
    let mut limit = HandshakeLimit::full(start);
    for _ in 0..200 {
        assert!(limit.admit(start));
    }
    assert!(!limit.admit(start), "the burst is spent");
    assert!(
        limit.admit(start + Duration::from_millis(10)),
        "one more per 10ms"
    );
    assert!(!limit.admit(start + Duration::from_millis(10)));
}

#[tokio::test]
async fn a_handshake_over_the_rate_limit_is_told_to_retry() {
    let transport = transport();
    // Stamped in the future so the empty bucket stays empty: from `now` it
    // would refill one token per 10ms, and a slow runner reaching the handler
    // that much later is let through.
    *transport.handshakes.lock().unwrap() = HandshakeLimit {
        tokens: 0.0,
        refilled: Instant::now() + Duration::from_secs(3600),
    };
    let (message, _) = message_1(&transport.public_key(), &[]);
    let response = app(Arc::clone(&transport))
        .oneshot(
            Request::post(HANDSHAKE_PATH)
                .body(Body::from(handshake_body(
                    &transport.public_key(),
                    &message,
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(response.headers()[RETRY_AFTER], "1");
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    assert_eq!(error_code(&body), "busy");
}

/// A flood of handshakes that never carry a request displaces its own
/// sessions, not a client's.
#[test]
fn a_full_table_drops_unused_sessions_before_used_ones() {
    let keys = || SessionKeys {
        request: Zeroizing::new([1; 32]),
        response: Zeroizing::new([2; 32]),
    };
    let start = Instant::now();
    let mut sessions = Sessions::default();
    let used = [0xff; SESSION_ID_LEN];
    sessions.insert(used, keys(), start);
    sessions.admit(&used, 1, start).unwrap();

    let later = start + Duration::from_secs(60);
    for n in 0..u32::try_from(MAX_SESSIONS + 64).unwrap() {
        let mut id = [0; SESSION_ID_LEN];
        id[..4].copy_from_slice(&n.to_be_bytes());
        sessions.insert(id, keys(), later);
    }
    assert!(
        sessions.keys(&used, later).is_ok(),
        "the used session survived"
    );
}

/// Forward secrecy rests on the keys actually being dropped, so an expired
/// session must go even if nothing ever names it again.
#[test]
fn expiring_drops_sessions_nobody_names_again() {
    let start = Instant::now();
    let mut sessions = Sessions::default();
    sessions.insert(
        [5; SESSION_ID_LEN],
        SessionKeys {
            request: Zeroizing::new([1; 32]),
            response: Zeroizing::new([2; 32]),
        },
        start,
    );
    sessions.expire(start + Duration::from_secs(60));
    assert_eq!(sessions.len(), 1, "still live");
    sessions.expire(start + SESSION_LIFETIME);
    assert_eq!(sessions.len(), 0);
}

#[tokio::test]
async fn a_low_order_client_key_is_refused() {
    let transport = transport();
    let mut low_order = vec![0u8; 32];
    low_order.extend_from_slice(&[0u8; 16]);
    let (status, body) = send(
        &transport,
        HANDSHAKE_PATH,
        handshake_body(&transport.public_key(), &low_order),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(error_code(&body), "malformed");
}

/// Every frame fits in 64 KiB, so a client can refuse a larger one outright.
#[tokio::test]
async fn response_headers_too_large_for_a_frame_become_a_502() {
    let transport = transport();
    let mut client = Client::open(&transport).await;
    let (id, sealed) = client.seal(&head("GET", "/huge-head", &[]), b"");
    let (_, body) = send(&transport, SEALED_PATH, sealed).await;
    let (head, _, ended) = client.open_response(id, &body);
    assert_eq!(head.status, 502);
    assert!(ended);
}

#[tokio::test]
async fn refusals_are_counted_by_code() {
    let mut registry = Registry::default();
    let transport = Arc::new(SealedTransport::from_secret(
        Zeroizing::new(TRANSPORT_SECRET),
        &SealedOptions::new(true, None),
        SealedMetrics::register(&mut registry),
    ));
    let _ = send(&transport, "/echo", Vec::new()).await;
    let _client = Client::open(&transport).await;

    let mut text = String::new();
    prometheus_client::encoding::text::encode(&mut text, &registry).unwrap();
    assert!(
        text.contains("sealed_refusals_total{code=\"sealed_required\"} 1"),
        "{text}"
    );
    assert!(text.contains("sealed_handshakes_total 1"), "{text}");
    assert!(text.contains("sealed_sessions 1"), "{text}");
}

const CLIENT_EPHEMERAL: [u8; 32] = [0x33; 32];
const NODE_EPHEMERAL: [u8; 32] = [0x44; 32];
const SESSION_ID: SessionId = [0x55; SESSION_ID_LEN];

/// Fixed vectors, repeated verbatim in mero-js: the two sides are separate
/// implementations of one wire format, and this is what keeps them the same
/// format. mero-js runs the Noise handshake itself, with WebCrypto, so it
/// checks both this crate's framing and snow's handshake.
#[test]
fn the_wire_format_matches_the_published_vectors() {
    let transport = transport();
    let mut initiator = builder()
        .remote_public_key(&transport.public_key())
        .fixed_ephemeral_key_for_testing_only(&CLIENT_EPHEMERAL)
        .build_initiator()
        .unwrap();
    let mut message = [0u8; 64];
    let len = initiator.write_message(&[], &mut message).unwrap();
    let handshake_request = handshake_body(&transport.public_key(), &message[..len]);

    let responder = builder()
        .local_private_key(&TRANSPORT_SECRET)
        .fixed_ephemeral_key_for_testing_only(&NODE_EPHEMERAL)
        .build_responder()
        .unwrap();
    let (message_2, _) = respond_with(responder, &message[..len], &SESSION_ID).unwrap();
    let handshake_response = [&[VERSION][..], &message_2].concat();

    let client = Client::finish(&mut initiator, &handshake_response);
    let request = client.seal_as(
        1,
        &RequestHead {
            method: "POST".to_owned(),
            path: "/jsonrpc".to_owned(),
            headers: vec![("content-type".to_owned(), "application/json".to_owned())],
        },
        b"{}",
    );
    let mut response_headers = HeaderMap::new();
    let _previous =
        response_headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    let mut frames = FrameSealer::new(&client.keys, client.header(1), 1);
    let head = serde_json::to_vec(&ResponseHead {
        status: 200,
        headers: header_pairs(&response_headers),
    })
    .unwrap();
    let response = [
        frames.seal(FRAME_HEAD, &head).unwrap(),
        frames.seal(FRAME_DATA, b"{\"ok\":true}").unwrap(),
        frames.seal(FRAME_END, &[]).unwrap(),
    ]
    .concat();
    assert_eq!(client.open_response(1, &response).1, b"{\"ok\":true}");

    assert_eq!(
        [
            hex::encode(transport.public_key()),
            hex::encode(handshake_request),
            hex::encode(handshake_response),
            hex::encode(request),
            hex::encode(response),
        ],
        [
            TRANSPORT_PUBLIC_VECTOR,
            HANDSHAKE_REQUEST_VECTOR,
            HANDSHAKE_RESPONSE_VECTOR,
            SEALED_REQUEST_VECTOR,
            SEALED_RESPONSE_VECTOR,
        ]
    );
    assert_eq!(PROLOGUE, b"calimero/sealed-http/v2");
}

// Reproduced independently with WebCrypto (X25519, SHA-256/HMAC, AES-GCM) in
// mero-js's sealed-fetch tests, handshake included, so the format is standard
// and not merely self-consistent.
const TRANSPORT_PUBLIC_VECTOR: &str =
    "0faa684ed28867b97f4a6a2dee5df8ce974e76b7018e3f22a1c4cf2678570f20";
const HANDSHAKE_REQUEST_VECTOR: &str =
    "020faa684ed28867b97f4a6a2dee5df8ce974e76b7018e3f22a1c4cf2678570f207b0d47d93427f831116078\
    1c7c733fd89f88970aef490d8aa0ee19a4cb8a1b1461fba335b4f3e9be8a34a61900e5b4be";
const HANDSHAKE_RESPONSE_VECTOR: &str =
    "02ff2ee45601ec1b67310c7790404585ae697331eee1c1f8cf2419731c1fff3e6b7d6339b7296bdc4778248f\
    57126a10ee5b7fb39b0cb104dcbfcaaaf44d0e6017379638a4";
const SEALED_REQUEST_VECTOR: &str =
    "025555555555555555555555555555555500000000000000019fa4301264a48bdf6811cfb1d7c3dad713ede5\
    ea1820608358ff2779cee1605efc3d97879f24d6cf302a7877beda552ca6714ef9d5496f978fdcdde522435c\
    f663e4be5f60170e8eab74161a913608fe268e5352fcd3090d62e2a12c92b06b51885d43c4d83dcfe544";
const SEALED_RESPONSE_VECTOR: &str =
    "0000004f4873ec5a589a1e3585360440100bc00d20522fe35bac2221dcfe80341ce3069481d4794b801776f4\
    a76188b81887eb83cfccd2981229af48a3820df924261b3ff12d4b29340ca07c392ed4e0ee4bed0000001cfa\
    20a7633655179808da19e2fcbc81bc219262fc4f5406582834f549000000111426897864d7a92eec9261fe29\
    93415592";

const SEVERAL_INNER_ORIGINS: [(&str, &str); 3] = [
    ("Origin", "http://localhost"),
    ("ORIGIN", "http://other.example"),
    ("origin", "null"),
];

#[tokio::test]
async fn every_inner_origin_is_replaced_by_the_outer_hops() {
    let origin = echoed_origin(&SEVERAL_INNER_ORIGINS, Some("https://app.example")).await;
    assert_eq!(origin, "https://app.example");
}

#[tokio::test]
async fn every_inner_origin_is_dropped_when_the_outer_hop_has_none() {
    let origin = echoed_origin(&SEVERAL_INNER_ORIGINS, None).await;
    assert_eq!(origin, "none");
}

/// What a fake auth service saw of the probes it was sent.
#[derive(Default)]
struct AuthLog {
    probes: Mutex<Vec<HeaderMap>>,
    logins: Mutex<Vec<(HeaderMap, Bytes)>>,
}

/// A stand-in for mero-auth on loopback: `/auth/validate` admits `Bearer
/// good` as account `acct` with `context:query`, and refuses anything else as
/// mero-auth does; `/auth/token` answers a login.
async fn fake_auth() -> (String, Arc<AuthLog>) {
    let log = Arc::new(AuthLog::default());
    let validate = {
        let log = Arc::clone(&log);
        move |headers: HeaderMap| {
            let log = Arc::clone(&log);
            async move {
                let good = headers
                    .get(AUTHORIZATION)
                    .is_some_and(|value| value == "Bearer good");
                log.probes.lock().unwrap().push(headers);
                if good {
                    (
                        StatusCode::OK,
                        [
                            ("x-auth-user", "acct"),
                            ("x-auth-permissions", "context:query"),
                            ("x-auth-account", "acct"),
                        ],
                        "",
                    )
                        .into_response()
                } else {
                    (
                        StatusCode::UNAUTHORIZED,
                        [("x-auth-error", "invalid_token")],
                        "Invalid token",
                    )
                        .into_response()
                }
            }
        }
    };
    let token = {
        let log = Arc::clone(&log);
        move |headers: HeaderMap, body: Bytes| {
            let log = Arc::clone(&log);
            async move {
                log.logins.lock().unwrap().push((headers, body));
                (StatusCode::OK, "{\"accessToken\":\"good\"}")
            }
        }
    };
    let router = Router::new()
        .route("/auth/validate", get(validate))
        .route("/auth/token", post(token));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    drop(tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    }));
    (origin, log)
}

fn guarded_transport(origin: &str) -> Arc<SealedTransport> {
    transport_with(
        &SealedOptions::new(false, None)
            .with_inner_scope(InnerScope::Uncredentialed {
                delegated_access: true,
            })
            .with_forward_auth(Some(ForwardAuth::new(origin).unwrap())),
    )
}

async fn sealed_call_with(
    transport: &Arc<SealedTransport>,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: &[u8],
) -> (ResponseHead, Vec<u8>) {
    let mut client = Client::open(transport).await;
    let (id, sealed) = client.seal(&head(method, path, headers), body);
    let (status, outer) = send(transport, SEALED_PATH, sealed).await;
    assert_eq!(status, StatusCode::OK, "the envelope itself is accepted");
    let (head, inner, _) = client.open_response(id, &outer);
    (head, inner)
}

/// With `forward_auth`, a logged-in call the proxy would have guarded is
/// guarded by the same auth service, asked the same question, and reaches the
/// route with the identity the auth service names.
#[tokio::test]
async fn behind_an_auth_proxy_a_sealed_call_is_guarded_by_its_auth_service() {
    let (origin, log) = fake_auth().await;
    let transport = guarded_transport(&origin);

    let (head, _) = sealed_call_with(
        &transport,
        "GET",
        "/admin-api/contexts?all=1",
        &[("authorization", "Bearer good")],
        b"",
    )
    .await;
    assert_eq!(head.status, 200, "an admitted token reaches the route");
    assert_eq!(response_header(&head, "x-echo-account"), Some("acct"));
    assert_eq!(
        response_header(&head, "x-echo-permissions"),
        Some("context:query")
    );

    let probes = log.probes.lock().unwrap();
    let probe = probes.last().unwrap();
    assert_eq!(probe["x-forwarded-method"], "GET");
    assert_eq!(probe["x-forwarded-uri"], "/admin-api/contexts?all=1");
    assert_eq!(
        probe["x-forwarded-host"], "tee-node.example",
        "the host the caller reached, for node-bound tokens"
    );
}

#[tokio::test]
async fn behind_an_auth_proxy_the_auth_services_refusal_comes_back_sealed() {
    let (origin, _log) = fake_auth().await;
    let transport = guarded_transport(&origin);

    for headers in [&[("authorization", "Bearer bad")][..], &[]] {
        let (head, body) =
            sealed_call_with(&transport, "DELETE", "/admin-api/contexts", headers, b"").await;
        assert_eq!(head.status, 401, "the route is never reached");
        assert_eq!(
            response_header(&head, "x-auth-error"),
            Some("invalid_token")
        );
        assert_eq!(body, b"Invalid token");
    }
}

/// The identity is the auth service's to name. An envelope stating one gets
/// it replaced by what the auth service says, or dropped when it says none.
#[tokio::test]
async fn behind_an_auth_proxy_an_envelope_cannot_name_its_own_identity() {
    let (origin, _log) = fake_auth().await;
    let transport = guarded_transport(&origin);

    let (head, _) = sealed_call_with(
        &transport,
        "GET",
        "/admin-api/contexts",
        &[
            ("authorization", "Bearer good"),
            ("x-auth-account", "someone-else"),
            ("x-auth-permissions", "admin"),
        ],
        b"",
    )
    .await;
    assert_eq!(head.status, 200);
    assert_eq!(response_header(&head, "x-echo-account"), Some("acct"));
    assert_eq!(
        response_header(&head, "x-echo-permissions"),
        Some("context:query")
    );

    // A public route asks nobody, and the stated identity does not survive.
    let intents = format!("/admin-api/contexts/{CONTEXT}/intents");
    let (head, _) = sealed_call_with(
        &transport,
        "POST",
        &intents,
        &[("x-auth-account", "someone-else")],
        b"warrant",
    )
    .await;
    assert_eq!(head.status, 200);
    assert_eq!(response_header(&head, "x-echo-account"), Some("none"));
}

#[tokio::test]
async fn behind_an_auth_proxy_public_routes_are_not_sent_to_the_auth_service() {
    let (origin, log) = fake_auth().await;
    let transport = guarded_transport(&origin);

    let intents = format!("/admin-api/contexts/{CONTEXT}/intents");
    let (head, body) = sealed_call_with(&transport, "POST", &intents, &[], b"warrant").await;
    assert_eq!(head.status, 200);
    assert_eq!(body, b"warrant");
    let (head, _) = sealed_call_with(&transport, "GET", "/admin-api/health", &[], b"").await;
    assert_eq!(head.status, 200);
    assert!(log.probes.lock().unwrap().is_empty());
}

/// A login is the auth service's: the proxy routes `/auth/` to it, and so does
/// the node for a sealed one, with the host the caller reached.
#[tokio::test]
async fn behind_an_auth_proxy_a_sealed_login_reaches_the_auth_service() {
    let (origin, log) = fake_auth().await;
    let transport = guarded_transport(&origin);

    let (head, body) = sealed_call_with(
        &transport,
        "POST",
        "/auth/token",
        &[
            ("content-type", "application/json"),
            ("x-forwarded-for", "203.0.113.9"),
            ("x-forwarded-host", "attacker.example"),
        ],
        b"{\"auth_method\":\"account_proof\"}",
    )
    .await;
    assert_eq!(head.status, 200);
    assert_eq!(body, b"{\"accessToken\":\"good\"}");

    let logins = log.logins.lock().unwrap();
    let (headers, sent) = logins.last().unwrap();
    assert_eq!(sent.as_ref(), b"{\"auth_method\":\"account_proof\"}");
    assert_eq!(
        headers["x-forwarded-host"], "tee-node.example",
        "the host the caller reached, not one the envelope states"
    );
    assert!(
        headers.get("x-forwarded-for").is_none(),
        "the proxy overwrites forwarding headers; an envelope does not set them"
    );
    assert_eq!(headers["content-type"], "application/json");
}

#[tokio::test]
async fn behind_an_auth_proxy_an_unreachable_auth_service_is_a_502() {
    // Bound and released: nothing listens there.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    drop(listener);
    let transport = guarded_transport(&origin);

    let (head, _) = sealed_call_with(
        &transport,
        "GET",
        "/admin-api/contexts",
        &[("authorization", "Bearer good")],
        b"",
    )
    .await;
    assert_eq!(head.status, 502, "never routed unguarded");
    let (head, _) = sealed_call_with(&transport, "POST", "/auth/token", &[], b"{}").await;
    assert_eq!(head.status, 502);
}

#[test]
fn forward_auth_is_a_loopback_http_origin_only() {
    for ok in [
        "http://127.0.0.1:3001",
        "http://127.0.0.1:3001/",
        "http://localhost:3001",
        "http://[::1]:3001",
    ] {
        assert!(ForwardAuth::new(ok).is_ok(), "{ok}");
    }
    for refused in [
        "https://127.0.0.1:3001",
        "http://10.0.0.5:3001",
        "http://auth.example:3001",
        "http://127.0.0.1:3001/auth",
        "http://127.0.0.1:3001/?x=1",
        "http://user:pw@127.0.0.1:3001",
        "127.0.0.1:3001",
    ] {
        assert!(ForwardAuth::new(refused).is_err(), "{refused}");
    }
}

#[test]
fn the_auth_service_is_routed_its_own_prefixes_only() {
    assert!(ForwardAuth::serves("/auth/token"));
    assert!(ForwardAuth::serves("/admin/client-key"));
    assert!(!ForwardAuth::serves("/admin-api/contexts"));
    assert!(!ForwardAuth::serves("/auth"));
    assert!(!ForwardAuth::serves("/authx/token"));
}
