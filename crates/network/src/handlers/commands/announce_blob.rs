use actix::{Context, Handler, Message, Response};
use calimero_network_primitives::messages::AnnounceBlob;
use eyre::eyre;
use libp2p::kad::{Quorum, Record, RecordKey};
use tracing::{debug, warn};

use crate::NetworkManager;

impl Handler<AnnounceBlob> for NetworkManager {
    type Result = Response<<AnnounceBlob as Message>::Result>;

    fn handle(&mut self, request: AnnounceBlob, _ctx: &mut Context<Self>) -> Self::Result {
        debug!(
            blob_id = %request.blob_id,
            context_id = %request.context_id,
            size = request.size,
            "Announcing blob to DHT"
        );

        // Create a unique key for this blob in this context
        let key =
            RecordKey::new(&[request.context_id.as_slice(), request.blob_id.as_slice()].concat());

        debug!(
            "ANNOUNCE: blob_id={}, context_id={}, key_len={}",
            request.blob_id,
            request.context_id,
            key.as_ref().len(),
        );

        // Create a signed record binding (this node, blob, size) to the
        // record key, so peers resolving it can authenticate that the
        // announcement was made by the peer it names. See
        // `crate::blob_provider_record`.
        let value = match crate::blob_provider_record::BlobProviderRecord::signed_value(
            key.as_ref(),
            &self.identity,
            request.size,
        ) {
            Ok(value) => value,
            Err(err) => {
                // Log the underlying signing error server-side only; return a
                // generic error so key/crypto detail can't leak into a caller
                // response or downstream log.
                warn!("Failed to sign blob provider record: {:?}", err);
                return Response::reply(Err(eyre!("failed to sign blob provider record")));
            }
        };

        let record = Record::new(key, value);

        debug!(
            "Storing DHT record with key length {} and value length {}",
            record.key.as_ref().len(),
            record.value.len()
        );

        match self
            .swarm
            .behaviour_mut()
            .kad
            .put_record(record, Quorum::One)
        {
            Ok(_) => {
                debug!("Successfully stored blob record in DHT");
                Response::reply(Ok(()))
            }
            Err(err) => {
                warn!("Failed to store blob record in DHT: {:?}", err);
                Response::reply(Err(eyre!("Failed to store record: {:?}", err)))
            }
        }
    }
}
