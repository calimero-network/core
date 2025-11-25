//! Tests for context application ID management
//!
//! Tests the new functionality for updating and persisting ApplicationId changes,
//! including the deferred installation handling and ApplicationId persistence.

use std::sync::Arc;

use calimero_blobstore::config::BlobStoreConfig;
use calimero_blobstore::{BlobManager, FileSystem};
use calimero_context_config::client::{
    config::{ClientConfig, ClientRelayerSigner, ClientSigner, LocalConfig},
    AnyTransport, Client as ExternalClient,
};
use calimero_network_primitives::client::NetworkClient;
use calimero_node_primitives::client::NodeClient;
use calimero_node_primitives::messages::NodeMessage;
use calimero_primitives::application::ApplicationId;
use calimero_primitives::context::ContextId;
use calimero_primitives::hash::Hash;
use calimero_store::db::InMemoryDB;
use calimero_store::key;
use calimero_store::types::ContextMeta;
use calimero_store::Store;
use calimero_utils_actix::LazyRecipient;
use std::collections::BTreeMap;
use tempfile::TempDir;
use tokio::sync::{broadcast, mpsc};

use calimero_context_primitives::client::ContextClient;

/// Setup a test ContextClient with in-memory storage
async fn setup_test_context_client() -> (ContextClient, TempDir) {
    let tmp_dir = tempfile::tempdir().unwrap();

    // 1. Create in-memory datastore
    let db = InMemoryDB::owned();
    let store = Store::new(Arc::new(db));

    // 2. Setup BlobManager
    let blob_store_config = BlobStoreConfig::new(tmp_dir.path().to_path_buf().try_into().unwrap());
    let file_system = FileSystem::new(&blob_store_config).await.unwrap();
    let blob_manager = BlobManager::new(store.clone(), file_system);

    // 3. Setup network and actor dependencies
    let network_client = NetworkClient::new(LazyRecipient::new());
    let (event_sender, _) = broadcast::channel(16);
    let (ctx_sync_tx, _) = mpsc::channel(16);
    let node_manager = LazyRecipient::<NodeMessage>::new();

    // 4. Construct NodeClient
    let node_client = NodeClient::new(
        store.clone(),
        blob_manager,
        network_client,
        node_manager,
        event_sender,
        ctx_sync_tx,
    );

    // 5. Setup ExternalClient
    let client_config = ClientConfig {
        params: BTreeMap::new(),
        signer: ClientSigner {
            relayer: ClientRelayerSigner {
                url: "http://127.0.0.1:3030".parse().unwrap(),
            },
            local: LocalConfig {
                protocols: BTreeMap::new(),
            },
        },
    };
    let external_client = ExternalClient::from_config(&client_config);

    // 6. Construct ContextClient
    let context_manager = LazyRecipient::new();
    let context_client = ContextClient::new(store, node_client, external_client, context_manager);

    (context_client, tmp_dir)
}

/// Create a test context in the database
fn create_test_context(
    context_client: &ContextClient,
    context_id: ContextId,
    application_id: ApplicationId,
) -> eyre::Result<()> {
    let mut handle = context_client.datastore_handle();
    let key = key::ContextMeta::new(context_id);

    let meta = ContextMeta::new(
        key::ApplicationMeta::new(application_id),
        *Hash::default(),
        vec![],
    );

    handle.put(&key, &meta)?;
    Ok(())
}

#[tokio::test]
async fn test_update_context_application_id_success() {
    let (context_client, _tmp_dir) = setup_test_context_client().await;

    // Create a test context
    let context_id = ContextId::from([1; 32]);
    let old_app_id = ApplicationId::from([10; 32]);
    let new_app_id = ApplicationId::from([20; 32]);

    create_test_context(&context_client, context_id, old_app_id).unwrap();

    // Verify initial state
    let handle = context_client.datastore_handle();
    let key = key::ContextMeta::new(context_id);
    let meta = handle.get(&key).unwrap().unwrap();
    assert_eq!(meta.application.application_id(), old_app_id);

    // Update ApplicationId
    context_client
        .update_context_application_id(&context_id, new_app_id)
        .unwrap();

    // Verify update persisted
    let meta = handle.get(&key).unwrap().unwrap();
    assert_eq!(meta.application.application_id(), new_app_id);
}

#[tokio::test]
async fn test_update_context_application_id_no_change() {
    let (context_client, _tmp_dir) = setup_test_context_client().await;

    // Create a test context
    let context_id = ContextId::from([1; 32]);
    let app_id = ApplicationId::from([10; 32]);

    create_test_context(&context_client, context_id, app_id).unwrap();

    // Update with same ApplicationId (should be a no-op)
    context_client
        .update_context_application_id(&context_id, app_id)
        .unwrap();

    // Verify state unchanged
    let handle = context_client.datastore_handle();
    let key = key::ContextMeta::new(context_id);
    let meta = handle.get(&key).unwrap().unwrap();
    assert_eq!(meta.application.application_id(), app_id);
}

#[tokio::test]
async fn test_update_context_application_id_not_found() {
    let (context_client, _tmp_dir) = setup_test_context_client().await;

    // Try to update non-existent context
    let context_id = ContextId::from([1; 32]);
    let app_id = ApplicationId::from([10; 32]);

    let result = context_client.update_context_application_id(&context_id, app_id);
    assert!(result.is_err());
    assert!(result
        .unwrap_err()
        .to_string()
        .contains("Context not found"));
}
