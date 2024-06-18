use calimero_sdk::env;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

pub(crate) struct Client {
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
    ) -> Result<Response<T, E>, String> {
        let headers = [("Content-Type", "application/json")];

        let body = serde_json::to_vec(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": "1",
            "method": method,
            "params": params,
        }))
        .map_err(|err| format!("Cannot serialize request: {:?}", err))?;

        let response = unsafe { env::ext::fetch(&self.url, "POST", &headers, &body) }?;
        serde_json::from_slice(&response).unwrap()
    }
}

#[derive(Serialize, Deserialize)]
#[serde(remote = "Result")]
pub enum ResultAlt<T, E> {
    #[serde(rename = "result")]
    Ok(T),
    #[serde(rename = "error")]
    Err(E),
}

impl<T, E> From<ResultAlt<T, E>> for Result<T, E> {
    fn from(result: ResultAlt<T, E>) -> Self {
        match result {
            ResultAlt::Ok(value) => Ok(value),
            ResultAlt::Err(err) => Err(err),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct Response<T: DeserializeOwned, E: DeserializeOwned> {
    pub jsonrpc: Option<String>,
    pub id: String,

    #[serde(with = "ResultAlt")]
    pub data: Result<T, RpcError<E>>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RpcError<E> {
    pub code: i32,
    pub message: String,
    pub data: Option<E>,
}
