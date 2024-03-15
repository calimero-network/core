use serde::{Deserialize, Serialize};

use crate::hash;

#[derive(Clone, Serialize, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub enum NodeEvent {
    TransactionExecuted(ExecutedTransactionInfo),
    PeerJoined(PeerJoinedInfo),
}

#[derive(Clone, Serialize, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ExecutedTransactionInfo {
    pub hash: hash::Hash,
}

#[derive(Clone, Serialize, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct PeerJoinedInfo {
    pub peer_id: libp2p::PeerId,
}
