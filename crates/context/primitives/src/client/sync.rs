//! Context configuration synchronization for off-chain mode.
//!
//! This module keeps context config and bootstrap metadata in the local store.
//! It does not query external contracts.

use calimero_primitives::application::ApplicationId;
use calimero_primitives::context::{Context, ContextConfigParams, ContextId};
use calimero_primitives::hash::Hash;
use calimero_store::{key, types};
use tokio::sync::oneshot;

use super::ContextClient;
use crate::messages::{ContextMessage, SyncRequest};

impl ContextClient {
    pub async fn sync_context_config(
        &self,
        context_id: ContextId,
        config: Option<ContextConfigParams<'_>>,
    ) -> eyre::Result<Context> {
        let mut handle = self.datastore.handle();

        let context_meta = handle.get(&key::ContextMeta::new(context_id))?;

        let (config, should_save_config) = config.map_or_else(
            || {
                let Some(config) = handle.get(&key::ContextConfig::new(context_id))? else {
                    eyre::bail!("context config not found")
                };

                Ok((
                    ContextConfigParams {
                        protocol: config.protocol.into_string().into(),
                        network_id: config.network.into_string().into(),
                        contract_id: config.contract.into_string().into(),
                        proxy_contract: config.proxy_contract.into_string().into(),
                        application_revision: config.application_revision,
                        members_revision: config.members_revision,
                    },
                    false,
                ))
            },
            |config| Ok((config, true)),
        )?;

        if should_save_config {
            let config = config.clone();
            handle.put(
                &key::ContextConfig::new(context_id),
                &types::ContextConfig::new(
                    config.protocol.into_owned().into_boxed_str(),
                    config.network_id.into_owned().into_boxed_str(),
                    config.contract_id.into_owned().into_boxed_str(),
                    config.proxy_contract.into_owned().into_boxed_str(),
                    config.application_revision,
                    config.members_revision,
                ),
            )?;
        }

        let (application_id, root_hash, dag_heads, should_bootstrap_sync) =
            if let Some(meta) = context_meta {
                (
                    meta.application.application_id(),
                    Hash::from(meta.root_hash),
                    meta.dag_heads,
                    should_save_config,
                )
            } else {
                let parsed_application_id = config.contract_id.as_ref().parse::<ApplicationId>().map_err(
                    |_| {
                        eyre::eyre!(
                            "missing bootstrap application id for context {} (contract_id is not an ApplicationId)",
                            context_id
                        )
                    },
                )?;

                let root_hash = Hash::default();
                let dag_heads = Vec::new();

                handle.put(
                    &key::ContextMeta::new(context_id),
                    &types::ContextMeta::new(
                        key::ApplicationMeta::new(parsed_application_id),
                        *root_hash,
                        dag_heads.clone(),
                    ),
                )?;

                (parsed_application_id, root_hash, dag_heads, true)
            };

        drop(handle);

        if should_bootstrap_sync {
            let (sender, receiver) = oneshot::channel();

            self.context_manager
                .send(ContextMessage::Sync {
                    request: SyncRequest {
                        context_id,
                        application_id,
                    },
                    outcome: sender,
                })
                .await
                .expect("Mailbox not to be dropped");

            receiver.await.expect("Mailbox not to be dropped");
        }

        Ok(Context::with_dag_heads(
            context_id,
            application_id,
            root_hash,
            dag_heads,
        ))
    }
}
