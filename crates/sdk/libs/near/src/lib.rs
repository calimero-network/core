use error::NearLibError;
use jsonrpc::Response;

pub mod error;
mod jsonrpc;
pub mod query;
pub mod types;
pub mod views;

pub struct Client {
    client: jsonrpc::Client,
}

pub trait RpcMethod {
    type Response: serde::de::DeserializeOwned;

    fn method_name(&self) -> &str;
    fn params(&self) -> Result<serde_json::Value, std::io::Error>;
}

impl Client {
    pub fn testnet() -> Self {
        Self {
            client: jsonrpc::Client::new("https://rpc.testnet.near.org/".to_string()),
        }
    }

    pub fn mainnet() -> Self {
        Self {
            client: jsonrpc::Client::new("https://rpc.mainnet.near.org/".to_string()),
        }
    }

    pub fn call<M>(&self, method: M) -> Result<Response<M::Response, String>, NearLibError>
    where
        M: RpcMethod,
    {
        self.client.call(method.method_name(), &method.params()?)
    }
}
