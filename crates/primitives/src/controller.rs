use serde::{Deserialize, Serialize};

use crate::api;

#[derive(Serialize, Deserialize, Debug)]
pub enum Command {
    WsApiRequest(api::WsClientId, Option<api::WsRequestId>, api::ApiRequest),
}
