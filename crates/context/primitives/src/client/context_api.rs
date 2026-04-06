//! Store-backed context config access.

use calimero_primitives::application::Application;
use calimero_primitives::context::{ContextConfigParams, ContextId};
use calimero_primitives::identity::PublicKey;
use calimero_store::key;
use eyre::OptionExt;
use futures_util::pin_mut;
use futures_util::StreamExt;

use super::ContextClient;

impl ContextClient {
    pub fn context_config(
        &self,
        context_id: &ContextId,
    ) -> eyre::Result<Option<ContextConfigParams>> {
        let handle = self.registry.datastore.handle();

        let key = key::ContextConfig::new(*context_id);

        let Some(config) = handle.get(&key)? else {
            return Ok(None);
        };

        let context_config = ContextConfigParams {
            application_id: None,
            application_revision: config.application_revision,
            members_revision: config.members_revision,
        };

        Ok(Some(context_config))
    }

    pub async fn get_context_application(
        &self,
        context_id: &ContextId,
    ) -> eyre::Result<Application> {
        let handle = self.registry.datastore.handle();
        let meta = handle
            .get(&key::ContextMeta::new(*context_id))?
            .ok_or_eyre("context meta not found")?;
        let app_id = meta.application.application_id();
        self.node_client()
            .get_application(&app_id)?
            .ok_or_eyre("application not found")
    }

    pub async fn get_context_member_page(
        &self,
        context_id: &ContextId,
        offset: usize,
        length: usize,
    ) -> eyre::Result<Vec<PublicKey>> {
        let mut out = Vec::new();
        let stream = self.get_context_members(context_id, None);
        pin_mut!(stream);
        let mut skip = offset;
        while out.len() < length {
            let Some(next) = stream.next().await else {
                break;
            };
            let (pk, _) = next?;
            if skip > 0 {
                skip -= 1;
                continue;
            }
            out.push(pk);
        }
        Ok(out)
    }
}
