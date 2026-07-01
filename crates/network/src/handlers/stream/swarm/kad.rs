use calimero_network_primitives::messages::NetworkEvent;
use calimero_primitives::blobs::BlobId;
use calimero_primitives::context::ContextId;
use libp2p::kad::store::RecordStore;
use libp2p::kad::{Event, GetRecordError, GetRecordOk, InboundRequest, QueryResult, Record};
use libp2p::PeerId;
use libp2p_metrics::Recorder;
use owo_colors::OwoColorize;
use tracing::debug;

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
                debug!("DHT GetRecord result for query_id={:?}: {:?}", id, result);
                debug!(
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
                                    debug!("Extracted peer_id {} from DHT record", peer_id);
                                } else {
                                    debug!("Failed to parse peer_id from DHT record value");
                                }
                            }
                            debug!("Found {} peers with blob", peers.len());

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
                                let _ignored = self.event_dispatcher.dispatch(
                                    NetworkEvent::BlobProvidersFound {
                                        blob_id,
                                        context_id: Some(context_id),
                                        providers: peers.clone(),
                                    },
                                );
                            }

                            Ok(peers)
                        }
                        Ok(GetRecordOk::FinishedWithNoAdditionalRecord { .. }) => {
                            debug!("Blob query completed with no additional records");
                            Ok(Vec::new())
                        }
                        Err(e) => {
                            debug!("DHT query failed with error: {:?}", e);
                            if let GetRecordError::NotFound { key, closest_peers } = e {
                                debug!(
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
            // Record filtering is enabled (`StoreInserts::FilterBoth`), so a
            // record replicated to us by another peer surfaces here as an
            // inbound request rather than being written to our store blind.
            // Validate its shape before admitting it; anything else is a
            // routing/read request with nothing to store.
            Event::InboundRequest { request } => match request {
                InboundRequest::PutRecord {
                    source,
                    record: Some(record),
                    ..
                } => {
                    if is_valid_blob_provider_record(&record) {
                        match self.swarm.behaviour_mut().kad.store_mut().put(record) {
                            Ok(()) => {
                                debug!(%source, "stored validated inbound DHT record");
                            }
                            Err(err) => {
                                debug!(%source, ?err, "rejected inbound DHT record: store put failed");
                            }
                        }
                    } else {
                        debug!(%source, "rejected malformed inbound DHT record");
                    }
                }
                // We announce blobs via `put_record`, never `start_providing`,
                // so this node holds no provider records — drop any a peer
                // tries to add rather than storing on its behalf.
                InboundRequest::AddProvider { .. } => {
                    debug!("ignoring inbound AddProvider record (provider records unused)");
                }
                InboundRequest::FindNode { .. }
                | InboundRequest::GetProvider { .. }
                | InboundRequest::GetRecord { .. }
                | InboundRequest::PutRecord { record: None, .. } => {}
            },
            Event::OutboundQueryProgressed { .. }
            | Event::ModeChanged { .. }
            | Event::PendingRoutablePeer { .. }
            | Event::RoutablePeer { .. }
            | Event::RoutingUpdated { .. }
            | Event::UnroutablePeer { .. } => {}
        }
    }
}

/// Structural validation for an inbound blob-provider record before we admit
/// it to our store.
///
/// With record filtering enabled every record another peer replicates to us
/// arrives for inspection instead of being written blind. We can't authorize
/// the *content* — these records are unsigned and the network layer has no
/// view of context membership — but we can reject anything not shaped like the
/// blob announcement this node itself produces (see the `AnnounceBlob`
/// handler): a 64-byte key (context id + blob id) whose value is a parseable
/// peer id followed by an 8-byte little-endian size. We also enforce the
/// store's `KAD_MAX_VALUE_BYTES` ceiling here, so an oversized value is dropped
/// in our own code rather than relying on the store's `put` as the only guard.
/// That drops malformed, oversized, and garbage records cheaply; the store's
/// own `max_value_bytes` / `max_records` bounds are the backstop.
fn is_valid_blob_provider_record(record: &Record) -> bool {
    /// context_id (32) + blob_id (32).
    const EXPECTED_KEY_LEN: usize = 64;
    /// Trailing little-endian `u64` blob size.
    const SIZE_LEN: usize = 8;

    if record.key.as_ref().len() != EXPECTED_KEY_LEN {
        return false;
    }

    // Reject anything larger than the store would accept, up front — a valid
    // blob-provider value is ~50 bytes, far below this ceiling.
    if record.value.len() > crate::behaviour::KAD_MAX_VALUE_BYTES {
        return false;
    }

    let Some(peer_id_bytes) = record
        .value
        .len()
        .checked_sub(SIZE_LEN)
        .map(|end| &record.value[..end])
    else {
        return false;
    };

    // A zero-length peer id can never be valid; `from_bytes` also rejects it,
    // but bail explicitly so intent is clear.
    !peer_id_bytes.is_empty() && PeerId::from_bytes(peer_id_bytes).is_ok()
}

#[cfg(test)]
mod tests {
    use libp2p::kad::RecordKey;

    use super::*;

    fn record(key: Vec<u8>, value: Vec<u8>) -> Record {
        Record::new(RecordKey::new(&key), value)
    }

    fn valid_value() -> Vec<u8> {
        let peer_id = PeerId::random();
        let size: u64 = 4096;
        [peer_id.to_bytes().as_slice(), &size.to_le_bytes()].concat()
    }

    #[test]
    fn accepts_a_well_formed_blob_record() {
        let rec = record(vec![7u8; 64], valid_value());
        assert!(is_valid_blob_provider_record(&rec));
    }

    #[test]
    fn rejects_wrong_key_length() {
        // One byte short and one byte long — only exactly 64 is a blob key.
        assert!(!is_valid_blob_provider_record(&record(
            vec![7u8; 63],
            valid_value()
        )));
        assert!(!is_valid_blob_provider_record(&record(
            vec![7u8; 65],
            valid_value()
        )));
    }

    #[test]
    fn rejects_value_too_short_for_a_size_suffix() {
        // At most 8 bytes (<= SIZE_LEN) leaves no room for a peer id: below 8
        // the `checked_sub` fails, and exactly 8 yields an empty prefix caught
        // by the `is_empty` guard.
        assert!(!is_valid_blob_provider_record(&record(
            vec![7u8; 64],
            vec![0u8; 4]
        )));
        assert!(!is_valid_blob_provider_record(&record(
            vec![7u8; 64],
            vec![0u8; 8]
        )));
        // One byte past the size suffix is a non-empty prefix, so it clears the
        // `is_empty` guard and must instead be rejected by the peer-id parse.
        assert!(!is_valid_blob_provider_record(&record(
            vec![7u8; 64],
            vec![0u8; 9]
        )));
    }

    #[test]
    fn rejects_unparseable_peer_id() {
        // Right shape (>8 bytes) but the leading bytes aren't a valid peer id.
        let value = [vec![0xffu8; 16], 1024u64.to_le_bytes().to_vec()].concat();
        assert!(!is_valid_blob_provider_record(&record(
            vec![7u8; 64],
            value
        )));
    }

    #[test]
    fn rejects_value_over_the_store_ceiling() {
        // A parseable peer id + size suffix, but padded past the store's
        // `max_value_bytes`: the validator drops it up front rather than
        // leaving the store's `put` as the only guard.
        let peer_id = PeerId::random();
        let padding = vec![0u8; crate::behaviour::KAD_MAX_VALUE_BYTES];
        let value = [
            peer_id.to_bytes().as_slice(),
            &padding,
            &4096u64.to_le_bytes(),
        ]
        .concat();
        assert!(value.len() > crate::behaviour::KAD_MAX_VALUE_BYTES);
        assert!(!is_valid_blob_provider_record(&record(
            vec![7u8; 64],
            value
        )));
    }
}
