use calimero_network_primitives::messages::NetworkEvent;
use calimero_primitives::blobs::BlobId;
use calimero_primitives::context::ContextId;
use libp2p::kad::{Event, GetRecordError, GetRecordOk, QueryResult};
use libp2p::PeerId;
use libp2p_metrics::Recorder;
use owo_colors::OwoColorize;
use tracing::{debug, info};

use super::{EventHandler, NetworkManager};

impl EventHandler<Event> for NetworkManager {
    fn handle(&mut self, event: Event) {
        self.metrics.record(&event);
        debug!("{}: {:?}", "kad".yellow(), event);

        match event {
            Event::OutboundQueryProgressed {
                id,
                result: QueryResult::Bootstrap(result),
                ..
            } => {
                if let Some(sender) = self.pending_bootstrap.remove(&id) {
                    let _ignored = sender.send(result.map(|_| ()).map_err(Into::into));
                }
            }
            Event::OutboundQueryProgressed {
                id,
                result: QueryResult::GetRecord(result),
                ..
            } => {
                // Handle blob query results
                info!("DHT GetRecord result for query_id={:?}: {:?}", id, result);
                info!(
                    "Found {} pending blob queries, looking for query_id={:?}",
                    self.pending_blob_queries.len(),
                    id
                );
                if let Some(sender) = self.pending_blob_queries.remove(&id) {
                    let peers = match &result {
                        Ok(GetRecordOk::FoundRecord(record)) => {
                            debug!(
                                "DHT query found record with key length {} and value length {}",
                                record.record.key.as_ref().len(),
                                record.record.value.len()
                            );

                            // Extract peer IDs from record values
                            let mut peers = Vec::new();
                            if record.record.value.len() >= 8 {
                                // Need at least 8 bytes for size
                                // The value format is: peer_id_bytes (variable length) + size (8 bytes)
                                // Extract size from the last 8 bytes
                                let size_start = record.record.value.len() - 8;
                                if let Ok(peer_id) =
                                    PeerId::from_bytes(&record.record.value[..size_start])
                                {
                                    peers.push(peer_id);
                                    info!("Extracted peer_id {} from DHT record", peer_id);
                                } else {
                                    info!("Failed to parse peer_id from DHT record value");
                                }
                            }
                            info!("Found {} peers with blob", peers.len());

                            // Extract blob_id and context_id from record key
                            if record.record.key.as_ref().len() >= 64 {
                                let context_id_bytes: [u8; 32] = record.record.key.as_ref()[..32]
                                    .try_into()
                                    .unwrap_or_default();
                                let blob_id_bytes: [u8; 32] = record.record.key.as_ref()[32..64]
                                    .try_into()
                                    .unwrap_or_default();

                                let blob_id = BlobId::from(blob_id_bytes);
                                let context_id = ContextId::from(context_id_bytes);

                                // Emit network event
                                self.event_recipient
                                    .do_send(NetworkEvent::BlobProvidersFound {
                                        blob_id,
                                        context_id: Some(context_id),
                                        providers: peers.clone(),
                                    });
                            }

                            Ok(peers)
                        }
                        Ok(GetRecordOk::FinishedWithNoAdditionalRecord { .. }) => {
                            debug!("Blob query completed with no additional records");
                            Ok(Vec::new())
                        }
                        Err(e) => {
                            info!("DHT query failed with error: {:?}", e);
                            if let GetRecordError::NotFound { key, closest_peers } = e {
                                info!(
                                    "DHT query NotFound - key_hex={}, closest_peers={:?}",
                                    hex::encode(key.as_ref()),
                                    closest_peers
                                );
                            }
                            Err(eyre::eyre!("Blob query failed: {:?}", e))
                        }
                    };
                    let _ignored = sender.send(peers);
                }
            }
            Event::InboundRequest { .. }
            | Event::OutboundQueryProgressed { .. }
            | Event::ModeChanged { .. }
            | Event::PendingRoutablePeer { .. }
            | Event::RoutablePeer { .. }
            | Event::RoutingUpdated { .. }
            | Event::UnroutablePeer { .. } => {}
        }
    }
}
