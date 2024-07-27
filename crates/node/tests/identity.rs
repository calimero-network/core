use calimero_node::{Node, NodeConfig};
use calimero_primitives::context::ContextId;
use camino::Utf8PathBuf;
use libp2p::identity::Keypair;
use tokio::sync::{broadcast, oneshot};
use {calimero_context, calimero_network, calimero_server, calimero_store};

fn create_test_config() -> NodeConfig {
    let identity = Keypair::generate_ed25519();
    NodeConfig {
        home: Utf8PathBuf::from("/tmp"),
        identity: identity.clone(),
        node_type: calimero_node_primitives::NodeType::Peer,
        application: calimero_context::config::ApplicationConfig {
            dir: Utf8PathBuf::from("/tmp/app"),
        },
        network: calimero_network::config::NetworkConfig {
            identity: identity.clone(),
            node_type: calimero_node_primitives::NodeType::Peer,
            swarm: calimero_network::config::SwarmConfig { listen: vec![] },
            bootstrap: Default::default(),
            discovery: Default::default(),
            catchup: calimero_network::config::CatchupConfig {
                batch_size: 10,
                receive_timeout: std::time::Duration::from_secs(5),
            },
        },
        server: calimero_server::config::ServerConfig {
            listen: vec![],
            identity: identity.clone(),
            admin: Default::default(),
            jsonrpc: Default::default(),
            websocket: Default::default(),
        },
        store: calimero_store::config::StoreConfig {
            path: Utf8PathBuf::from("/tmp/store"),
        },
    }
}

#[tokio::test]
async fn test_handle_call() {
    let config = create_test_config();
    let (network_client, _) = calimero_network::run(&config.network)
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

    let (node_events, _) = broadcast::channel(32);

    let mut node = Node::new(&config, network_client, node_events, ctx_manager, store);

    let context_id = ContextId::from([0; 32]);
    let public_key = [1u8; 32];

    // Note: In a real scenario, you'd need to set up the context before this call.
    // This might involve calling other Node methods or setting up the context manually.

    let method = "test_method".to_string();
    let payload = vec![1, 2, 3, 4];
    let write = true; // Assuming this is a write operation

    let (outcome_sender, outcome_receiver) = oneshot::channel();

    // Call handle_call method
    node.handle_call(
        context_id,
        method,
        payload,
        write,
        public_key,
        outcome_sender,
    )
    .await;

    // Wait for the outcome
    let outcome = outcome_receiver.await.expect("Failed to receive outcome");

    match outcome {
        Ok(result) => {
            assert!(
                !result.returns.is_err(),
                "Execution should not result in an error"
            );
        }
        Err(e) => panic!("handle_call failed: {:?}", e),
    }
}
