use std::ops::Deref;

use calimero_context_config::client::AnyTransport;
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
    config: &'a ContextConfigParams<'a>,
}

impl Deref for ExternalClient<'_> {
    type Target = calimero_context_config::client::Client<AnyTransport>;

    fn deref(&self) -> &Self::Target {
        &self.client.external_client
    }
}

impl ExternalClient<'_> {
    const fn context_client(&self) -> &ContextClient {
        &self.client
    }
}

impl ContextClient {
    pub const fn external_client<'a>(
        &'a self,
        context_id: &ContextId,
        config: &'a ContextConfigParams<'a>,
    ) -> eyre::Result<ExternalClient<'a>> {
        Ok(ExternalClient {
            context_id: *context_id,
            client: self,
            config,
        })
    }
}
