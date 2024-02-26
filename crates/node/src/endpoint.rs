use std::sync::{Arc, Mutex};

use jsonrpsee::core::async_trait;
use jsonrpsee::proc_macros::rpc;
use jsonrpsee::types::ErrorObjectOwned;
use tracing::info;

#[rpc(server)]
pub trait CalimeroRPC {
    #[method(name = "send")]
    async fn send(&self, message: String);

    #[method(name = "read")]
    async fn read(&self) -> Result<Option<String>, ErrorObjectOwned>;
}

pub struct CalimeroRPCImpl {
    mempool: Arc<Mutex<Vec<String>>>,
}

impl CalimeroRPCImpl {
    pub fn new() -> Self {
        CalimeroRPCImpl {
            mempool: Arc::new(Mutex::new(Vec::new())),
        }
    }
}

#[async_trait]
impl CalimeroRPCServer for CalimeroRPCImpl {
    async fn send(&self, message: String) {
        let mut list = self.mempool.lock().unwrap();
        list.push(message.clone());

        info!("Broadcasting: {}", message);
    }

    async fn read(&self) -> Result<Option<String>, ErrorObjectOwned> {
        let mut list = self.mempool.lock().unwrap(); // In real code, handle lock errors
        Ok(list.pop())
    }
}

impl Clone for CalimeroRPCImpl {
    fn clone(&self) -> Self {
        CalimeroRPCImpl {
            mempool: self.mempool.clone(),
        }
    }
}
