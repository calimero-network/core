use jsonrpc::Response;
use serde::Deserialize;

mod jsonrpc;

pub struct NearSdk {
    client: jsonrpc::Client,
}

#[derive(Debug, Clone, Deserialize)]
pub struct BalanceResponse {
    pub amount: String,
}

impl NearSdk {
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

    pub fn get_balance(&self, account_id: &str) -> Result<BalanceResponse, String> {
        let response: Response<BalanceResponse, String> = self.client.call(
            "query",
            serde_json::json!({
                "request_type": "view_account",
                "finality": "final",
                "account_id": account_id,
            }),
        )?;

        match response.data {
            Ok(r) => Ok(r),
            Err(e) => Err(e.message),
        }
    }
}
