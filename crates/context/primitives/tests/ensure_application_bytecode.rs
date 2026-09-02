//! `ContextClient::ensure_application_bytecode` - the acquisition a node runs
//! when it holds an application row but not the bytecode it names.
//!
//! The row under test throughout is the one `GroupOp::ContextRegistered` writes
//! on every replica that can decrypt it: the creator's real blob id, no size, no
//! package. A device that follows no context never reaches the context bootstrap
//! that would resolve it, so this is what has to.

use calimero_context_client::client::ContextClient;
use calimero_node_primitives::client::NodeClient;
use calimero_node_primitives::test_fixtures::{bundle, node_client};
use calimero_primitives::application::{Application, ApplicationId};
use calimero_primitives::blobs::BlobId;
use calimero_primitives::context::ContextId;
use calimero_store::{key, types, Store};
use calimero_utils_actix::LazyRecipient;
use futures_util::io::Cursor;
use tempfile::TempDir;

const STUB_SOURCE: &str = "calimero://pending-blob-share";

/// The context whose bytecode is being acquired. Only names the scope the blob
/// is announced under; nothing here is a member of it.
fn context() -> ContextId {
    ContextId::from([0x0C; 32])
}

/// The shared node fixture with a `ContextClient` over the same store.
async fn node() -> (ContextClient, NodeClient, Store, (TempDir, TempDir)) {
    let (node_client, store, data_dir, blob_dir) = node_client().await;
    let context_client =
        ContextClient::new(store.clone(), node_client.clone(), LazyRecipient::new());
    (context_client, node_client, store, (data_dir, blob_dir))
}

/// The row `ContextRegistered`'s apply writes: the creator's real blob, no size,
/// no package. See `governance-store`'s `apply_group_op_inner`.
fn write_stub(store: &Store, application_id: ApplicationId, blob_id: BlobId, source: &str) {
    let blob = key::BlobMeta::new(blob_id);
    store
        .handle()
        .put(
            &key::ApplicationMeta::new(application_id),
            &types::ApplicationMeta::new(
                blob,
                0,
                source.to_owned().into_boxed_str(),
                Vec::new().into_boxed_slice(),
                blob,
                types::PackageInfo {
                    package: String::new().into_boxed_str(),
                    version: String::new().into_boxed_str(),
                    signer_id: String::new().into_boxed_str(),
                    state_version: 0,
                },
            ),
        )
        .unwrap();
}

/// Copy `blob_id` from `from`'s blobstore into `into`'s, the way blob share
/// delivers it.
async fn hand_over_blob(from: &NodeClient, into: &NodeClient, blob_id: BlobId) {
    let bytes = from
        .get_blob_bytes(&blob_id, None)
        .await
        .unwrap()
        .expect("source node holds the blob");
    let (received, _) = into
        .add_blob(Cursor::new(bytes.as_ref()), Some(bytes.len() as u64), None)
        .await
        .unwrap();
    assert_eq!(received, blob_id);
}

/// The creator: a node that installed the bundle, plus the application it got.
async fn creator(dir: &TempDir, wasm: &[u8]) -> (NodeClient, Application, (TempDir, TempDir)) {
    let (_client, node_client, _store, dirs) = node().await;
    let application_id = node_client
        .install_application_from_path(bundle(dir, "com.example.paired", "1.0.0", wasm))
        .await
        .unwrap();
    let application = node_client
        .get_application(&application_id)
        .unwrap()
        .expect("creator installed it");
    (node_client, application, dirs)
}

/// An application that is genuinely installed is left alone. The source is a
/// registry URL nothing can reach, so any attempt to redo the acquisition fails
/// loudly instead of passing by accident.
#[tokio::test]
async fn an_installed_application_is_left_alone() {
    let dir = TempDir::new().unwrap();
    let (creator_node, application, _creator_blobs) = creator(&dir, b"installed wasm").await;
    let (client, node_client, store, _blobs) = node().await;
    hand_over_blob(&creator_node, &node_client, application.blob.bytecode).await;

    let blob = key::BlobMeta::new(application.blob.bytecode);
    store
        .handle()
        .put(
            &key::ApplicationMeta::new(application.id),
            &types::ApplicationMeta::new(
                blob,
                application.size,
                "http://127.0.0.1:1/unreachable.mpk"
                    .to_owned()
                    .into_boxed_str(),
                application.metadata.clone().into_boxed_slice(),
                blob,
                types::PackageInfo {
                    package: "com.example.paired".into(),
                    version: "1.0.0".into(),
                    signer_id: "signer".into(),
                    state_version: 0,
                },
            ),
        )
        .unwrap();

    assert!(client
        .ensure_application_bytecode(application.id, context())
        .await
        .expect("an installed row is answered without reaching for the source"));
}

/// Nothing names the application here yet, which is the ordinary state before
/// the `ContextRegistered` op applies.
#[tokio::test]
async fn an_unknown_application_is_not_yet() {
    let (client, _node_client, _store, _blobs) = node().await;

    assert!(!client
        .ensure_application_bytecode(ApplicationId::from([0x11; 32]), context())
        .await
        .unwrap());
}

/// The context bootstrap's own stub names no bytecode at all, so there is
/// nothing to fetch. The pass declines rather than failing, so a namespace
/// nobody can answer for cannot poison whatever drives it.
#[tokio::test]
async fn a_stub_naming_no_bytecode_is_not_yet_rather_than_an_error() {
    let (client, _node_client, store, _blobs) = node().await;
    let application_id = ApplicationId::from([0x22; 32]);
    write_stub(&store, application_id, BlobId::from([0u8; 32]), STUB_SOURCE);

    assert!(!client
        .ensure_application_bytecode(application_id, context())
        .await
        .expect("a row with nothing to fetch is not an error"));
}

/// A raw wasm blob's application id is derived from the bytes, size, source and
/// metadata it was installed with, none of which a stub row carries - so it is
/// declined rather than installed under an id that names nothing.
#[tokio::test]
async fn a_non_bundle_blob_is_refused_rather_than_installed() {
    let (client, node_client, store, _blobs) = node().await;
    let application_id = ApplicationId::from([0x33; 32]);
    let (blob_id, _) = node_client
        .add_blob(Cursor::new(&b"raw wasm, no manifest"[..]), None, None)
        .await
        .unwrap();
    write_stub(&store, application_id, blob_id, STUB_SOURCE);

    assert!(!client
        .ensure_application_bytecode(application_id, context())
        .await
        .unwrap());
    assert_eq!(
        node_client
            .get_application(&application_id)
            .unwrap()
            .unwrap()
            .size,
        0,
        "the stub is left as it was"
    );
}

/// The centrepiece: a stub plus the bytes must end at the row a joiner ends at.
/// Mirrors `test_bundle_blob_sharing_integration` in
/// `calimero-node-primitives`, field for field.
#[tokio::test]
async fn a_stub_becomes_the_row_a_joiner_ends_up_with() {
    let dir = TempDir::new().unwrap();
    let wasm = b"paired device wasm bytecode";
    let (creator_node, creator_app, _creator_blobs) = creator(&dir, wasm).await;

    let (client, node_client, store, _blobs) = node().await;
    hand_over_blob(&creator_node, &node_client, creator_app.blob.bytecode).await;
    write_stub(
        &store,
        creator_app.id,
        creator_app.blob.bytecode,
        &creator_app.source.to_string(),
    );
    assert!(
        node_client
            .get_application(&creator_app.id)
            .unwrap()
            .unwrap()
            .size
            == 0,
        "starts as a stub"
    );

    assert!(client
        .ensure_application_bytecode(creator_app.id, context())
        .await
        .unwrap());

    let installed = node_client
        .get_application(&creator_app.id)
        .unwrap()
        .expect("installed");
    assert_eq!(installed.blob.bytecode, creator_app.blob.bytecode);
    assert_eq!(installed.size, creator_app.size);
    assert_eq!(installed.source.to_string(), creator_app.source.to_string());
    assert_eq!(installed.metadata, creator_app.metadata);
    assert!(
        !installed.metadata.is_empty(),
        "metadata comes off the manifest"
    );

    assert_eq!(
        node_client
            .get_application_bytes(&creator_app.id, None)
            .await
            .unwrap()
            .unwrap()
            .as_ref(),
        wasm
    );
}

/// A pass that could not acquire leaves nothing behind that stops the next one:
/// the same row, once it names bytecode this node can reach, installs on the
/// second call rather than staying a stub for good.
#[tokio::test]
async fn a_second_pass_installs_what_the_first_could_not_reach() {
    let dir = TempDir::new().unwrap();
    let wasm = b"retried wasm bytecode";
    let (creator_node, creator_app, _creator_blobs) = creator(&dir, wasm).await;

    let (client, node_client, store, _blobs) = node().await;
    write_stub(
        &store,
        creator_app.id,
        BlobId::from([0u8; 32]),
        &creator_app.source.to_string(),
    );

    assert!(
        !client
            .ensure_application_bytecode(creator_app.id, context())
            .await
            .unwrap(),
        "the first pass has no bytecode to fetch"
    );

    // What the `ContextRegistered` apply and a blob share between them supply.
    hand_over_blob(&creator_node, &node_client, creator_app.blob.bytecode).await;
    write_stub(
        &store,
        creator_app.id,
        creator_app.blob.bytecode,
        &creator_app.source.to_string(),
    );

    assert!(client
        .ensure_application_bytecode(creator_app.id, context())
        .await
        .unwrap());
    let installed = node_client
        .get_application(&creator_app.id)
        .unwrap()
        .unwrap();
    assert_eq!(installed.size, creator_app.size);
    assert_eq!(installed.metadata, creator_app.metadata);
}
