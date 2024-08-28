mod error;
mod jsonrpc;
pub mod query;
pub mod types;
pub mod views;

pub use error::Error;
pub use jsonrpc::Client;
pub use query::*;
use serde::de::DeserializeOwned;
use serde_json::Value;
pub use types::*;

pub trait RpcMethod {
    type Response: DeserializeOwned;
    type Error: DeserializeOwned;

    fn method_name(&self) -> &str;
    fn params(&self) -> Value;
}
