use serde::{Deserialize, Serialize};

use crate::client::protocol::near::Near;
use crate::client::protocol::starknet::Starknet;
use crate::client::protocol::Method;
use crate::repr::Repr;
use crate::types::{Application, ApplicationSource, ContextId};

#[derive(Serialize, Deserialize)]
pub struct ApplicationRequest {
    pub(crate) context_id: Repr<ContextId>,
}

impl Method<ApplicationRequest> for Near {
    const METHOD: &'static str = "application_revision";

    type Returns = Application<'static>;

    fn encode(params: &ApplicationRequest) -> eyre::Result<Vec<u8>> {
        let encoded_body = serde_json::to_vec(&params)?;
        Ok(encoded_body)
    }

    fn decode(response: &[u8]) -> eyre::Result<Self::Returns> {
        let temp: Application<'_> = serde_json::from_slice(response)?;
        Ok(Application {
            id: temp.id,
            blob: temp.blob,
            size: temp.size,
            source: ApplicationSource(temp.source.0.into_owned().into()),
            metadata: temp.metadata.to_owned(),
        })
    }
}

impl Method<ApplicationRequest> for Starknet {
    type Returns = Application<'static>;

    const METHOD: &'static str = "application_revision";

    fn encode(params: &ApplicationRequest) -> eyre::Result<Vec<u8>> {
        todo!()
    }

    fn decode(response: &[u8]) -> eyre::Result<Self::Returns> {
        todo!()
    }
}
