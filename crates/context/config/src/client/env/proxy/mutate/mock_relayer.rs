use crate::client::env::Method;
use crate::client::protocol::mock_relayer::MockRelayer;
use crate::ProposalWithApprovals;

use super::Mutate;

impl Method<MockRelayer> for Mutate {
    const METHOD: &'static str = "proxy_mutate";

    type Returns = Option<ProposalWithApprovals>;

    fn encode(self) -> eyre::Result<Vec<u8>> {
        // Mock relayer accepts unsigned proxy requests encoded as JSON.
        serde_json::to_vec(&self.raw_request).map_err(Into::into)
    }

    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        if response.is_empty() {
            Ok(None)
        } else {
            serde_json::from_slice(&response).map_err(Into::into)
        }
    }
}
