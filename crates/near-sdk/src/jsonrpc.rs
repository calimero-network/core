use calimero_sdk::env::internal::fetch;
use serde::de::DeserializeOwned;
use serde::Deserialize;
use std::collections::HashMap;

pub struct Client {
    url: String,
}

impl Client {
    pub fn new(url: String) -> Self {
        Self { url }
    }

    pub fn call<T: DeserializeOwned, E: DeserializeOwned>(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> Response<T, E> {
        let headers = HashMap::from([("Content-Type".to_string(), "application/json".to_string())]);

        let body = serde_json::to_vec(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": "1",
            "method": method,
            "params": params,
        }))
        .unwrap();

        let response = unsafe { fetch("POST", &self.url, headers, body) };
        serde_json::from_str(&response).unwrap()
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct Response<T, E> {
    pub result: Option<T>,
    pub error: Option<RpcError<E>>,
    pub id: String,

    pub jsonrpc: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RpcError<E> {
    pub code: i32,
    pub message: String,
    pub data: Option<E>,
}
