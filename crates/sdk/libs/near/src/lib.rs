use jsonrpc::Response;
use query::{QueryResponseKind, RpcQueryRequest};
use types::{BlockId, StoreKey};
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
        let response: Response<query::RpcQueryResponse, String> =
            self.client.call("query", request)?;

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

    pub fn view_code(
        &self,
        account_id: &str,
        block_id: BlockId,
    ) -> Result<views::ContractCodeView, String> {
        let request = RpcQueryRequest {
            block_id,
            request: QueryRequest::ViewCode {
                account_id: account_id.to_string(),
            },
        };

        let response: Response<query::RpcQueryResponse, String> =
            self.client.call("query", request)?;

        match response.data {
            Ok(r) => {
                if let QueryResponseKind::ViewCode(vc) = r.kind {
                    return Ok(vc);
                }
                return Err("Unexpected response returned.".to_string());
            }
            Err(e) => Err(format!("Error: {}, Code: {}", e.message, e.code,)),
        }
    }

    pub fn view_state(
        &self,
        account_id: &str,
        prefix: StoreKey,
        include_proof: bool,
        block_id: BlockId,
    ) -> Result<views::ViewStateResult, String> {
        let request = RpcQueryRequest {
            block_id,
            request: QueryRequest::ViewState {
                account_id: account_id.to_string(),
                prefix,
                include_proof,
            },
        };

        let response: Response<query::RpcQueryResponse, String> =
            self.client.call("query", request)?;

        match response.data {
            Ok(r) => {
                if let QueryResponseKind::ViewState(vs) = r.kind {
                    return Ok(vs);
                }
                return Err("Unexpected response returned.".to_string());
            }
            Err(e) => Err(format!("Error: {}, Code: {}", e.message, e.code,)),
        }
    }

    pub fn view_access_key(
        &self,
        account_id: &str,
        public_key: &str,
        block_id: BlockId,
    ) -> Result<views::ViewStateResult, String> {
        let request = RpcQueryRequest {
            block_id,
            request: QueryRequest::ViewAccessKey {
                account_id: account_id.to_string(),
                public_key: public_key.to_string(),
            },
        };

        let response: Response<query::RpcQueryResponse, String> =
            self.client.call("query", request)?;

        match response.data {
            Ok(r) => {
                if let QueryResponseKind::ViewState(vs) = r.kind {
                    return Ok(vs);
                }
                return Err("Unexpected response returned.".to_string());
            }
            Err(e) => Err(format!("Error: {}, Code: {}", e.message, e.code,)),
        }
    }
}
