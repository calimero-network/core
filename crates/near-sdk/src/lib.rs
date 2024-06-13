use jsonrpc::Response;
use serde::Deserialize;

mod jsonrpc;

pub struct NearSdk {
    jsonrpc_client: jsonrpc::Client,
}

#[derive(Debug, Clone, Deserialize)]
pub struct BalanceResponse {
    pub amount: String,
}

impl NearSdk {
    pub fn testnet() -> Self {
        Self {
            jsonrpc_client: jsonrpc::Client::new("https://rpc.testnet.near.org/".to_string()),
        }
    }

    pub fn mainnet() -> Self {
        Self {
            jsonrpc_client: jsonrpc::Client::new("https://rpc.mainnet.near.org/".to_string()),
        }
    }

    pub fn get_balance(&self, account_id: &str) -> BalanceResponse {
        let response: Response<BalanceResponse, String> = self.jsonrpc_client.call(
            "query",
            serde_json::json!({
                "request_type": "view_account",
                "finality": "final",
                "account_id": account_id,
            }),
        );

        if let Some(response) = response.result {
            response
        } else if let Some(error) = response.error {
            panic!("Error: {:?}", error.message);
        } else {
            panic!("Error: no response or error field in response");
        }
    }
}
