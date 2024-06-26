mod error;
mod jsonrpc;
pub mod query;
pub mod types;
pub mod views;

pub use error::Error;
pub use jsonrpc::Client;

pub trait RpcMethod {
    type Response: serde::de::DeserializeOwned;
    type Error: serde::de::DeserializeOwned;

    fn method_name(&self) -> &str;
    fn params(&self) -> serde_json::Value;
}
