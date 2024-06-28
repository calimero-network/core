use serde::{Deserialize, Serialize};

use crate::context::ContextId;
use crate::hash::Hash;

#[derive(Clone, Serialize, Deserialize, Debug)]
#[serde(untagged)]
pub enum NodeEvent {
    Application(ApplicationEvent),
}

#[derive(Clone, Serialize, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ApplicationEvent {
    pub context_id: ContextId,
    #[serde(flatten)]
    pub payload: ApplicationEventPayload,
}

#[derive(Clone, Serialize, Deserialize, Debug)]
#[serde(tag = "type", content = "data", rename_all = "PascalCase")]
pub enum ApplicationEventPayload {
    TransactionExecuted(ExecutedTransactionPayload),
    PeerJoined(PeerJoinedPayload),
    OutcomeEvent(OutcomeEventPayload),
}

#[derive(Clone, Serialize, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ExecutedTransactionPayload {
    pub hash: Hash,
}

#[derive(Clone, Serialize, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct PeerJoinedPayload {
    pub peer_id: libp2p_identity::PeerId,
}

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct OutcomeEvent {
    pub kind: String,
    pub data: Vec<u8>,
}

#[derive(Clone, Serialize, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct OutcomeEventPayload {
    pub events: Vec<OutcomeEvent>,
}
