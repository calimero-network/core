//! What sealing costs a request: the client seals it, the server's `intercept`
//! opens it and seals the answer, the client opens that, over loopback TCP
//! against a handler that reads the body and answers its length. Beside it the
//! same request sent plain to the same server, so the difference is the
//! envelope (AES-256-GCM both ways, framing, the replay window), not HTTP.
//! The Noise handshake runs once, before the timed rounds, as a session lasts.

use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::body::Bytes;
use axum::routing::post;
use axum::Router;
use calimero_sealed::client::SealedTransport as Client;
use calimero_sealed::{intercept, SealedOptions, SealedTransport};
use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use tokio::runtime::Runtime;
use url::Url;

/// A small JSON call, the largest frame, a large upload.
const SIZES: [usize; 3] = [1024, 64 * 1024, 1024 * 1024];

async fn length(body: Bytes) -> String {
    body.len().to_string()
}

async fn serve() -> (Url, Arc<SealedTransport>) {
    let transport = Arc::new(SealedTransport::generate(&SealedOptions::default()));
    let app = Router::new().route("/{*rest}", post(length));
    // The envelope wraps the router from outside, as the node does.
    let app = tower::Layer::layer(
        &axum::middleware::from_fn_with_state(Arc::clone(&transport), intercept),
        app,
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("a loopback port");
    let addr = listener.local_addr().expect("a bound address");
    let _server = tokio::spawn(async move {
        axum::serve(
            listener,
            axum::ServiceExt::<axum::extract::Request>::into_make_service(app),
        )
        .await
        .expect("the server runs");
    });
    let url = format!("http://{addr}/").parse().expect("a valid url");
    (url, transport)
}

/// Send `iters` requests carrying `body` one after another, sealed or plain.
fn timed(
    rt: &Runtime,
    iters: u64,
    url: &Url,
    http: &reqwest::Client,
    sealed: Option<&Client>,
    body: &[u8],
) -> Duration {
    rt.block_on(async {
        let target = url.join("apps/x/intents").expect("a valid path");
        let start = Instant::now();
        for _ in 0..iters {
            let request = http
                .post(target.clone())
                .body(body.to_vec())
                .build()
                .expect("a well-formed request");
            let response = match sealed {
                Some(client) => client.execute(request).await,
                None => http.execute(request).await.map_err(eyre::Report::from),
            }
            .expect("the server answers");
            let answer = response.text().await.expect("a body");
            assert_eq!(answer, body.len().to_string());
        }
        start.elapsed()
    })
}

fn request(c: &mut Criterion) {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("a runtime");
    let (url, transport) = rt.block_on(serve());
    let http = reqwest::Client::new();
    let client = Client::with_key(url.clone(), http.clone(), transport.public_key());
    // Open the session before timing: a handshake is once per session.
    let _warm = timed(&rt, 1, &url, &http, Some(&client), b"warm");

    let mut group = c.benchmark_group("request");
    for size in SIZES {
        let body = vec![0xAB_u8; size];
        group.throughput(Throughput::Bytes(size as u64));
        group.bench_with_input(BenchmarkId::new("sealed", size), &body, |b, body| {
            b.iter_custom(|iters| timed(&rt, iters, &url, &http, Some(&client), body));
        });
        group.bench_with_input(BenchmarkId::new("plain", size), &body, |b, body| {
            b.iter_custom(|iters| timed(&rt, iters, &url, &http, None, body));
        });
    }
    group.finish();
}

criterion_group!(benches, request);
criterion_main!(benches);
