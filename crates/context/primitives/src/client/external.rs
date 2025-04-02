use std::ops::Deref;

use calimero_primitives::context::{ContextConfigParams, ContextId};
use calimero_store::key;

use super::ContextClient;

mod config;
mod proxy;

impl ContextClient {
    pub fn context_config(
        &self,
        context_id: &ContextId,
    ) -> eyre::Result<Option<ContextConfigParams<'static>>> {
        let handle = self.datastore.handle();

        let key = key::ContextConfig::new(*context_id);

        let Some(config) = handle.get(&key)? else {
            return Ok(None);
        };

        let context_config = ContextConfigParams {
            protocol: config.protocol.into_string().into(),
            network_id: config.network.into_string().into(),
            contract_id: config.contract.into_string().into(),
            proxy_contract: config.proxy_contract.into_string().into(),
            application_revision: config.application_revision,
            members_revision: config.members_revision,
        };

        Ok(Some(context_config))
    }
}

#[derive(Debug)]
pub struct ExternalClient<'a> {
    context_id: ContextId,
    client: &'a ContextClient,
    config: ContextConfigParams<'static>,
}

impl Deref for ExternalClient<'_> {
    type Target = ContextClient;

    fn deref(&self) -> &Self::Target {
        self.client
    }
}

impl ContextClient {
    pub fn external_client(
        &self,
        context_id: &ContextId,
    ) -> eyre::Result<Option<ExternalClient<'_>>> {
        let Some(params) = self.context_config(context_id)? else {
            return Ok(None);
        };

        Ok(Some(ExternalClient {
            context_id: *context_id,
            client: self,
            config: params,
        }))
    }
}
