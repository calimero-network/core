//! One configured source, no second route. What matters is what ends up
//! installed and who was contacted to get it - never which legs were planned.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread;

use async_trait::async_trait;
use calimero_app_downloader::port::{ApplicationStore, InstalledApplication};
use calimero_app_downloader::registry::{RegistryConfig, RegistryCoords, RegistryMode};
use calimero_app_downloader::source::dht::PeerBlobs;
use calimero_app_downloader::source::Bytes;
use calimero_app_downloader::{app_source, AppRequest, ApplicationDownloader, Outcome};
use calimero_primitives::application::{ApplicationId, ApplicationSource};
use calimero_primitives::blobs::BlobId;
use calimero_primitives::context::ContextId;
use url::Url;

/// The coordinates every request below is published under.
const PACKAGE: &str = "com.example.app";
const VERSION: &str = "1.0.0";

/// The bytes a peer would serve, and the blob id the store hands back for them.
const BYTES: &[u8] = b"a bundle from somewhere";

fn bytecode_id() -> BlobId {
    BlobId::from([0x33; 32])
}

/// A registry that 404s every request and counts how often it was asked. A
/// refusal is enough: what is under test is whether it was dialled at all.
fn counting_registry() -> (Url, Arc<AtomicUsize>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
    let base = format!("http://{}", listener.local_addr().expect("local addr"))
        .parse()
        .expect("registry base");
    let hits = Arc::new(AtomicUsize::new(0));
    let seen = Arc::clone(&hits);
    let _serving = thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { break };
            let _previous = seen.fetch_add(1, Ordering::SeqCst);
            let _request = stream.read(&mut [0_u8; 1024]);
            let _served = stream.write_all(
                b"HTTP/1.1 404 Not Found\r\ncontent-length: 0\r\nconnection: close\r\n\r\n",
            );
        }
    });
    (base, hits)
}

/// Peers that would serve the bytes, counting every contact - so a route that
/// must not run can be pinned at zero rather than inferred from its result.
#[derive(Clone, Debug, Default)]
struct RecordingPeers {
    requests: Arc<AtomicUsize>,
}

#[async_trait]
impl PeerBlobs for RecordingPeers {
    async fn fetch_bytecode_from_peers(
        &self,
        _bytecode_id: &BlobId,
        _context_id: &ContextId,
    ) -> eyre::Result<Option<Bytes>> {
        let _previous = self.requests.fetch_add(1, Ordering::SeqCst);
        Ok(Some(Arc::from(BYTES)))
    }
}

/// A store holding nothing, counting what gets installed into it.
#[derive(Clone, Debug, Default)]
struct RecordingStore {
    installed: Arc<AtomicUsize>,
}

#[async_trait]
impl ApplicationStore for RecordingStore {
    fn has_bytecode(&self, _bytecode_id: &BlobId) -> eyre::Result<bool> {
        Ok(false)
    }

    fn installed_application(
        &self,
        _application_id: &ApplicationId,
    ) -> eyre::Result<Option<InstalledApplication>> {
        Ok(None)
    }

    async fn read_local_bytecode(&self, _bytecode_id: &BlobId) -> eyre::Result<Option<Arc<[u8]>>> {
        Ok(None)
    }

    async fn store_bytecode(&self, bytes: &[u8]) -> eyre::Result<(BlobId, u64)> {
        Ok((bytecode_id(), bytes.len() as u64))
    }

    async fn release_bytecode(&self, _bytecode_id: BlobId) -> eyre::Result<()> {
        Ok(())
    }

    async fn bind_application(
        &self,
        _application_id: &ApplicationId,
        _bytecode_id: BlobId,
        _size: u64,
        _source: &ApplicationSource,
        _coords: Option<RegistryCoords<'_>>,
        _bytes: &[u8],
    ) -> eyre::Result<()> {
        let _previous = self.installed.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

fn context() -> ContextId {
    ContextId::from([0x11; 32])
}

fn req(context_id: &ContextId) -> AppRequest<'_> {
    AppRequest {
        bytecode_id: Some(bytecode_id()),
        application_id: Some(ApplicationId::from([0x22; 32])),
        package: PACKAGE,
        version: VERSION,
        context_id: Some(context_id),
    }
}

// An http node resolves applications from its registry and nowhere else. A peer
// that would have served the bytes must never be asked, whatever the registry said.
#[tokio::test]
async fn http_mode_never_touches_peers() {
    let (base, _hits) = counting_registry();
    let store = RecordingStore::default();
    let peers = RecordingPeers::default();
    let source = app_source(
        &RegistryConfig::new(RegistryMode::Http, Some(base)),
        peers.clone(),
    )
    .expect("a base_url is configured");

    let context = context();
    let outcome = ApplicationDownloader::new(store.clone(), source)
        .download(&req(&context))
        .await
        .expect("an empty registry is not a fault");

    assert_eq!(outcome, Outcome::Unavailable);
    assert_eq!(
        peers.requests.load(Ordering::SeqCst),
        0,
        "an http node must not reach a peer, even one holding the bytes"
    );
    assert_eq!(store.installed.load(Ordering::SeqCst), 0);
}

// The inverse: a dht node resolves from peers and never dials a registry, even
// one it has configured.
#[tokio::test]
async fn dht_mode_never_touches_http() {
    let (base, hits) = counting_registry();
    let store = RecordingStore::default();
    let peers = RecordingPeers::default();
    let source = app_source(
        &RegistryConfig::new(RegistryMode::Dht, Some(base)),
        peers.clone(),
    )
    .expect("the peer route needs no configuration");

    let context = context();
    let outcome = ApplicationDownloader::new(store.clone(), source)
        .download(&req(&context))
        .await
        .expect("the peer route must not fault");

    assert_eq!(outcome, Outcome::Installed);
    assert_eq!(
        hits.load(Ordering::SeqCst),
        0,
        "a dht node must not dial the registry, even a configured one"
    );
    assert_eq!(peers.requests.load(Ordering::SeqCst), 1);
    assert_eq!(store.installed.load(Ordering::SeqCst), 1);
}

// There is no route behind the registry, so a node told to use one without
// saying where must fail where it is configured, not silently at fetch time.
#[test]
fn http_mode_without_a_base_url_has_no_source_at_all() {
    let err = app_source(
        &RegistryConfig::new(RegistryMode::Http, None),
        RecordingPeers::default(),
    )
    .expect_err("http mode with no base_url cannot resolve anything");
    assert!(
        err.to_string().contains("base_url"),
        "the error must name what is missing, got: {err}"
    );
}
