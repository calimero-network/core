//! The registry source through the downloader's one entry point: bytes verify
//! against the blob id governance named, and the row lands under its app id.

use std::sync::Arc;

use calimero_app_downloader::registry::{RegistryConfig, RegistryMode, PENDING_BLOB_SHARE_SOURCE};
use calimero_app_downloader::source::dht::PeerBlobs;
use calimero_app_downloader::{app_source, AppRequest, ApplicationDownloader, Outcome};
use calimero_node_primitives::client::application::{InstallOrigin, NotABundle};
use calimero_node_primitives::client::NodeClient;
use calimero_primitives::application::{ApplicationId, ApplicationSource};
use calimero_primitives::blobs::BlobId;
use calimero_store::db::InMemoryDB;
use calimero_store::{key, types, Store};
use ed25519_dalek::SigningKey;
use rand::rand_core::UnwrapErr;
use rand::rngs::SysRng;
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
) -> eyre::Result<Outcome> {
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

/// Raw wasm derives no id, so a group naming it could bind it under any id;
/// only a signed bundle is installed from a remote request.
#[tokio::test]
async fn downloaded_raw_wasm_is_refused_and_writes_no_row() {
    let raw = b"raw wasm, not a bundle".to_vec();
    let raw_blob = common::blob_id_of(&raw).await;
    let named_id = ApplicationId::from([0xB1; 32]);

    let (node_client, _data, _blobs) = common::create_test_node_client(None).await;
    let (url, server) = common::serve_once(raw).await;

    let err = download(&node_client, &base_of(&url), &req(raw_blob, named_id))
        .await
        .expect_err("raw wasm a group named must be refused");
    let _ignored = server.await;

    assert!(
        err.downcast_ref::<NotABundle>().is_some(),
        "refused as raw wasm, got: {err}"
    );
    assert!(
        node_client
            .get_application(&named_id)
            .expect("row read")
            .is_none(),
        "a refused install must write no row"
    );
    assert!(!node_client.has_blob(&raw_blob).expect("blob lookup"));
}

/// A bundle that verifies but names another application must leave nothing:
/// that row belongs to someone else, and a released blob would break it.
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

/// An artifact lives at its coordinates on the one configured registry, so a
/// redirect has nowhere legitimate to point and is refused rather than followed.
#[tokio::test]
async fn a_registry_redirect_is_refused() {
    let (bundle, named_id) = common::minimal_signed_bundle_bytes(PACKAGE, VERSION);
    let expected_blob = common::blob_id_of(&bundle).await;

    let (node_client, _data, _blobs) = common::create_test_node_client(None).await;
    let (target, target_server) = common::serve_once(bundle).await;
    let (entry, entry_server) = common::redirect_once(&target).await;

    let err = download(
        &node_client,
        &base_of(&entry),
        &req(expected_blob, named_id),
    )
    .await
    .expect_err("a redirect must be refused, not followed");
    let _ignored = entry_server.await;
    // Nothing reaches the redirect target, so its accept never returns.
    target_server.abort();

    assert!(
        err.to_string().contains("302"),
        "error must report the redirect status, got: {err}"
    );
    assert!(!node_client.has_blob(&expected_blob).expect("blob lookup"));
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
            .install_by_coords(PACKAGE, VERSION, InstallOrigin::Operator)
            .await
            .expect("the install must not fault")
            .map(|(id, _blob)| id),
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
            .install_by_coords(PACKAGE, VERSION, InstallOrigin::Operator)
            .await
            .expect("an unpublished version is not a fault"),
        None
    );
    let _ignored = server.await;
}

/// A bare install has no id to check against, so the coordinates are the only
/// promise made - another package's bundle signs just as validly.
#[tokio::test]
async fn a_bare_install_refuses_a_substituted_package() {
    let (bundle, substituted_id) =
        common::minimal_signed_bundle_bytes("com.example.other", VERSION);

    let (url, server) = common::serve_once(bundle).await;
    let (node_client, _data, _blobs) = node_pointed_at(&base_of(&url)).await;

    let err = node_client
        .install_by_coords(PACKAGE, VERSION, InstallOrigin::Operator)
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

/// An id is derived from (package, signer) and so is version-stable: a sibling
/// release passes signature, package and id alike. Only the version separates them.
#[tokio::test]
async fn a_bare_install_refuses_a_substituted_version() {
    let (bundle, _id) = common::minimal_signed_bundle_bytes(PACKAGE, "0.9.0");

    let (url, server) = common::serve_once(bundle).await;
    let (node_client, _data, _blobs) = node_pointed_at(&base_of(&url)).await;

    let err = node_client
        .install_by_coords(PACKAGE, VERSION, InstallOrigin::Operator)
        .await
        .expect_err("a substituted version must be refused");
    let _ignored = server.await;

    let err = err.to_string();
    assert!(
        err.contains("0.9.0") && err.contains(VERSION),
        "error must name both versions, got: {err}"
    );
}

/// Two releases of one application: same signer, so the same id.
fn two_releases(older: &str, newer: &str) -> (Vec<u8>, Vec<u8>, ApplicationId) {
    let key = SigningKey::generate(&mut UnwrapErr(SysRng));
    let (older, id) = common::signed_bundle_bytes_by(&key, PACKAGE, older, &[]);
    let (newer, newer_id) = common::signed_bundle_bytes_by(&key, PACKAGE, newer, &[]);
    assert_eq!(id, newer_id, "releases by one signer share an id");
    (older, newer, id)
}

/// A node over `store` whose one source is the registry at `base`.
async fn node_over(store: &Store, base: &Url) -> (NodeClient, TempDir, TempDir) {
    let (node_client, data, blobs) = common::create_test_node_client(Some(store.clone())).await;
    let registry = RegistryConfig::new(RegistryMode::Http, Some(base.clone()));
    (node_client.with_registry(registry), data, blobs)
}

/// Install `bundle` the way the admin API's dev install does.
async fn install_by_operator(node_client: &NodeClient, bundle: &[u8]) -> BlobId {
    let dir = TempDir::new().expect("temp dir");
    let path = dir.path().join("app.mpk");
    std::fs::write(&path, bundle).expect("write bundle");
    let _id = node_client
        .install_application_from_path(path.try_into().expect("utf-8 path"))
        .await
        .expect("operator install");
    common::blob_id_of(bundle).await
}

async fn store_blob(node_client: &NodeClient, bytes: &[u8]) -> BlobId {
    let (blob_id, _size) = node_client
        .add_blob(bytes, Some(bytes.len() as u64), None)
        .await
        .expect("store blob");
    blob_id
}

/// A base no test contacts: the bytes are already local.
fn unused_base() -> Url {
    "http://127.0.0.1:9/".parse().expect("base")
}

/// Any group can name any application id, so a download it triggers must not
/// roll this node's installed release back.
#[tokio::test]
async fn a_downloaded_older_release_never_replaces_the_installed_row() {
    let (older, newer, id) = two_releases("1.0.0", "2.0.0");
    let older_blob = common::blob_id_of(&older).await;
    let (url, server) = common::serve_once(older).await;

    let store = Store::new(Arc::new(InMemoryDB::owned()));
    let (node_client, _data, _blobs) = node_over(&store, &base_of(&url)).await;
    let newer_blob = install_by_operator(&node_client, &newer).await;

    assert_eq!(
        download(&node_client, &base_of(&url), &req(older_blob, id))
            .await
            .expect("an older release is still acquired for whoever binds it"),
        Outcome::Installed
    );
    let _ignored = server.await;

    let row = row(&store, id);
    assert_eq!(&*row.version, "2.0.0");
    assert_eq!(row.bytecode.blob_id(), newer_blob);
    assert!(node_client.has_blob(&older_blob).expect("blob lookup"));
}

#[tokio::test]
async fn a_held_older_release_never_replaces_the_installed_row() {
    let (older, newer, id) = two_releases("1.0.0", "2.0.0");

    let store = Store::new(Arc::new(InMemoryDB::owned()));
    let (node_client, _data, _blobs) = node_over(&store, &unused_base()).await;
    let newer_blob = install_by_operator(&node_client, &newer).await;
    let older_blob = store_blob(&node_client, &older).await;

    assert_eq!(
        download(&node_client, &unused_base(), &req(older_blob, id))
            .await
            .expect("the walk must not fault"),
        Outcome::Installed
    );

    let row = row(&store, id);
    assert_eq!(&*row.version, "2.0.0");
    assert_eq!(row.bytecode.blob_id(), newer_blob);
}

#[tokio::test]
async fn downloaded_raw_wasm_never_replaces_a_signed_row_and_is_released() {
    let (_older, newer, id) = two_releases("1.0.0", "2.0.0");
    let raw = b"raw wasm, not a bundle".to_vec();
    let raw_blob = common::blob_id_of(&raw).await;
    let (url, server) = common::serve_once(raw).await;

    let store = Store::new(Arc::new(InMemoryDB::owned()));
    let (node_client, _data, _blobs) = node_over(&store, &base_of(&url)).await;
    let newer_blob = install_by_operator(&node_client, &newer).await;

    let _refused = download(&node_client, &base_of(&url), &req(raw_blob, id))
        .await
        .expect_err("raw wasm must not replace a signed release");
    let _ignored = server.await;

    assert_eq!(row(&store, id).bytecode.blob_id(), newer_blob);
    assert!(!node_client.has_blob(&raw_blob).expect("blob lookup"));
}

#[tokio::test]
async fn a_downloaded_newer_release_replaces_the_installed_row() {
    let (older, newer, id) = two_releases("1.0.0", "2.0.0");
    let newer_blob = common::blob_id_of(&newer).await;
    let (url, server) = common::serve_once(newer).await;

    let store = Store::new(Arc::new(InMemoryDB::owned()));
    let (node_client, _data, _blobs) = node_over(&store, &base_of(&url)).await;
    let _older_blob = install_by_operator(&node_client, &older).await;

    assert_eq!(
        download(&node_client, &base_of(&url), &req(newer_blob, id))
            .await
            .expect("the walk must not fault"),
        Outcome::Installed
    );
    let _ignored = server.await;

    let row = row(&store, id);
    assert_eq!(&*row.version, "2.0.0");
    assert_eq!(row.bytecode.blob_id(), newer_blob);
}

/// A relay resolving a member's older target keeps the bundle and reports its
/// blob, without touching the row.
#[tokio::test]
async fn a_remote_coords_install_keeps_the_newer_row() {
    let (older, newer, id) = two_releases(VERSION, "2.0.0");
    let older_blob = common::blob_id_of(&older).await;
    let (url, server) = common::serve_once(older).await;

    let store = Store::new(Arc::new(InMemoryDB::owned()));
    let (node_client, _data, _blobs) = node_over(&store, &base_of(&url)).await;
    let newer_blob = install_by_operator(&node_client, &newer).await;

    let installed = node_client
        .install_by_coords(PACKAGE, VERSION, InstallOrigin::Remote)
        .await
        .expect("an older release is not a fault");
    let _ignored = server.await;

    assert_eq!(installed, Some((id, older_blob)));
    assert_eq!(row(&store, id).bytecode.blob_id(), newer_blob);
    assert!(node_client.has_blob(&older_blob).expect("blob lookup"));
}

/// A version that is not semver cannot be ordered against the row's release,
/// so the row keeps its release and the bundle stays a blob.
#[tokio::test]
async fn a_remote_coords_install_keeps_the_row_for_an_unordered_release() {
    let (unordered, newer, id) = two_releases("nightly", "2.0.0");
    let unordered_blob = common::blob_id_of(&unordered).await;
    let (url, server) = common::serve_once(unordered).await;

    let store = Store::new(Arc::new(InMemoryDB::owned()));
    let (node_client, _data, _blobs) = node_over(&store, &base_of(&url)).await;
    let newer_blob = install_by_operator(&node_client, &newer).await;

    let installed = node_client
        .install_by_coords(PACKAGE, "nightly", InstallOrigin::Remote)
        .await
        .expect("an unordered release is not a fault");
    let _ignored = server.await;

    assert_eq!(installed, Some((id, unordered_blob)));
    assert_eq!(row(&store, id).bytecode.blob_id(), newer_blob);
}

/// Equal versions need no order, so a rebuilt non-semver release still lands.
#[tokio::test]
async fn a_remote_install_of_the_same_unordered_version_replaces_the_row() {
    let key = SigningKey::generate(&mut UnwrapErr(SysRng));
    let (first, id) = common::signed_bundle_bytes_by(&key, PACKAGE, "nightly", &[]);
    let (rebuilt, _id) = common::signed_bundle_bytes_by(&key, PACKAGE, "nightly", &["svc"]);

    let store = Store::new(Arc::new(InMemoryDB::owned()));
    let (node_client, _data, _blobs) = node_over(&store, &unused_base()).await;
    let _first_blob = install_by_operator(&node_client, &first).await;
    let rebuilt_blob = store_blob(&node_client, &rebuilt).await;

    let _id = node_client
        .install_expected_bundle_blob(
            &id,
            &rebuilt_blob,
            &PENDING_BLOB_SHARE_SOURCE.parse().expect("source"),
        )
        .await
        .expect("install");

    assert_eq!(row(&store, id).bytecode.blob_id(), rebuilt_blob);
}

/// A non-semver row cannot be shown older than anything, so a remote semver
/// release never replaces it.
#[tokio::test]
async fn a_remote_semver_release_never_replaces_an_unordered_row() {
    let (semver_release, unordered, id) = two_releases("0.0.1", "nightly");

    let store = Store::new(Arc::new(InMemoryDB::owned()));
    let (node_client, _data, _blobs) = node_over(&store, &unused_base()).await;
    let unordered_blob = install_by_operator(&node_client, &unordered).await;
    let semver_blob = store_blob(&node_client, &semver_release).await;

    let _id = node_client
        .install_expected_bundle_blob(
            &id,
            &semver_blob,
            &PENDING_BLOB_SHARE_SOURCE.parse().expect("source"),
        )
        .await
        .expect("a kept row is not a fault");

    assert_eq!(row(&store, id).bytecode.blob_id(), unordered_blob);
}

#[tokio::test]
async fn downloaded_raw_wasm_never_replaces_a_raw_row_and_is_released() {
    let held = b"raw wasm already bound".to_vec();
    let raw = b"raw wasm, not a bundle".to_vec();
    let raw_blob = common::blob_id_of(&raw).await;
    let named_id = ApplicationId::from([0xB4; 32]);
    let (url, server) = common::serve_once(raw).await;

    let store = Store::new(Arc::new(InMemoryDB::owned()));
    let (node_client, _data, _blobs) = node_over(&store, &base_of(&url)).await;
    let held_blob = store_blob(&node_client, &held).await;
    let source: ApplicationSource = "file:///home/dev/app.wasm".parse().expect("source");
    node_client
        .write_application_row(&named_id, &held_blob, held.len() as u64, &source)
        .expect("first install");

    let _refused = download(&node_client, &base_of(&url), &req(raw_blob, named_id))
        .await
        .expect_err("raw wasm must not replace a row that holds bytes");
    let _ignored = server.await;

    assert_eq!(row(&store, named_id).bytecode.blob_id(), held_blob);
    assert!(!node_client.has_blob(&raw_blob).expect("blob lookup"));
}

/// Reinstalling a missing intermediate version is how an operator recovers a
/// stranded context, so an admin install may still roll the row back.
#[tokio::test]
async fn an_operator_install_may_roll_the_row_back() {
    let (older, newer, id) = two_releases("1.0.0", "2.0.0");

    let store = Store::new(Arc::new(InMemoryDB::owned()));
    let (node_client, _data, _blobs) = node_over(&store, &unused_base()).await;
    let _newer_blob = install_by_operator(&node_client, &newer).await;
    let older_blob = install_by_operator(&node_client, &older).await;

    let row = row(&store, id);
    assert_eq!(&*row.version, "1.0.0");
    assert_eq!(row.bytecode.blob_id(), older_blob);
}

/// A resync or blob share delivering an older release keeps the row, stores
/// none of its service blobs, and still succeeds.
#[tokio::test]
async fn an_expected_blob_install_keeps_the_newer_row() {
    let key = SigningKey::generate(&mut UnwrapErr(SysRng));
    let (newer, id) = common::signed_bundle_bytes_by(&key, PACKAGE, "2.0.0", &[]);
    let (older, _id) = common::signed_bundle_bytes_by(&key, PACKAGE, "1.0.0", &["svc"]);

    let store = Store::new(Arc::new(InMemoryDB::owned()));
    let (node_client, _data, _blobs) = node_over(&store, &unused_base()).await;
    let newer_blob = install_by_operator(&node_client, &newer).await;
    let older_blob = store_blob(&node_client, &older).await;

    let installed = node_client
        .install_expected_bundle_blob(
            &id,
            &older_blob,
            &PENDING_BLOB_SHARE_SOURCE.parse().expect("source"),
        )
        .await
        .expect("an older release is not a fault");

    assert_eq!(installed, id);
    assert_eq!(row(&store, id).bytecode.blob_id(), newer_blob);
    assert_eq!(
        node_client.list_blobs().expect("list").len(),
        2,
        "a kept row must store no service blob"
    );
}

/// A rebuilt release under an unchanged version still replaces the row.
#[tokio::test]
async fn an_expected_blob_install_accepts_the_same_version() {
    let key = SigningKey::generate(&mut UnwrapErr(SysRng));
    let (first, id) = common::signed_bundle_bytes_by(&key, PACKAGE, VERSION, &[]);
    let (rebuilt, _id) = common::signed_bundle_bytes_by(&key, PACKAGE, VERSION, &["svc"]);

    let store = Store::new(Arc::new(InMemoryDB::owned()));
    let (node_client, _data, _blobs) = node_over(&store, &unused_base()).await;
    let _first_blob = install_by_operator(&node_client, &first).await;
    let rebuilt_blob = store_blob(&node_client, &rebuilt).await;

    let _id = node_client
        .install_expected_bundle_blob(
            &id,
            &rebuilt_blob,
            &PENDING_BLOB_SHARE_SOURCE.parse().expect("source"),
        )
        .await
        .expect("install");

    assert_eq!(row(&store, id).bytecode.blob_id(), rebuilt_blob);
}

/// The stub governance seeds before any bytes arrive holds nothing, and raw
/// wasm still may not fill it.
#[tokio::test]
async fn downloaded_raw_wasm_never_fills_a_placeholder_row() {
    let raw = b"raw wasm, not a bundle".to_vec();
    let raw_blob = common::blob_id_of(&raw).await;
    let named_id = ApplicationId::from([0xB3; 32]);
    let (url, server) = common::serve_once(raw).await;

    let store = Store::new(Arc::new(InMemoryDB::owned()));
    let (node_client, _data, _blobs) = node_over(&store, &base_of(&url)).await;
    store
        .handle()
        .put(
            &key::ApplicationMeta::new(named_id),
            &types::ApplicationMeta::new(
                key::BlobMeta::new(raw_blob),
                0,
                PENDING_BLOB_SHARE_SOURCE.into(),
                Box::default(),
                key::BlobMeta::new(BlobId::from([0; 32])),
                types::PackageInfo {
                    package: PACKAGE.into(),
                    version: VERSION.into(),
                    signer_id: String::new().into_boxed_str(),
                    state_version: 0,
                },
            ),
        )
        .expect("seed stub");

    let _refused = download(&node_client, &base_of(&url), &req(raw_blob, named_id))
        .await
        .expect_err("raw wasm must not fill a stub");
    let _ignored = server.await;

    assert_eq!(row(&store, named_id).size, 0, "the stub must stay a stub");
    assert!(!node_client.has_blob(&raw_blob).expect("blob lookup"));
}
