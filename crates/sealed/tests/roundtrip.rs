//! The client and server halves against each other over real TCP.

use std::sync::Arc;

use axum::body::Bytes;
use axum::http::{HeaderMap, Method, Uri};
use axum::response::sse::{Event, Sse};
use axum::response::IntoResponse;
use axum::routing::{any, get};
use axum::Router;
use calimero_sealed::client::{SealedTransport as Client, StaleTransportKey};
use calimero_sealed::{intercept, SealedOptions, SealedTransport};
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
        "body": String::from_utf8_lossy(&body),
    }))
}

async fn events() -> impl IntoResponse {
    let stream = futures_util::stream::iter(
        (0..3).map(|i| Ok::<_, std::convert::Infallible>(Event::default().data(format!("e{i}")))),
    );
    Sse::new(stream)
}

async fn serve(options: SealedOptions) -> (Url, Arc<SealedTransport>, tokio::task::JoinHandle<()>) {
    let transport = Arc::new(SealedTransport::generate(&options));
    let app = Router::new()
        .route("/events", get(events))
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
