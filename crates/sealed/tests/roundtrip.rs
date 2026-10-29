//! The client and server halves against each other over real TCP.

use std::sync::Arc;

use axum::body::Bytes;
use axum::http::{HeaderMap, Method, Uri};
use axum::response::sse::{Event, Sse};
use axum::response::IntoResponse;
use axum::routing::{any, get};
use axum::Extension;
use axum::Router;
use calimero_sealed::client::{SealedTransport as Client, StaleTransportKey};
use calimero_sealed::{intercept, PeerKey, SealedOptions, SealedTransport};
use futures_util::StreamExt;
use url::Url;

async fn echo(method: Method, uri: Uri, headers: HeaderMap, body: Bytes) -> impl IntoResponse {
    let auth = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    axum::Json(serde_json::json!({
        "method": method.as_str(),
        "path": uri.path(),
        "auth": auth,
        "origin": headers.get("origin").and_then(|v| v.to_str().ok()),
        "forwardedHost": headers.get("x-forwarded-host").and_then(|v| v.to_str().ok()),
        "body": String::from_utf8_lossy(&body),
    }))
}

async fn events() -> impl IntoResponse {
    let stream = futures_util::stream::iter(
        (0..3).map(|i| Ok::<_, std::convert::Infallible>(Event::default().data(format!("e{i}")))),
    );
    Sse::new(stream)
}

/// Which node the opened request says is calling.
async fn caller(peer: Option<Extension<PeerKey>>) -> impl IntoResponse {
    axum::Json(serde_json::json!({
        "peer": peer.map(|Extension(PeerKey(key))| hex::encode(key)),
    }))
}

/// Attests a key known in advance.
#[derive(Debug)]
struct Known([u8; 32]);

impl calimero_sealed::client::Attest for Known {
    fn attest_transport_key<'a>(
        &'a self,
        _http: &'a reqwest::Client,
        _api_url: &'a Url,
    ) -> futures_util::future::BoxFuture<'a, eyre::Result<[u8; 32]>> {
        Box::pin(async move { Ok(self.0) })
    }
}

async fn serve(options: SealedOptions) -> (Url, Arc<SealedTransport>, tokio::task::JoinHandle<()>) {
    let transport = Arc::new(SealedTransport::generate(&options));
    let app = Router::new()
        .route("/events", get(events))
        .route("/caller", get(caller))
        .route("/{*rest}", any(echo));
    // The envelope wraps the router from outside, so an opened request is routed afresh.
    let app = tower::Layer::layer(
        &axum::middleware::from_fn_with_state(Arc::clone(&transport), intercept),
        app,
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let handle = tokio::spawn(async move {
        axum::serve(
            listener,
            axum::ServiceExt::<axum::extract::Request>::into_make_service(app),
        )
        .await
        .unwrap();
    });
    (
        format!("http://{addr}/").parse().unwrap(),
        transport,
        handle,
    )
}

#[tokio::test]
async fn request_and_response_round_trip() {
    let (url, transport, _h) = serve(SealedOptions::default()).await;
    let http = reqwest::Client::new();
    let client = Client::with_key(url.clone(), http.clone(), transport.public_key());
    for i in 0..3 {
        let req = http
            .post(url.join("apps/x/intents").unwrap())
            .header("authorization", "Bearer secret-token")
            .body(format!("hello {i}"))
            .build()
            .unwrap();
        let resp = client.execute(req).await.unwrap();
        assert_eq!(resp.status(), 200);
        let v: serde_json::Value = resp.json().await.unwrap();
        assert_eq!(v["path"], "/apps/x/intents");
        assert_eq!(v["auth"], "Bearer secret-token");
        assert_eq!(v["body"], format!("hello {i}"));
    }
}

#[tokio::test]
async fn event_stream_opens_frame_by_frame() {
    let (url, transport, _h) = serve(SealedOptions::default()).await;
    let http = reqwest::Client::new();
    let client = Client::with_key(url.clone(), http.clone(), transport.public_key());
    let req = http.get(url.join("events").unwrap()).build().unwrap();
    let resp = client.execute(req).await.unwrap();
    let mut body = String::new();
    let mut stream = resp.bytes_stream();
    while let Some(chunk) = stream.next().await {
        body.push_str(&String::from_utf8_lossy(&chunk.unwrap()));
    }
    assert!(
        body.contains("data: e0") && body.contains("data: e2"),
        "{body}"
    );
}

#[tokio::test]
async fn a_wrong_key_is_reported_stale() {
    let (url, _transport, _h) = serve(SealedOptions::default()).await;
    let http = reqwest::Client::new();
    let client = Client::with_key(url.clone(), http.clone(), [9; 32]);
    let req = http.get(url.join("x").unwrap()).build().unwrap();
    let err = client.execute(req).await.unwrap_err();
    assert!(err.downcast_ref::<StaleTransportKey>().is_some(), "{err:?}");
}

#[tokio::test]
async fn required_mode_refuses_plain_requests() {
    let options = SealedOptions {
        required: true,
        unsealed_paths: vec!["/attestation".into()],
        ..SealedOptions::default()
    };
    let (url, _transport, _h) = serve(options).await;
    let plain = reqwest::get(url.join("apps/x/intents").unwrap())
        .await
        .unwrap();
    assert_eq!(plain.status(), 403);
    let allowed = reqwest::get(url.join("attestation").unwrap())
        .await
        .unwrap();
    assert_eq!(allowed.status(), 200);
}

/// Xil's review, issue 13: a relay between client and node set the outer
/// Origin and X-Forwarded-Host, and the node read them as the sealed request's.
#[tokio::test]
async fn a_relay_cannot_choose_the_origin_the_node_reads() {
    let (url, transport, _h) = serve(SealedOptions::default()).await;
    // A relay that forwards every request, adding its own outer headers.
    let upstream = url.clone();
    let relay = Router::new().route(
        "/{*rest}",
        any(
            move |method: Method, uri: Uri, headers: HeaderMap, body: Bytes| {
                let upstream = upstream.clone();
                async move {
                    let mut req = reqwest::Client::new()
                        .request(
                            method,
                            upstream.join(uri.path().trim_start_matches('/')).unwrap(),
                        )
                        .header("origin", "https://evil.example")
                        .header("x-forwarded-host", "evil.example")
                        .body(body);
                    for (name, value) in &headers {
                        if name != "host" && name != "content-length" {
                            req = req.header(name, value);
                        }
                    }
                    let resp = req.send().await.unwrap();
                    let status = resp.status();
                    let headers = resp.headers().clone();
                    let bytes = resp.bytes().await.unwrap();
                    let mut out = (status, bytes).into_response();
                    for (name, value) in &headers {
                        if name != "content-length" && name != "transfer-encoding" {
                            let _prev = out.headers_mut().insert(name, value.clone());
                        }
                    }
                    out
                }
            },
        ),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let relay_url: Url = format!("http://{}/", listener.local_addr().unwrap())
        .parse()
        .unwrap();
    let _relay = tokio::spawn(async move { axum::serve(listener, relay).await.unwrap() });

    let http = reqwest::Client::new();
    let client = Client::with_key(relay_url.clone(), http.clone(), transport.public_key());
    let req = http
        .post(relay_url.join("apps/x/intents").unwrap())
        .body("hi")
        .build()
        .unwrap();
    let resp = client.execute(req).await.unwrap();
    assert_eq!(resp.status(), 200);
    let v: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(v["origin"], serde_json::Value::Null, "{v}");
    assert_eq!(v["forwardedHost"], serde_json::Value::Null, "{v}");
}

/// POST `len` bytes to the envelope over a raw connection the way many clients
/// do (Python's, curl without `Expect`): the whole body first, and only then
/// read the answer. Answers the status line and the body, or the error that
/// sending or reading met.
async fn post_whole_then_read(url: &Url, len: usize, chunked: bool) -> std::io::Result<String> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let mut stream =
        tokio::net::TcpStream::connect((url.host_str().unwrap(), url.port().unwrap())).await?;
    let framing = if chunked {
        "transfer-encoding: chunked".to_owned()
    } else {
        format!("content-length: {len}")
    };
    let head = format!(
        "POST /sealed/v2 HTTP/1.1\r\nhost: node\r\n\
         content-type: application/vnd.calimero.sealed\r\n{framing}\r\n\r\n"
    );
    stream.write_all(head.as_bytes()).await?;
    // A version byte, then zeros: an envelope naming a session no node holds.
    let mut piece = vec![0u8; 1 << 20];
    piece[0] = 2;
    let mut sent = 0;
    while sent < len {
        let n = piece.len().min(len - sent);
        if chunked {
            stream.write_all(format!("{n:x}\r\n").as_bytes()).await?;
        }
        stream.write_all(&piece[..n]).await?;
        if chunked {
            stream.write_all(b"\r\n").await?;
        }
        piece[0] = 0;
        sent += n;
    }
    if chunked {
        stream.write_all(b"0\r\n\r\n").await?;
    }
    let mut answer = Vec::new();
    let mut buf = [0u8; 4096];
    loop {
        let n = stream.read(&mut buf).await?;
        answer.extend_from_slice(&buf[..n]);
        let text = String::from_utf8_lossy(&answer);
        // One small JSON refusal: the answer is complete once its body closes.
        if n == 0 || text.contains("\r\n\r\n") && text.trim_end().ends_with('}') {
            return Ok(text.into_owned());
        }
    }
}

/// merobox's 100 and 250 MiB blob uploads met a closed connection instead of
/// the 413 the node had sent: it answered at once and closed with the body
/// unread, and that reset threw the answer away before the client, still
/// sending, read it.
#[tokio::test]
async fn a_request_past_the_cap_is_answered_after_it_is_sent() {
    let (url, _transport, _h) = serve(SealedOptions::default()).await;
    let cap = calimero_sealed::server::MAX_SEALED_BYTES;
    for len in [cap + 1, 100 << 20] {
        let answer = post_whole_then_read(&url, len, false)
            .await
            .unwrap_or_else(|e| panic!("{len} bytes declared: no answer: {e}"));
        assert!(answer.starts_with("HTTP/1.1 413"), "{answer}");
        assert!(
            answer.contains(r#""code":"too_large""#) && answer.contains("64 MiB"),
            "{answer}"
        );
    }
    // Chunked, the length is unknown up front; the session is refused at its
    // first bytes and the rest read past the cap all the same.
    let answer = post_whole_then_read(&url, 100 << 20, true)
        .await
        .unwrap_or_else(|e| panic!("chunked: no answer: {e}"));
    assert!(answer.starts_with("HTTP/1.1 4"), "{answer}");
}

/// A node calling a peer proves its own transport key (Noise IK), and every
/// request of that session names it; an anonymous session names nobody.
#[tokio::test]
async fn an_identified_session_names_its_caller() {
    let (url, transport, _h) = serve(SealedOptions::default()).await;
    let http = reqwest::Client::new();
    let me = SealedTransport::generate(&SealedOptions::default());
    let peer = Client::identified_as(
        url.clone(),
        http.clone(),
        Arc::new(Known(transport.public_key())),
        me.static_key(),
    );
    for _ in 0..2 {
        let req = http.get(url.join("caller").unwrap()).build().unwrap();
        let v: serde_json::Value = peer.execute(req).await.unwrap().json().await.unwrap();
        assert_eq!(v["peer"], hex::encode(me.public_key()));
    }

    let anonymous = Client::with_key(url.clone(), http.clone(), transport.public_key());
    let req = http.get(url.join("caller").unwrap()).build().unwrap();
    let v: serde_json::Value = anonymous.execute(req).await.unwrap().json().await.unwrap();
    assert_eq!(v["peer"], serde_json::Value::Null);
}

/// A static key the caller does not hold cannot be claimed: the handshake
/// fails, so no session names it.
#[tokio::test]
async fn an_identified_handshake_with_another_nodes_key_is_refused() {
    let (url, transport, _h) = serve(SealedOptions::default()).await;
    let http = reqwest::Client::new();
    // An IK message 1 built with one static key, its sealed static key swapped
    // for another node's: the responder cannot open it.
    let other = SealedTransport::generate(&SealedOptions::default());
    let mut noise = snow::Builder::new("Noise_IK_25519_AESGCM_SHA256".parse().unwrap())
        .prologue(b"calimero/sealed-http/v2")
        .local_private_key(&[7; 32])
        .remote_public_key(&transport.public_key())
        .build_initiator()
        .unwrap();
    let mut message_1 = vec![0u8; 96];
    let len = noise.write_message(&[], &mut message_1).unwrap();
    message_1.truncate(len);
    message_1[32..64].copy_from_slice(&other.public_key());
    let body = [&[3u8][..], &transport.public_key(), &message_1].concat();
    let r = http
        .post(url.join("sealed/v2/handshake").unwrap())
        .body(body)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 400);
}
