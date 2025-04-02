use calimero_blobstore::BlobManager;
use calimero_network_primitives::client::NetworkClient;
use calimero_primitives::context::ContextId;
use calimero_store::Store;
use libp2p::gossipsub::IdentTopic;
use tracing::info;

mod alias;
mod application;
mod blob;

#[derive(Clone, Debug)]
pub struct NodeClient {
    datastore: Store,
    blobstore: BlobManager,
    network_manager: NetworkClient,
}

impl NodeClient {
    pub async fn subscribe(&self, context_id: &ContextId) -> eyre::Result<()> {
        let topic = IdentTopic::new(context_id);

        let _ignored = self.network_manager.subscribe(topic).await?;

        info!(%context_id, "Subscribed to context");

        Ok(())
    }

    pub async fn unsubscribe(&self, context_id: &ContextId) -> eyre::Result<()> {
        let topic = IdentTopic::new(context_id);

        let _ignored = self.network_manager.unsubscribe(topic).await?;

        info!(%context_id, "Unsubscribed from context");

        Ok(())
    }

    pub async fn get_peers_count(&self, context: Option<ContextId>) -> eyre::Result<usize> {
        let Some(context) = context else {
            let peers = self.network_manager.peer_count().await;

            return Ok(peers);
        };

        let topic = IdentTopic::new(context);

        let peers = self.network_manager.mesh_peer_count(topic.hash()).await;

        Ok(peers)
    }

    // // on node, not client
    // pub async fn get_sender_key() {}
    // pub async fn update_sender_key() {}
    // pub async fn get_private_key() {}
    // approve & propose
    // // on node, not client

    // pub async fn execute(
    //     &self,
    //     context_id: ContextId,
    //     method: String,
    //     payload: Vec<u8>,
    //     executor_public_key: PublicKey,
    // ) -> Result<Outcome, ExecutionError> {
    //     let (tx, rx) = oneshot::channel();

    //     self.node_manager
    //         .send(NodeMessage::Execute {
    //             request: ExecuteRequest {
    //                 context_id,
    //                 method,
    //                 payload,
    //                 executor_public_key,
    //             },
    //             outcome: tx,
    //         })
    //         .await
    //         .expect("Mailbox to not be dropped");

    //     rx.await.expect("Mailbox to not be dropped")
    // }
}
