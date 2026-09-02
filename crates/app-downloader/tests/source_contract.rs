//! What a source owes the downloader: unverified bytes, `Ok(None)` when it
//! simply had none yet, and `Err` only for a real fault.

use async_trait::async_trait;
use calimero_app_downloader::source::dht::{DhtRegistry, PeerBlobs};
use calimero_app_downloader::source::http::HttpRegistry;
use calimero_app_downloader::source::{AppRequest, AppSource};
use calimero_primitives::application::ApplicationId;
use calimero_primitives::blobs::BlobId;
use calimero_primitives::context::ContextId;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::task::JoinHandle;
use url::Url;

/// The coordinates every request below is published under.
const PACKAGE: &str = "com.example.app";
const VERSION: &str = "1.0.0";

/// Answer exactly one request on a loopback port with `response` verbatim.
/// Loopback also proves no host guard applies to the operator's own base.
async fn respond_once(response: Vec<u8>) -> (Url, JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind loopback");
    let addr = listener.local_addr().expect("local addr");
    let handle = tokio::spawn(async move {
        if let Ok((mut sock, _)) = listener.accept().await {
            let mut scratch = [0_u8; 1024];
            let _ignored = sock.read(&mut scratch).await;
            let _ignored = sock.write_all(&response).await;
            let _ignored = sock.flush().await;
        }
    });
    let base = format!("http://{addr}/").parse().expect("valid url");
    (base, handle)
}

async fn serve_once(body: Vec<u8>) -> (Url, JoinHandle<()>) {
    let mut response = format!(
        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: application/octet-stream\r\n\r\n",
        body.len()
    )
    .into_bytes();
    response.extend_from_slice(&body);
    respond_once(response).await
}

fn req(context_id: Option<&ContextId>) -> AppRequest<'_> {
    AppRequest {
        application_id: Some(ApplicationId::from([0x22; 32])),
        package: PACKAGE,
        version: VERSION,
        bytecode_id: Some(BlobId::from([0x33; 32])),
        context_id,
    }
}

#[tokio::test]
async fn http_source_returns_bytes_verbatim() {
    let bundle = b"a bundle, verbatim".to_vec();
    let (base, server) = serve_once(bundle.clone()).await;

    let fetched = HttpRegistry::new(base)
        .expect("client")
        .fetch(&req(None))
        .await
        .expect("a served artifact is not a fault");
    let _ignored = server.await;

    assert_eq!(
        fetched.as_deref(),
        Some(bundle.as_slice()),
        "the source must hand over the bytes it read, unverified and unaltered"
    );
}

// Not published there yet is not a fault: the caller retries on next access.
#[tokio::test]
async fn http_source_returns_none_on_404() {
    let (base, server) =
        respond_once(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n".to_vec()).await;

    let fetched = HttpRegistry::new(base)
        .expect("client")
        .fetch(&req(None))
        .await
        .expect("a 404 must not be reported as a fault");
    let _ignored = server.await;

    assert!(fetched.is_none());
}

/// An unusable base is the same on every fetch, so it is refused when the
/// source is built rather than once a download is in flight.
#[test]
fn http_source_refuses_a_base_it_cannot_address() {
    let base: Url = "mailto:ops@example.com".parse().expect("valid");
    let err = HttpRegistry::new(base).expect_err("an unaddressable base must not build a source");
    assert!(
        err.to_string().contains("mailto"),
        "error must name the refused scheme, got: {err}"
    );
}

/// Peers that answer nothing: the no-context arm must return before the peer
/// route asks them anything.
#[derive(Debug)]
struct NeverAsked;

#[async_trait]
impl PeerBlobs for NeverAsked {
    async fn fetch_bytecode_from_peers(
        &self,
        _bytecode_id: &BlobId,
        _context_id: &ContextId,
    ) -> eyre::Result<Option<std::sync::Arc<[u8]>>> {
        unreachable!("the peer route must not reach a peer without a context")
    }
}

// The peer route authorizes by context membership, so without one it has
// nothing to ask - which is "nobody had the bytes", never a fault.
#[tokio::test]
async fn dht_source_requires_a_context() {
    let fetched = DhtRegistry::new(NeverAsked)
        .fetch(&req(None))
        .await
        .expect("a missing context is not a fault");
    assert!(fetched.is_none());
}

/// An empty coordinate is a caller bug; reporting it as "not published" would
/// describe a fetch that never ran.
#[tokio::test]
async fn an_empty_coordinate_is_a_rejection_not_an_absence() {
    let (base, _server) = serve_once(b"never fetched".to_vec()).await;
    let source = HttpRegistry::new(base).expect("client");

    let mut request = req(None);
    request.package = "";
    request.version = "";

    let err = source
        .fetch(&request)
        .await
        .expect_err("an empty coordinate cannot address an artifact");
    assert!(
        err.to_string().contains("cannot address an artifact"),
        "unexpected error: {err}"
    );
}

/// A coordinate that cannot become a path segment is a rejection, not an
/// absence - `Ok(None)` would describe a fetch that never happened.
#[tokio::test]
async fn a_rejected_coordinate_names_itself_in_the_error() {
    let (base, _server) = serve_once(b"never fetched".to_vec()).await;
    let source = HttpRegistry::new(base).expect("client");

    let mut request = req(None);
    request.version = "1.0.0/../../secret";

    let err = source
        .fetch(&request)
        .await
        .expect_err("an unaddressable coordinate must not read as absent");
    assert!(
        err.to_string().contains("1.0.0/../../secret"),
        "error must name the offending coordinate, got: {err}"
    );
}

/// An artifact lives at its coordinates on the configured registry, so there is
/// nowhere legitimate for a redirect to point. Following one would let whatever
/// answers the base choose the next address this node connects to.
#[tokio::test]
async fn http_source_refuses_a_redirect_rather_than_following_it() {
    let (base, server) = respond_once(
        b"HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:1/evil\r\nContent-Length: 0\r\n\r\n"
            .to_vec(),
    )
    .await;
    let err = HttpRegistry::new(base)
        .expect("client")
        .fetch(&req(None))
        .await
        .expect_err("a redirect must be refused, not followed");
    let _ignored = server.await;
    assert!(
        err.to_string().contains("302"),
        "error must report the redirect status, got: {err}"
    );
}
