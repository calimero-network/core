use serde::{Deserialize, Serialize};

use crate::application::ApplicationId;
use crate::hash::Hash;

#[derive(Clone, Serialize, Deserialize, Debug)]
#[serde(untagged)]
pub enum NodeEvent {
    ApplicationEvent(ApplicationEventPayload),
}

#[derive(Clone, Serialize, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ApplicationEventPayload {
    pub application_id: ApplicationId,
    pub event: ApplicationEventType,
}

#[derive(Clone, Serialize, Deserialize, Debug)]
#[serde(tag = "type", content = "data", rename_all = "PascalCase")]
pub enum ApplicationEventType {
    TransactionExecuted(ExecutedTransactionPayload),
    PeerJoined(PeerJoinedPayload),
}

#[derive(Clone, Serialize, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ExecutedTransactionPayload {
    pub hash: Hash,
}

#[derive(Clone, Serialize, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct PeerJoinedPayload {
    pub peer_id: libp2p::PeerId,
}
