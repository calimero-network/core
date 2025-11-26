use crate::client::env::Method;
use crate::client::protocol::mock_relayer::MockRelayer;

use super::Mutate;

impl<'a> Method<MockRelayer> for Mutate<'a> {
    const METHOD: &'static str = "mutate";

    type Returns = ();

    fn encode(self) -> eyre::Result<Vec<u8>> {
        // Mock relayer skips signature verification, so we can forward the request kind directly.
        serde_json::to_vec(&self.kind).map_err(Into::into)
    }

    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        if response.is_empty() {
            Ok(())
        } else {
            eyre::bail!("mock-relayer mutate returned unexpected payload")
        }
    }
}
