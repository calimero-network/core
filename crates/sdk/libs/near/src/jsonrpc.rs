use calimero_sdk::env;
use serde::de::DeserializeOwned;
use serde::Deserialize;

pub(crate) struct Client {
    url: String,
    id: std::cell::RefCell<u64>,
}

impl Client {
    pub fn new(url: String) -> Self {
        Self {
            url,
            id: std::cell::RefCell::new(0),
        }
    }

    pub fn call<T: DeserializeOwned, E: DeserializeOwned>(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<Response<T, E>, String> {
        let headers = [("Content-Type", "application/json")];

        *self.id.borrow_mut() += 1;
        let body = serde_json::to_vec(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": self.id.borrow().to_string(),
            "method": method,
            "params": params,
        }))
        .map_err(|err| format!("Cannot serialize request: {:?}", err))?;

        let response = unsafe { env::ext::fetch(&self.url, "POST", &headers, &body) }?;
        let response = String::from_utf8(response).map_err(|e| e.to_string())?;
        serde_json::from_str::<Response<T, E>>(&response).map_err(|e| e.to_string())
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct Response<T: DeserializeOwned, E: DeserializeOwned> {
    pub jsonrpc: Option<String>,
    pub id: String,

    #[serde(with = "calimero_primitives::common::ResultAlt", flatten)]
    pub data: Result<T, RpcError<E>>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RpcError<E> {
    pub code: i32,
    pub message: String,
    pub data: Option<E>,
}
