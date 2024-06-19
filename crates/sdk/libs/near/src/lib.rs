use jsonrpc::Response;
use query::{QueryResponseKind, RpcQueryRequest};
use types::BlockId;
use views::QueryRequest;

mod jsonrpc;
pub mod query;
pub mod types;
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

    pub fn view_account(
        &self,
        account_id: &str,
        block_id: BlockId,
    ) -> Result<views::AccountView, String> {
        let request = RpcQueryRequest {
            block_id,
            request: QueryRequest::ViewAccount {
                account_id: account_id.to_string(),
            },
        };
        let response: Response<query::RpcQueryResponse, String> = self
            .client
            .call("query", serde_json::to_value(&request).unwrap())?;

        match response.data {
            Ok(r) => {
                if let QueryResponseKind::ViewAccount(va) = r.kind {
                    return Ok(va);
                }
                return Err("Unexpected response returned.".to_string());
            }
            Err(e) => Err(format!("Error: {}, Code: {}", e.message, e.code,)),
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
            Err(e) => Err(format!("Error: {}, Code: {}", e.message, e.code,)),
        }
    }
}
