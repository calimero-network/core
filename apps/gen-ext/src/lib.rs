use calimero_sdk::app;
use calimero_sdk::borsh::{BorshDeserialize, BorshSerialize};
use calimero_sdk_near::query::RpcQueryRequest;
use calimero_sdk_near::views::QueryRequest;
use calimero_sdk_near::Client;

#[app::state]
#[derive(BorshDeserialize, BorshSerialize, Default)]
#[borsh(crate = "calimero_sdk::borsh")]
pub struct GenExt;

#[app::logic]
impl GenExt {
    #[app::init]
    pub fn init() -> GenExt {
        GenExt
    }

    pub fn view_account(&mut self, account_id: &str, block_height: u64) -> String {
        let client = Client::testnet();
        let request = RpcQueryRequest {
            block_id: calimero_sdk_near::BlockId::Height(block_height),
            request: QueryRequest::ViewAccount {
                account_id: account_id.parse().unwrap(),
            },
        };
        match client.call(request) {
            Ok(r) => format!("{:?}", r),
            Err(e) => format!("{:?}", e),
        }
    }
}
