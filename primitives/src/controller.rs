use serde::{Deserialize, Serialize};

use crate::api::{ApiRequest, WsClientId, WsRequestId};

#[derive(Serialize, Deserialize, Debug)]
pub enum Command {
    WsApiRequest(WsClientId, Option<WsRequestId>, ApiRequest),
}
