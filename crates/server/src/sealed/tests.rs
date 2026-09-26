use core::time::Duration;

use axum::http::header::AUTHORIZATION;
use axum::response::sse::{Event, Sse};
use axum::routing::{get, post};
use axum::Router;
use futures_util::stream as futures_stream;
use tower::{Layer, ServiceExt};

use super::session::{builder, respond_with, PROLOGUE};
use super::*;

const TRANSPORT_SECRET: [u8; 32] = [0x22; 32];

fn transport() -> Arc<SealedTransport> {
    Arc::new(SealedTransport::from_secret(Zeroizing::new(
        TRANSPORT_SECRET,
    )))
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
        ],
        body,
    )
}

fn app(
    transport: Arc<SealedTransport>,
) -> impl tower::Service<Request, Response = Response, Error = core::convert::Infallible> {
    let router = Router::new()
        .route("/echo", post(echo))
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
        let (message, mut initiator) = message_1(&transport.public_key(), &[]);
        let (status, body) = send(
            transport,
            HANDSHAKE_PATH,
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
        open_request(&replayed_keys, &envelope).is_err(),
        "the transport key and the recorded handshake do not rebuild the session keys"
    );
    assert!(open_request(&client.keys, &envelope).is_ok());
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
