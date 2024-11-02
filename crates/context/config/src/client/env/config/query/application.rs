use serde::Serialize;

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
        todo!()
    }

    fn decode(_response: Vec<u8>) -> eyre::Result<Self::Returns> {
        todo!()
    }
}
