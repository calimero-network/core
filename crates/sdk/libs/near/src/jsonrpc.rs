use calimero_sdk::env;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::error::NearLibError;

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

    pub fn call<T: DeserializeOwned, E: DeserializeOwned, P: Serialize>(
        &self,
        method: &str,
        params: P,
    ) -> Result<Response<T, E>, NearLibError> {
        let headers = [("Content-Type", "application/json")];

        *self.id.borrow_mut() += 1;
        let body = serde_json::to_vec(&Request {
            jsonrpc: "2.0",
            id: &*self.id.borrow().to_string(),
            method,
            params,
        })?;

        let response = unsafe { env::ext::fetch(&self.url, "POST", &headers, &body) }
            .map_err(NearLibError::FetchError)?;
        Ok(serde_json::from_slice::<Response<T, E>>(&response)?)
    }
}

#[derive(Debug, Clone, Serialize)]
struct Request<'a, P: Serialize> {
    pub jsonrpc: &'a str,
    pub id: &'a str,
    pub method: &'a str,

    pub params: P,
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
