use jsonrpc_core::{Error, IoHandler, Params, Value};
use jsonrpc_http_server::{AccessControlAllowOrigin, DomainsValidation, ServerBuilder};
use std::sync::{Arc, Mutex};
use tokio;
use tracing::info;

pub struct CalimeroRPCHandler {
    mempool: Arc<Mutex<Vec<String>>>,
}

impl CalimeroRPCHandler {
    pub fn new() -> Self {
        CalimeroRPCHandler {
            mempool: Arc::new(Mutex::new(Vec::new())),
        }
    }

    pub async fn send(&self, params: Params) -> Result<Value, Error> {
        let input: Vec<String> = match params.parse() {
            Ok(val) => val,
            Err(_) => return Err(Error::invalid_params("Expected a single string parameter")),
        };

        if input.len() != 1 {
            return Err(Error::invalid_params(
                "Expected exactly one string parameter",
            ));
        }

        let mut list = self.mempool.lock().unwrap();
        list.push(input[0].clone());

        info!("Broadcasting: {}", input[0]);

        Ok(Value::String(format!("Added: {}", input[0])))
    }

    pub async fn read(&self) -> Result<Option<String>, Error> {
        let mut list = self.mempool.lock().unwrap(); // In real code, handle lock errors
        Ok(list.pop())
    }
}

impl Clone for CalimeroRPCHandler {
    fn clone(&self) -> Self {
        CalimeroRPCHandler {
            mempool: self.mempool.clone(),
        }
    }
}
