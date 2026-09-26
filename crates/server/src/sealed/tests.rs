use axum::http::header::AUTHORIZATION;
use axum::response::sse::{Event, Sse};
use axum::routing::{get, post};
use axum::Router;
use futures_util::stream;
use tower::{Layer, ServiceExt};

use super::*;

const TRANSPORT_SECRET: [u8; 32] = [0x22; 32];
const CLIENT_SECRET: [u8; 32] = [0x33; 32];
const REQUEST_NONCE: [u8; NONCE_LEN] = [0x44; NONCE_LEN];

fn transport() -> Arc<SealedTransport> {
    Arc::new(SealedTransport::from_secret(Zeroizing::new(
        TRANSPORT_SECRET,
    )))
}

fn public_of(secret: &[u8; 32]) -> [u8; 32] {
    MontgomeryPoint::mul_base_clamped(*secret).0
}

/// The client's side of the exchange, as mero-js does it.
struct Client {
    secret: [u8; 32],
    transport_public: [u8; 32],
}

impl Client {
    fn new(secret: [u8; 32], transport_public: [u8; 32]) -> Self {
        Self {
            secret,
            transport_public,
        }
    }

    fn keys(&self) -> ExchangeKeys {
        ExchangeKeys::derive(
            &self.secret,
            &self.transport_public,
            &self.transport_public,
            &public_of(&self.secret),
        )
        .unwrap()
    }

    fn seal(&self, head: &RequestHead, body: &[u8]) -> Vec<u8> {
        let mut header = Vec::with_capacity(REQUEST_HEADER_LEN);
        header.push(VERSION);
        header.extend_from_slice(&self.transport_public);
        header.extend_from_slice(&public_of(&self.secret));
        header.extend_from_slice(&REQUEST_NONCE);
        let mut buffer = join_inner(head, body);
        self.keys()
            .request
            .seal_in_place_append_tag(
                Nonce::assume_unique_for_key(REQUEST_NONCE),
                Aad::from(&header),
                &mut buffer,
            )
            .unwrap();
        header.extend_from_slice(&buffer);
        header
    }

    fn open(&self, sealed: &[u8]) -> (ResponseHead, Vec<u8>) {
        assert_eq!(sealed[0], VERSION);
        let keys = self.keys();
        let nonce: [u8; NONCE_LEN] = sealed[1..=NONCE_LEN].try_into().unwrap();
        let mut buffer = sealed[1 + NONCE_LEN..].to_vec();
        let len = keys
            .response
            .open_in_place(
                Nonce::assume_unique_for_key(nonce),
                Aad::from(&keys.response_aad),
                &mut buffer,
            )
            .unwrap()
            .len();
        buffer.truncate(len);
        let (head, body) = split_inner::<ResponseHead>(&buffer).unwrap();
        (head, body.to_vec())
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
        ts: unix_now(),
    }
}

async fn echo(headers: HeaderMap, uri: Uri, body: Bytes) -> impl IntoResponse {
    let auth = headers
        .get(AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("none")
        .to_owned();
    let host = headers
        .get(HOST)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("none")
        .to_owned();
    (
        [
            ("x-echo-auth", auth),
            ("x-echo-uri", uri.to_string()),
            ("x-echo-host", host),
        ],
        body,
    )
}

fn app(
    transport: Arc<SealedTransport>,
) -> impl tower::Service<Request, Response = Response, Error = core::convert::Infallible> {
    let router = Router::new().route("/echo", post(echo)).route(
        "/stream",
        get(|| async {
            Sse::new(stream::iter([Ok::<_, core::convert::Infallible>(
                Event::default().data("tick"),
            )]))
        }),
    );
    axum::middleware::from_fn_with_state(transport, intercept).layer(router)
}

async fn post_sealed(transport: Arc<SealedTransport>, body: Vec<u8>) -> (StatusCode, Bytes) {
    let response = app(transport)
        .oneshot(
            Request::post(SEALED_PATH)
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

#[tokio::test]
async fn a_sealed_request_reaches_the_router_and_its_response_comes_back_sealed() {
    let transport = transport();
    let client = Client::new(CLIENT_SECRET, transport.public_key());
    let sealed = client.seal(
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

    let (status, body) = post_sealed(transport, sealed).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        !body
            .windows(b"secret-token".len())
            .any(|w| w == b"secret-token")
            && !body.windows(b"hello".len()).any(|w| w == b"hello"),
        "nothing of the exchange is readable on the wire"
    );

    let (head, inner) = client.open(&body);
    assert_eq!(head.status, 200);
    assert_eq!(inner, b"{\"hello\":1}");
    let header = |name: &str| {
        head.headers
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.clone())
    };
    assert_eq!(
        header("x-echo-auth").as_deref(),
        Some("Bearer secret-token")
    );
    assert_eq!(header("x-echo-uri").as_deref(), Some("/echo?x=1"));
    assert_eq!(
        header("x-echo-host").as_deref(),
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

/// A restarted node holds a new key. It must say so rather than fail opaquely,
/// so the client knows to attest again.
#[tokio::test]
async fn a_request_sealed_to_another_transport_key_is_refused_as_stale() {
    let client = Client::new(CLIENT_SECRET, public_of(&[0x55; 32]));
    let sealed = client.seal(&head("POST", "/echo", &[]), b"");
    let (status, body) = post_sealed(transport(), sealed).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(error_code(&body), "stale_transport_key");
}

#[tokio::test]
async fn a_tampered_request_does_not_open() {
    let transport = transport();
    let client = Client::new(CLIENT_SECRET, transport.public_key());
    let mut sealed = client.seal(&head("POST", "/echo", &[]), b"payload");
    let last = sealed.len() - 1;
    sealed[last] ^= 1;
    let (status, body) = post_sealed(transport, sealed).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(error_code(&body), "undecryptable");
}

/// A proxy holds every sealed request it forwarded. Sending one again must not
/// run it again.
#[tokio::test]
async fn a_replayed_request_is_refused() {
    let transport = transport();
    let client = Client::new(CLIENT_SECRET, transport.public_key());
    let sealed = client.seal(&head("POST", "/echo", &[]), b"once");

    let (status, _) = post_sealed(Arc::clone(&transport), sealed.clone()).await;
    assert_eq!(status, StatusCode::OK);
    let (status, body) = post_sealed(transport, sealed).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(error_code(&body), "replayed_request");
}

#[tokio::test]
async fn a_request_outside_the_time_window_is_refused() {
    let transport = transport();
    let client = Client::new(CLIENT_SECRET, transport.public_key());
    let mut stale = head("POST", "/echo", &[]);
    stale.ts -= MAX_SKEW.as_secs() + 1;
    let (status, body) = post_sealed(transport, client.seal(&stale, b"")).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(error_code(&body), "stale_request");
}

#[tokio::test]
async fn an_envelope_inside_an_envelope_is_refused() {
    let transport = transport();
    let client = Client::new(CLIENT_SECRET, transport.public_key());
    let sealed = client.seal(&head("POST", SEALED_PATH, &[]), b"");
    let (status, body) = post_sealed(transport, sealed).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(error_code(&body), "malformed");
}

/// A stream never ends, so buffering it would hold the request forever.
#[tokio::test]
async fn a_streaming_response_is_refused_inside_the_envelope() {
    let transport = transport();
    let client = Client::new(CLIENT_SECRET, transport.public_key());
    let sealed = client.seal(&head("GET", "/stream", &[]), b"");
    let (status, body) = post_sealed(transport, sealed).await;
    assert_eq!(status, StatusCode::OK);
    let (head, _) = client.open(&body);
    assert_eq!(head.status, 501);
}

#[test]
fn a_low_order_client_key_is_refused() {
    assert!(ExchangeKeys::derive(&TRANSPORT_SECRET, &[0; 32], &[0; 32], &[0; 32]).is_err());
}

/// Fixed vectors, repeated verbatim in mero-js: the two sides are separate
/// implementations of one wire format, and this is what keeps them the same
/// format.
#[test]
fn the_wire_format_matches_the_published_vectors() {
    let transport = transport();
    let client = Client::new(CLIENT_SECRET, transport.public_key());
    let request = client.seal(
        &RequestHead {
            method: "POST".to_owned(),
            path: "/jsonrpc".to_owned(),
            headers: vec![("content-type".to_owned(), "application/json".to_owned())],
            ts: 1_700_000_000,
        },
        b"{}",
    );
    let mut headers = HeaderMap::new();
    let _previous = headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    let response = client.keys().seal_response_with(
        StatusCode::OK,
        &headers,
        b"{\"ok\":true}",
        [0x66; NONCE_LEN],
    );

    assert_eq!(
        [
            hex::encode(transport.public_key()),
            hex::encode(request),
            hex::encode(response),
        ],
        [
            TRANSPORT_PUBLIC_VECTOR,
            SEALED_REQUEST_VECTOR,
            SEALED_RESPONSE_VECTOR,
        ]
    );
}

// Also reproduced independently with WebCrypto (X25519, HKDF, AES-GCM) in
// mero-js's sealed-fetch tests, so the format is standard and not merely
// self-consistent.
const TRANSPORT_PUBLIC_VECTOR: &str =
    "0faa684ed28867b97f4a6a2dee5df8ce974e76b7018e3f22a1c4cf2678570f20";
const SEALED_REQUEST_VECTOR: &str =
    "010faa684ed28867b97f4a6a2dee5df8ce974e76b7018e3f22a1c4cf2678570f20\
    7b0d47d93427f8311160781c7c733fd89f88970aef490d8aa0ee19a4cb8a1b14444444444444444444444444\
    42085b2ea7f498345db1128aaf8d1d7ce8c97845c231098d87138a0f61ed2b2428005101caf5f07843bb12ea5a82\
    54ec07b049d4131bc1e4ad5a21f06efbb8862f297519d2601228d1301ce50306cfb30dd7f854c54f64b2848919c9\
    b493dcc57ba710df763b7f5978be9a255036cf2388a9776d1f36b57575";
const SEALED_RESPONSE_VECTOR: &str =
    "01666666666666666666666666ca7eba7d5d7435fe4ae2e3dc7e69037a8ee4\
    1a3f2fdbb01cd701f9e6d3f54ad2d9ab8fd688219b29b1e405692da8af400a01fa636a8c9da9db7323ac28df6bad\
    29dff31e0743f635f0030a7452d640307cf692c021b435c7c5fd5ffdb1";
