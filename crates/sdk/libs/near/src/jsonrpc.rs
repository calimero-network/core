use std::cell::RefCell;

use calimero_sdk::env::ext::fetch;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{from_slice as from_json_slice, to_vec as to_json_vec};

use crate::error::RpcError;
use crate::{Error, RpcMethod};

pub struct Client {
    url: String,
    id: RefCell<u64>,
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
            id: RefCell::new(0),
        }
    }

    pub fn call<M>(&self, method: M) -> Result<M::Response, Error<M::Error>>
    where
        M: RpcMethod,
    {
        let headers = [("Content-Type", "application/json")];

        *self.id.borrow_mut() += 1;
        let body = to_json_vec(&Request {
            jsonrpc: "2.0",
            id: *self.id.borrow(),
            method: method.method_name(),
            params: method.params(),
        })?;

        let response =
            unsafe { fetch(&self.url, "POST", &headers, &body) }.map_err(Error::FetchError)?;

        from_json_slice::<Response<_, _>>(&response)?
            .data
            .map_err(Error::ServerError)
    }
}

#[derive(Clone, Debug, Serialize)]
struct Request<'a, P: Serialize> {
    jsonrpc: &'a str,
    id: u64,
    method: &'a str,

    params: P,
}

#[derive(Clone, Debug, Deserialize)]
#[expect(dead_code)]
pub struct Response<T: DeserializeOwned, E: DeserializeOwned> {
    pub jsonrpc: Option<String>,
    pub id: u64,

    #[serde(with = "calimero_primitives::common::ResultAlt", flatten)]
    pub data: Result<T, RpcError<E>>,
}
