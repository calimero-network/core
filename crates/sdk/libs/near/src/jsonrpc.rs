use calimero_sdk::env;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::{Error, RpcMethod};

pub struct Client {
    url: String,
    id: std::cell::RefCell<u64>,
}

impl Client {
    pub fn testnet() -> Self {
        Self::new("https://rpc.testnet.near.org/".to_string())
    }

    pub fn mainnet() -> Self {
        Self::new("https://rpc.mainnet.near.org/".to_string())
    }

    fn new(url: String) -> Self {
        Self {
            url,
            id: std::cell::RefCell::new(0),
        }
    }

    pub fn call<M>(&self, method: M) -> Result<M::Response, Error<RpcError<M::Error>>>
    where
        M: RpcMethod,
    {
        let headers = [("Content-Type", "application/json")];

        *self.id.borrow_mut() += 1;
        let body = serde_json::to_vec(&Request {
            jsonrpc: "2.0",
            id: &*self.id.borrow().to_string(),
            method: method.method_name(),
            params: method.params()?,
        })?;

        let response = unsafe { env::ext::fetch(&self.url, "POST", &headers, &body) }
            .map_err(Error::FetchError)?;

        serde_json::from_slice::<Response<M::Response, RpcError<M::Error>>>(&response)?
            .data
            .map_err(|e| Error::ServerError(e))
    }
}

#[derive(Debug, Clone, Serialize)]
struct Request<'a, P: Serialize> {
    jsonrpc: &'a str,
    id: &'a str,
    method: &'a str,

    params: P,
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
