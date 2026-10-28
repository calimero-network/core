#![no_main]
//! The sealed channel's front door: the first bytes any client or peer sends a
//! node, before anything is authenticated. Every request goes through
//! `calimero_sealed::intercept`, which parses the envelope header, looks up
//! the session, admits the request id and opens the AEAD.
//!
//! The first input byte picks the handshake or the sealed endpoint; the rest
//! is the body. The node must answer every input without a panic, and must
//! never hand an unopened body to the router behind it.

use std::sync::{Arc, OnceLock};

use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use axum::routing::any;
use axum::Router;
use calimero_sealed::{intercept, SealedOptions, SealedTransport, HANDSHAKE_PATH, SEALED_PATH};
use libfuzzer_sys::fuzz_target;
use tower::ServiceExt;

fn runtime() -> &'static tokio::runtime::Runtime {
    static RT: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RT.get_or_init(|| {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime")
    })
}

fn router() -> Router {
    static TRANSPORT: OnceLock<Arc<SealedTransport>> = OnceLock::new();
    let transport = TRANSPORT
        .get_or_init(|| {
            Arc::new(SealedTransport::generate(&SealedOptions {
                required: true,
                unsealed_paths: vec!["/health".into()],
            }))
        })
        .clone();
    // A route reached only through an opened envelope says so.
    Router::new()
        .route("/{*path}", any(|| async { StatusCode::IM_A_TEAPOT }))
        .layer(axum::middleware::from_fn_with_state(transport, intercept))
}

fuzz_target!(|data: &[u8]| {
    let Some((&pick, body)) = data.split_first() else {
        return;
    };
    let path = if pick & 1 == 0 { HANDSHAKE_PATH } else { SEALED_PATH };
    let request = Request::builder()
        .method(Method::POST)
        .uri(path)
        .header("content-type", "application/octet-stream")
        .body(Body::from(body.to_vec()))
        .expect("request");
    let status = runtime()
        .block_on(router().oneshot(request))
        .expect("infallible")
        .status();
    // No session was ever opened, so no envelope can open: nothing reaches the router.
    assert_ne!(status, StatusCode::IM_A_TEAPOT, "an unopened request reached the router");
});
