use actix::{Context, Handler, Message, Response};
use calimero_network_primitives::messages::AnnounceBlob;
use eyre::eyre;
use libp2p::kad::{Quorum, Record, RecordKey};
use tracing::info;

use crate::NetworkManager;

impl Handler<AnnounceBlob> for NetworkManager {
    type Result = Response<<AnnounceBlob as Message>::Result>;

    fn handle(&mut self, request: AnnounceBlob, _ctx: &mut Context<Self>) -> Self::Result {
        info!(
            blob_id = %request.blob_id,
            context_id = %request.context_id,
            size = request.size,
            "Announcing blob to DHT"
        );

        // Create a unique key for this blob in this context
        let key = RecordKey::new(&[request.context_id.as_slice(), request.blob_id.as_slice()].concat());

        info!(
            "ANNOUNCE: blob_id={}, context_id={}, key_len={}",
            request.blob_id,
            request.context_id,
            key.as_ref().len(),
        );

        // Create a record with blob metadata (size and peer ID)
        let peer_id = *self.swarm.local_peer_id();
        let value = [peer_id.to_bytes().as_slice(), &request.size.to_le_bytes()].concat();

        let record = Record::new(key, value);

        info!(
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
                info!("Successfully stored blob record in DHT");
                Response::reply(Ok(()))
            }
            Err(err) => {
                info!("Failed to store blob record in DHT: {:?}", err);
                Response::reply(Err(eyre!("Failed to store record: {:?}", err)))
            }
        }
    }
}
