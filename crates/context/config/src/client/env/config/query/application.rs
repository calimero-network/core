use serde::Serialize;
use starknet::core::codec::{Decode, Encode};
use starknet_crypto::Felt;

use crate::client::env::config::types::starknet::{
    Application as StarknetApplication, CallData, FeltPair,
};
use crate::client::env::Method;
use crate::client::protocol::near::Near;
use crate::client::protocol::starknet::Starknet;
use crate::repr::Repr;
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
        let felt_pair: FeltPair = self.context_id.into();
        let mut call_data = CallData::default();
        felt_pair.encode(&mut call_data)?;
        Ok(call_data.0)
    }

    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        if response.is_empty() {
            return Err(eyre::eyre!("No application found"));
        }

        if response.len() % 32 != 0 {
            return Err(eyre::eyre!(
                "Invalid response length: {} bytes is not a multiple of 32",
                response.len()
            ));
        }

        // Convert bytes to Felts
        let mut felts = Vec::new();
        let chunks = response.chunks_exact(32);

        // Verify no remainder
        if !chunks.remainder().is_empty() {
            return Err(eyre::eyre!("Response length is not a multiple of 32 bytes"));
        }

        for chunk in chunks {
            let chunk_array: [u8; 32] = chunk
                .try_into()
                .map_err(|e| eyre::eyre!("Failed to convert chunk to array: {}", e))?;
            felts.push(Felt::from_bytes_be(&chunk_array));
        }

        if felts.is_empty() {
            return Err(eyre::eyre!("No felts decoded from response"));
        }

        // Skip version felt and decode the application
        let application = StarknetApplication::decode(&felts[1..])
            .map_err(|e| eyre::eyre!("Failed to decode application: {:?}", e))?;

        Ok(application.into())
    }
}
