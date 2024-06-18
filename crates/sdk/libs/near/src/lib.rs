use jsonrpc::Response;

mod jsonrpc;
pub mod views;

pub struct Client {
    client: jsonrpc::Client,
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

    pub fn view_account(&self, account_id: &str) -> Result<views::AccountView, String> {
        let response: Response<views::AccountView, String> = self.client.call(
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

    pub fn view_code(&self, account_id: &str) -> Result<views::ContractCodeView, String> {
        let response: Response<views::ContractCodeView, String> = self.client.call(
            "query",
            serde_json::json!({
                "request_type": "view_code",
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
