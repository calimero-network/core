use serde::Serialize;
use starknet_crypto::Felt;

use crate::client::env::Method;
use crate::client::protocol::near::Near;
use crate::client::protocol::starknet::Starknet;
use crate::repr::{Repr, ReprBytes, ReprTransmute};
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
        let (high_bytes, low_bytes) = bytes.split_at(bytes.len() / 2);

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

        // Check if it's a None response (single zero Felt)
        if response.len() == 32 && response.iter().all(|&x| x == 0) {
            return Err(eyre::eyre!("No application found"));
        }

        // First 32 bytes is the flag/version (0x0), skip it
        let response = &response[32..];

        // Next two Felts are application id (high/low)
        let mut id_bytes = [0u8; 32];
        id_bytes[..16].copy_from_slice(&response[16..32]); // high part
        id_bytes[16..].copy_from_slice(&response[48..64]); // low part
        let id = Repr::new(id_bytes.rt()?);

        // Next two Felts are blob id (high/low)
        let mut blob_bytes = [0u8; 32];
        blob_bytes[..16].copy_from_slice(&response[80..96]); // high part
        blob_bytes[16..].copy_from_slice(&response[112..128]); // low part
        let blob = Repr::new(blob_bytes.rt()?);

        // Next Felt is size (0x1af25)
        let size = u64::from_be_bytes(response[152..160].try_into()?);

        // Source string starts after the length Felt (0x2)
        let mut source_bytes = Vec::new();
        let mut i = 192; // Start after length Felt
        while i < response.len() {
            let chunk = &response[i..];
            if chunk.iter().take(32).all(|&b| b == 0) {
                break;
            }
            source_bytes.extend(chunk.iter().take(32).filter(|&&b| b != 0));
            i += 32;
        }
        let source = ApplicationSource(String::from_utf8(source_bytes)?.into());

        // Find metadata after source string (look for 0.0.1)
        let metadata_bytes: Vec<u8> = response
            .windows(5)
            .find(|window| window == b"0.0.1")
            .map(|_| b"0.0.1".to_vec())
            .unwrap_or_default();
        let metadata = ApplicationMetadata(Repr::new(metadata_bytes.into()));
        Ok(Application::new(id, blob, size, source, metadata))
    }
}
