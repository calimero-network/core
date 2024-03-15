use serde::{Deserialize, Serialize};

use crate::transaction;

#[derive(Clone, Serialize, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub enum NodeEvent {
    TransactionExecuted(
        TransactionExecutionStatus,
        transaction::Transaction,
        Vec<String>,
    ),
    PeerJoined(libp2p::PeerId),
}

#[derive(Clone, Serialize, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub enum TransactionExecutionStatus {
    Succeeded,
    Failed,
}
