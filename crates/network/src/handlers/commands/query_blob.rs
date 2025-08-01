use actix::{Context, Handler, Message, ResponseFuture};
use calimero_network_primitives::messages::QueryBlob;
use eyre::eyre;
use libp2p::kad::RecordKey;
use tokio::sync::oneshot;
use tracing::info;

use crate::NetworkManager;

impl Handler<QueryBlob> for NetworkManager {
    type Result = ResponseFuture<<QueryBlob as Message>::Result>;

    fn handle(&mut self, request: QueryBlob, _ctx: &mut Context<Self>) -> Self::Result {
        info!(
            blob_id = %request.blob_id,
            context_id = ?request.context_id.as_ref().map(|id| id.to_string()),
            "Querying DHT for blob"
        );

        let (sender, receiver) = oneshot::channel();

        // Create search key based on context
        let key = if let Some(context_id) = request.context_id {
            // Search in specific context
            let mut key_bytes = Vec::with_capacity(64);
            key_bytes.extend_from_slice(&*context_id);
            key_bytes.extend_from_slice(&*request.blob_id);

            info!(
                "QUERY: blob_id={}, context_id={}, key_len={}",
                request.blob_id,
                context_id,
                key_bytes.len(),
            );

            RecordKey::new(&key_bytes)
        } else {
            // Global search would require searching all known contexts
            // For MVP, we'll return an error for global queries
            drop(sender.send(Err(eyre!("Global blob queries not yet supported"))));
            return Box::pin(async { receiver.await.expect("Sender not to be dropped") });
        };

        // Start the query (get_record returns QueryId directly)
        let query_id = self.swarm.behaviour_mut().kad.get_record(key);

        // Store the query for completion handling
        let _previous = self.pending_blob_queries.insert(query_id, sender);

        Box::pin(async { receiver.await.expect("Sender not to be dropped") })
    }
}
