//! The registry source, driven through the downloader's one entry point: the
//! bytes must verify against the blob id governance named, and the row must
//! land under the application id governance named.

use std::sync::Arc;

use calimero_app_downloader::registry::{RegistryConfig, RegistryMode};
use calimero_app_downloader::source::dht::PeerBlobs;
use calimero_app_downloader::{
    app_source, AppRequest, ApplicationDownloader, DownloadError, Outcome,
};
use calimero_primitives::application::{ApplicationId, ApplicationSource};
use calimero_primitives::blobs::BlobId;
use calimero_store::db::InMemoryDB;
use calimero_store::{key, types, Store};
use tempfile::TempDir;
use url::Url;

mod common;

/// The coordinates every artifact below is published under.
const PACKAGE: &str = "com.example.app";
const VERSION: &str = "1.0.0";

/// The scheme+authority of a served URL, which is what an operator puts in
/// `[registry].base_url`. The fixture answers any path.
fn base_of(url: &Url) -> Url {
    url.join("/").expect("base")
}

fn req(bytecode_id: BlobId, application_id: ApplicationId) -> AppRequest<'static> {
    AppRequest {
        bytecode_id: Some(bytecode_id),
        application_id: Some(application_id),
        package: PACKAGE,
        version: VERSION,
        context_id: None,
    }
}

/// An http node has no peer source at all, so nothing here can reach one.
#[derive(Debug)]
struct NoPeers;

#[async_trait::async_trait]
impl PeerBlobs for NoPeers {
    async fn fetch_bytecode_from_peers(
        &self,
        _bytecode_id: &BlobId,
        _context_id: &calimero_primitives::context::ContextId,
    ) -> eyre::Result<Option<Arc<[u8]>>> {
        unreachable!("an http node builds no peer source")
    }
}

async fn download(
    node_client: &calimero_node_primitives::client::NodeClient,
    base: &Url,
    req: &AppRequest<'_>,
) -> Result<Outcome, DownloadError> {
    let source = app_source(
        &RegistryConfig::new(RegistryMode::Http, Some(base.clone())),
        NoPeers,
    )
    .expect("a base_url is configured");
    ApplicationDownloader::new(node_client.clone(), source)
        .download(req)
        .await
}

#[tokio::test]
async fn installs_a_bundle_from_the_registry_under_the_named_application_id() {
    let (bundle, named_id) = common::minimal_signed_bundle_bytes(PACKAGE, VERSION);
    let expected_blob = common::blob_id_of(&bundle).await;

    let (node_client, _data, _blobs) = common::create_test_node_client(None).await;
    let (url, server) = common::serve_once(bundle).await;

    assert_eq!(
        download(&node_client, &base_of(&url), &req(expected_blob, named_id))
            .await
            .expect("the walk must not fault"),
        Outcome::Installed
    );
    let _ignored = server.await;

    let row = node_client
        .get_application(&named_id)
        .expect("row read")
        .expect("row must exist under the id governance named");
    assert_eq!(
        row.blob.bytecode, expected_blob,
        "row must point at the verified blob"
    );
    assert!(
        node_client.has_blob(&expected_blob).expect("blob lookup"),
        "bytes must be in the local blobstore"
    );
}

#[tokio::test]
async fn refuses_bytes_that_do_not_match_the_named_blob_id() {
    let (served, _served_id) = common::minimal_signed_bundle_bytes(PACKAGE, VERSION);
    let (other, _other_id) = common::minimal_signed_bundle_bytes("com.example.other", "9.9.9");
    let wrong_expectation = common::blob_id_of(&other).await;
    let named_id = ApplicationId::from([0xAC; 32]);

    let (node_client, _data, _blobs) = common::create_test_node_client(None).await;
    let (url, server) = common::serve_once(served).await;

    let _refused = download(
        &node_client,
        &base_of(&url),
        &req(wrong_expectation, named_id),
    )
    .await
    .expect_err("mismatched bytes must be refused");
    let _ignored = server.await;

    assert!(
        node_client
            .get_application(&named_id)
            .expect("row read")
            .is_none(),
        "a refused install must write no row"
    );
    assert!(
        node_client.list_blobs().expect("list").is_empty(),
        "a refused install must leave no bytes behind"
    );
}

fn row(store: &Store, application_id: ApplicationId) -> types::ApplicationMeta {
    let handle = store.handle();
    handle
        .get(&key::ApplicationMeta::new(application_id))
        .expect("row read")
        .expect("row must exist under the id governance named")
}

/// A raw-wasm id folds in per-node values, so its row is adopted under the
/// named id - and records the coordinates it was actually fetched from.
#[tokio::test]
async fn a_raw_wasm_registry_install_records_its_coordinates() {
    let bytes = b"raw wasm, not a bundle".to_vec();
    let expected_blob = common::blob_id_of(&bytes).await;
    let named_id = ApplicationId::from([0xB1; 32]);

    let store = Store::new(Arc::new(InMemoryDB::owned()));
    let (node_client, _data, _blobs) = common::create_test_node_client(Some(store.clone())).await;
    let (url, server) = common::serve_once(bytes).await;

    assert_eq!(
        download(&node_client, &base_of(&url), &req(expected_blob, named_id))
            .await
            .expect("the walk must not fault"),
        Outcome::Installed
    );
    let _ignored = server.await;

    let row = row(&store, named_id);
    assert_eq!(&*row.package, PACKAGE);
    assert_eq!(&*row.version, VERSION);
}

/// A locally built app is published nowhere. Absent coordinates must stay
/// absent: a placeholder would aim the resolver at a URL nobody published.
#[tokio::test]
async fn absent_coordinates_are_written_as_absent() {
    let named_id = ApplicationId::from([0xB2; 32]);
    let source: ApplicationSource = "file:///home/dev/app.wasm".parse().expect("source");

    let store = Store::new(Arc::new(InMemoryDB::owned()));
    let (node_client, _data, _blobs) = common::create_test_node_client(Some(store.clone())).await;

    node_client
        .write_application_row(&named_id, &BlobId::from([0x33; 32]), 12, &source, None)
        .expect("row write");

    let row = row(&store, named_id);
    assert!(row.package.is_empty(), "got package {:?}", row.package);
    assert!(row.version.is_empty(), "got version {:?}", row.version);
}

/// A bundle that verifies byte-for-byte but names a different application must
/// leave nothing behind: no bytes, and no row under the id it does name - that
/// row is another application's, and pointing it at a released blob breaks it.
#[tokio::test]
async fn a_failed_bind_writes_no_row_and_releases_every_blob() {
    let (bundle, derived_id) = common::signed_bundle_bytes(PACKAGE, VERSION, &["alpha", "beta"]);
    let expected_blob = common::blob_id_of(&bundle).await;
    let named_id = ApplicationId::from([0xAC; 32]);

    let (node_client, _data, _blobs) = common::create_test_node_client(None).await;
    let (url, server) = common::serve_once(bundle).await;

    let _refused = download(&node_client, &base_of(&url), &req(expected_blob, named_id))
        .await
        .expect_err("an artifact naming another application must be refused");
    let _ignored = server.await;

    assert!(
        node_client
            .get_application(&derived_id)
            .expect("row read")
            .is_none(),
        "a refused install must write no row, not even under the id the \
         artifact itself derives"
    );
    assert!(
        node_client.list_blobs().expect("list").is_empty(),
        "a refused install must leave no bytes behind - neither the bundle \
         nor a service blob, and there is no content-addressed GC"
    );
}

/// Redirect-to-object-storage is how a registry serves artifacts, and a private
/// registry's storage host is private too. The blob id stays the byte authority.
#[tokio::test]
async fn a_registry_redirect_to_a_private_host_is_followed() {
    let (bundle, named_id) = common::minimal_signed_bundle_bytes(PACKAGE, VERSION);
    let expected_blob = common::blob_id_of(&bundle).await;

    let (node_client, _data, _blobs) = common::create_test_node_client(None).await;
    let (target, target_server) = common::serve_once(bundle).await;
    let (entry, entry_server) = common::redirect_once(&target).await;

    assert_eq!(
        download(
            &node_client,
            &base_of(&entry),
            &req(expected_blob, named_id)
        )
        .await
        .expect("the walk must not fault"),
        Outcome::Installed,
        "the operator's own registry must be able to redirect"
    );
    let _ignored = entry_server.await;
    let _ignored = target_server.await;

    assert!(node_client.has_blob(&expected_blob).expect("blob lookup"));
}

/// A node whose registry is `base`, for the bare install-by-coordinates path.
async fn node_pointed_at(
    base: &Url,
) -> (
    calimero_node_primitives::client::NodeClient,
    TempDir,
    TempDir,
) {
    let (node_client, data, blobs) = common::create_test_node_client(None).await;
    let registry = RegistryConfig::new(RegistryMode::Http, Some(base.clone()));
    (node_client.with_registry(registry), data, blobs)
}

/// A bare install names no application id, so the verified manifest is what
/// decides where the row lands.
#[tokio::test]
async fn a_bare_install_lands_under_the_manifest_derived_id() {
    let (bundle, derived_id) = common::minimal_signed_bundle_bytes(PACKAGE, VERSION);

    let (url, server) = common::serve_once(bundle).await;
    let (node_client, _data, _blobs) = node_pointed_at(&base_of(&url)).await;

    assert_eq!(
        node_client
            .install_by_coords(PACKAGE, VERSION)
            .await
            .expect("the install must not fault"),
        Some(derived_id)
    );
    let _ignored = server.await;

    assert!(node_client
        .get_application(&derived_id)
        .expect("row read")
        .is_some());
}

/// Nothing published at these coordinates is not a fault: the caller is told
/// the source had nothing, and retries.
#[tokio::test]
async fn a_bare_install_reports_unpublished_coordinates_as_absent() {
    let (url, server) = common::serve_status_once("404 Not Found").await;
    let (node_client, _data, _blobs) = node_pointed_at(&base_of(&url)).await;

    assert_eq!(
        node_client
            .install_by_coords(PACKAGE, VERSION)
            .await
            .expect("an unpublished version is not a fault"),
        None
    );
    let _ignored = server.await;
}

/// A bare install has no id to check the artifact against, so the coordinates
/// are the only promise the registry made. A signed bundle for some other
/// package still satisfies its own signature - it must not install here.
#[tokio::test]
async fn a_bare_install_refuses_a_substituted_package() {
    let (bundle, substituted_id) =
        common::minimal_signed_bundle_bytes("com.example.other", VERSION);

    let (url, server) = common::serve_once(bundle).await;
    let (node_client, _data, _blobs) = node_pointed_at(&base_of(&url)).await;

    let err = node_client
        .install_by_coords(PACKAGE, VERSION)
        .await
        .expect_err("a substituted package must be refused");
    let _ignored = server.await;

    let err = err.to_string();
    assert!(
        err.contains(PACKAGE) && err.contains("com.example.other"),
        "error must name both packages, got: {err}"
    );
    assert!(
        node_client
            .get_application(&substituted_id)
            .expect("row read")
            .is_none(),
        "a refused install must write no row"
    );
    assert!(
        node_client.list_blobs().expect("list").is_empty(),
        "a refused install must leave no bytes behind"
    );
}

/// An application id is derived from (package, signer) and is therefore
/// version-stable, so a sibling release of the SAME package - an older,
/// vulnerable one, say - satisfies the signature, the package check and the
/// derived id alike. Only the version the caller asked for separates them.
#[tokio::test]
async fn a_bare_install_refuses_a_substituted_version() {
    let (bundle, _id) = common::minimal_signed_bundle_bytes(PACKAGE, "0.9.0");

    let (url, server) = common::serve_once(bundle).await;
    let (node_client, _data, _blobs) = node_pointed_at(&base_of(&url)).await;

    let err = node_client
        .install_by_coords(PACKAGE, VERSION)
        .await
        .expect_err("a substituted version must be refused");
    let _ignored = server.await;

    let err = err.to_string();
    assert!(
        err.contains("0.9.0") && err.contains(VERSION),
        "error must name both versions, got: {err}"
    );
}
