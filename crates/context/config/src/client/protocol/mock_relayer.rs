use std::borrow::Cow;
use std::collections::BTreeMap;

use thiserror::Error;
use url::Url;

use super::Protocol;
use crate::client::config::RawCredentials;
use crate::client::transport::{AssociatedTransport, TransportRequest};

// Mock relayer uses raw credentials
pub type Credentials = RawCredentials;

// Mock relayer protocol type
#[derive(Copy, Clone, Debug)]
pub enum MockRelayer {}

impl Protocol for MockRelayer {
    const PROTOCOL: &'static str = "mock-relayer";
}

#[derive(Clone, Debug)]
pub struct NetworkConfig {
    pub rpc_url: Url,
    pub credentials: RawCredentials,
}

#[derive(Debug)]
pub struct MockRelayerConfig<'a> {
    pub networks: BTreeMap<Cow<'a, str>, NetworkConfig>,
}

#[derive(Clone, Debug)]
pub struct MockRelayerTransport<'a> {
    networks: BTreeMap<Cow<'a, str>, NetworkConfig>,
}

impl<'a> MockRelayerTransport<'a> {
    #[must_use]
    pub fn new(config: &MockRelayerConfig<'a>) -> Self {
        Self {
            networks: config.networks.clone(),
        }
    }
}

impl AssociatedTransport for MockRelayerTransport<'_> {
    type Protocol = MockRelayer;
}

#[derive(Debug, Error)]
pub enum MockRelayerError {
    #[error("mock-relayer transport should not be called directly - use relayer signer")]
    NotSupported,
}

impl crate::client::transport::ProtocolTransport for MockRelayerTransport<'_> {
    type Error = MockRelayerError;

    async fn send(
        &self,
        request: TransportRequest<'_>,
        payload: Vec<u8>,
    ) -> Result<Vec<u8>, Self::Error> {
        // Mock implementation - should never be called since mock-relayer uses relayer signer
        let _ = (request, payload);
        Err(MockRelayerError::NotSupported)
    }
}
