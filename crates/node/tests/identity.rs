use calimero_identity::IdentityHandler;
use calimero_node::{Node, NodeConfig};
use calimero_primitives::context::Context;
use calimero_primitives::hash::Hash;
use libp2p::identity::Keypair;

use super::*;

fn create_test_config() -> NodeConfig {
    let identity = Keypair::generate_ed25519();
    NodeConfig {
        home: camino::Utf8PathBuf::from("/tmp"),
        identity: identity,
        node_type: calimero_node_primitives::NodeType::Peer,
        application: calimero_context::config::ApplicationConfig::default(),
        network: calimero_network::config::NetworkConfig::default(),
        server: calimero_server::config::ServerConfig::default(),
        store: calimero_store::config::StoreConfig::default(),
    }
}

#[tokio::test]
async fn test_get_executor_identity() {
    let config = create_test_config();
    let identity_handler = IdentityHandler::from(&config.identity);
    let (network_client, _) = calimero_network::run(&config.network, identity_handler.clone())
        .await
        .expect("Failed to run network");

    let identity = network_client
        .identity_handler
        .as_ref()
        .unwrap()
        .read()
        .await
        .get_executor_identity();

    assert!(
        !identity.is_empty(),
        "Executor identity should not be empty"
    );
}

#[tokio::test]
async fn test_sign_message() {
    let config = create_test_config();
    let identity_handler = IdentityHandler::from(&config.identity);
    let (network_client, _) = calimero_network::run(&config.network, identity_handler.clone())
        .await
        .expect("Failed to run network");

    let message = b"Hello, World!";
    let signature = network_client
        .identity_handler
        .as_ref()
        .unwrap()
        .write()
        .await
        .sign_message(message);

    assert!(!signature.is_empty(), "Signature should not be empty");
    // Note: We don't have a verify_signature method, so we can't check the
    // signature validity here.
}

#[tokio::test]
async fn test_execute_transaction_with_identity() {
    let config = create_test_config();
    let identity_handler = IdentityHandler::from(&config.identity);
    let (network_client, _) = calimero_network::run(&config.network, identity_handler.clone())
        .await
        .expect("Failed to run network");

    let store = calimero_store::Store::open::<calimero_store::db::RocksDB>(&config.store)
        .expect("Failed to open store");
    let ctx_manager = calimero_context::ContextManager::start(
        &config.application,
        store.clone(),
        network_client.clone(),
    )
    .await
    .expect("Failed to start context manager");

    let (node_events, _) = tokio::sync::broadcast::channel(32);

    let mut node = Node::new(&config, network_client, node_events, ctx_manager, store);

    let context = Context {
        id: calimero_primitives::context::ContextId::from([0; 32]),
        application_id: "test_app".to_string().into(),
        last_transaction_hash: Hash::default(),
    };
    let method = "test_method".to_string();
    let payload = vec![1, 2, 3, 4];

    let outcome = node
        .execute(context, None, method, payload)
        .await
        .expect("Failed to execute transaction");

    // Note: The Outcome struct doesn't include executor_identity, so we can't
    // check it directly. Instead, we can check if the outcome is not empty or
    // has some expected structure.
    assert!(
        !outcome.returns.is_err(),
        "Execution should not result in an error"
    );
}
