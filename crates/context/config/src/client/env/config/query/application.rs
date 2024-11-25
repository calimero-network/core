use serde::Serialize;
use starknet::core::codec::Decode;
use starknet_crypto::Felt;

use crate::client::env::config::types::starknet::Application as StarknetApplication;
use crate::client::env::Method;
use crate::client::protocol::near::Near;
use crate::client::protocol::starknet::Starknet;
use crate::repr::{Repr, ReprBytes};
use crate::types::{Application, ApplicationMetadata, ApplicationSource, ContextId};

#[derive(Copy, Clone, Debug, Serialize)]
pub(super) struct ApplicationRequest {
    pub(super) context_id: Repr<ContextId>,
}

impl Method<Near> for ApplicationRequest {
    const METHOD: &'static str = "application";

    type Returns = Application<'static>;

    fn encode(self) -> eyre::Result<Vec<u8>> {
        serde_json::to_vec(&self).map_err(Into::into)
    }

    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        let application: Application<'_> = serde_json::from_slice(&response)?;

        Ok(Application::new(
            application.id,
            application.blob,
            application.size,
            ApplicationSource(application.source.0.into_owned().into()),
            ApplicationMetadata(Repr::new(
                application.metadata.0.into_inner().into_owned().into(),
            )),
        ))
    }
}

impl Method<Starknet> for ApplicationRequest {
    type Returns = Application<'static>;

    const METHOD: &'static str = "application";

    fn encode(self) -> eyre::Result<Vec<u8>> {
        // Split context_id into high/low parts
        let bytes = self.context_id.as_bytes();
        let mid_point = bytes.len().checked_div(2).ok_or_else(|| eyre::eyre!("Length should be even"))?;
        let (high_bytes, low_bytes) = bytes.split_at(mid_point);

        // Convert to Felts
        let high_felt = Felt::from_bytes_be_slice(high_bytes);
        let low_felt = Felt::from_bytes_be_slice(low_bytes);

        // Convert both Felts to bytes and concatenate
        let mut result = Vec::new();
        result.extend_from_slice(&high_felt.to_bytes_be());
        result.extend_from_slice(&low_felt.to_bytes_be());
        Ok(result)
    }

    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        if response.is_empty() {
            return Err(eyre::eyre!("No application found"));
        }

        // Convert bytes to Felts
        let mut felts = Vec::new();
        for chunk in response.chunks(32) {
            if chunk.len() == 32 {
                let chunk_array: [u8; 32] = chunk
                    .try_into()
                    .map_err(|e| eyre::eyre!("Failed to convert chunk to array: {}", e))?;
                felts.push(Felt::from_bytes_be(&chunk_array));
            }
        }

        // Skip version felt and decode the application
        let application = StarknetApplication::decode(&felts[1..])
            .map_err(|e| eyre::eyre!("Failed to decode application: {:?}", e))?;

        Ok(application.into())
    }
}
