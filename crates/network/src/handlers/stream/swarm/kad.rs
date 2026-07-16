use calimero_network_primitives::messages::NetworkEvent;
use calimero_primitives::blobs::BlobId;
use calimero_primitives::context::ContextId;
use libp2p::kad::store::RecordStore;
use libp2p::kad::{Event, GetRecordError, GetRecordOk, InboundRequest, QueryResult, Record};
use libp2p_metrics::Recorder;
use owo_colors::OwoColorize;
use tracing::{debug, warn};

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

                            // Authenticate the provider record before trusting
                            // the peer id it names. An unsigned/forged record
                            // could otherwise point us at an arbitrary peer
                            // (misdirection / eclipse); `verify` binds the
                            // record to the peer that signed it.
                            let mut peers = Vec::new();
                            match crate::blob_provider_record::BlobProviderRecord::verify(
                                record.record.key.as_ref(),
                                &record.record.value,
                            ) {
                                Some(peer_id) => {
                                    peers.push(peer_id);
                                    debug!("Verified provider peer_id {} from DHT record", peer_id);
                                }
                                None => {
                                    debug!("Dropping DHT record: provider signature invalid or malformed");
                                }
                            }
                            debug!("Found {} verified peers with blob", peers.len());

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
                                // Surfaced at warn, not debug: the validator
                                // already rejects oversized values, so a put
                                // failure here is most likely the store hitting
                                // its `max_records` ceiling — a capacity signal
                                // an operator should see without debug logging.
                                warn!(%source, ?err, "rejected inbound DHT record: store put failed (store may be full)");
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
                // FilterBoth always attaches the record, so a PutRecord with no
                // payload is an unexpected protocol state (or config drift) —
                // log it so it's distinguishable from a routine routing event.
                InboundRequest::PutRecord {
                    source,
                    record: None,
                    ..
                } => {
                    debug!(%source, "received PutRecord with no record payload — ignoring");
                }
                InboundRequest::FindNode { .. }
                | InboundRequest::GetProvider { .. }
                | InboundRequest::GetRecord { .. } => {}
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

    if record.key.as_ref().len() != EXPECTED_KEY_LEN {
        return false;
    }

    // Reject anything larger than the store would accept, up front.
    if record.value.len() > crate::behaviour::KAD_MAX_VALUE_BYTES {
        return false;
    }

    // Authenticate the record: valid signature + the embedded key hashes to the
    // named peer. This is applied to records another peer asks us to replicate,
    // so we never store (and later re-serve) a forged provider announcement.
    crate::blob_provider_record::BlobProviderRecord::verify(record.key.as_ref(), &record.value)
        .is_some()
}

#[cfg(test)]
mod tests {
    use libp2p::identity::Keypair;
    use libp2p::kad::RecordKey;

    use super::*;
    use crate::blob_provider_record::BlobProviderRecord;

    fn record(key: Vec<u8>, value: Vec<u8>) -> Record {
        Record::new(RecordKey::new(&key), value)
    }

    /// A signed value for a 64-byte key of all `key_byte`s.
    fn signed_value(key_byte: u8) -> (Vec<u8>, Vec<u8>) {
        let key = vec![key_byte; 64];
        let kp = Keypair::generate_ed25519();
        let value = BlobProviderRecord::signed_value(&key, &kp, 4096).expect("sign");
        (key, value)
    }

    #[test]
    fn accepts_a_well_formed_signed_record() {
        let (key, value) = signed_value(7);
        assert!(is_valid_blob_provider_record(&record(key, value)));
    }

    #[test]
    fn rejects_wrong_key_length() {
        // Exactly 64 is the only accepted blob-key length; the validator bails
        // on length before even attempting to verify.
        let (_key, value) = signed_value(7);
        assert!(!is_valid_blob_provider_record(&record(
            vec![7u8; 63],
            value.clone()
        )));
        assert!(!is_valid_blob_provider_record(&record(
            vec![7u8; 65],
            value
        )));
    }

    #[test]
    fn rejects_record_signed_for_a_different_key() {
        // A record legitimately signed for key A must not validate under key B —
        // `verify` binds the signature to the DHT key.
        let (_key_a, value) = signed_value(7);
        assert!(!is_valid_blob_provider_record(&record(
            vec![9u8; 64],
            value
        )));
    }

    #[test]
    fn rejects_unsigned_or_garbage_value() {
        assert!(!is_valid_blob_provider_record(&record(
            vec![7u8; 64],
            b"legacy-unsigned-or-garbage".to_vec()
        )));
    }

    #[test]
    fn rejects_value_over_the_store_ceiling() {
        // Padded past the store's `max_value_bytes`: dropped up front rather
        // than leaving the store's `put` as the only guard.
        let (key, mut value) = signed_value(7);
        value.extend(std::iter::repeat_n(
            0u8,
            crate::behaviour::KAD_MAX_VALUE_BYTES,
        ));
        assert!(value.len() > crate::behaviour::KAD_MAX_VALUE_BYTES);
        assert!(!is_valid_blob_provider_record(&record(key, value)));
    }
}
