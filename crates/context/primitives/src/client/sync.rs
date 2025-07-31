use calimero_primitives::context::{Context, ContextConfigParams, ContextId};
use calimero_primitives::hash::Hash;
use calimero_store::{key, types};
use tokio::sync::oneshot;
use url::Url;

use super::ContextClient;
use crate::messages::sync::SyncRequest;
use crate::messages::ContextMessage;

impl ContextClient {
    pub async fn sync_context_config(
        &self,
        context_id: ContextId,
        config: Option<ContextConfigParams<'_>>,
    ) -> eyre::Result<Context> {
        let mut handle = self.datastore.handle();

        let context = handle.get(&key::ContextMeta::new(context_id))?;

        let (mut config, mut should_save_config) = config.map_or_else(
            || {
                let Some(config) = handle.get(&key::ContextConfig::new(context_id))? else {
                    eyre::bail!("context config not found")
                };

                let config = ContextConfigParams {
                    protocol: config.protocol.into_string().into(),
                    network_id: config.network.into_string().into(),
                    contract_id: config.contract.into_string().into(),
                    proxy_contract: config.proxy_contract.into_string().into(),
                    application_revision: config.application_revision,
                    members_revision: config.members_revision,
                };

                Ok((config, false))
            },
            |config| Ok((config, true)),
        )?;

        let members_revision = {
            let external_client = self.external_client(&context_id, &config)?;

            let config_client = external_client.config();

            config_client.members_revision().await?
        };

        if context.is_none() || members_revision != config.members_revision {
            should_save_config = true;
            config.members_revision = members_revision;

            let external_client = self.external_client(&context_id, &config)?;

            let config_client = external_client.config();

            for (offset, length) in (0..).map(|i| (100_usize.saturating_mul(i), 100)) {
                let members = config_client.members(offset, length).await?;

                if members.is_empty() {
                    break;
                }

                for member in members {
                    let key = key::ContextIdentity::new(context_id, member);

                    if !handle.has(&key)? {
                        handle.put(
                            &key,
                            &types::ContextIdentity {
                                private_key: None,
                                sender_key: None,
                            },
                        )?;
                    }
                }
            }
        }

        let application_revision = {
            let external_client = self.external_client(&context_id, &config)?;

            let config_client = external_client.config();

            config_client.application_revision().await?
        };

        let mut application_id = None;

        if context.is_none() || application_revision != config.application_revision {
            should_save_config = true;
            config.application_revision = application_revision;

            let external_client = self.external_client(&context_id, &config)?;

            let config_client = external_client.config();

            let application = config_client.application().await?;

            application_id = Some(application.id);

            if !self.node_client.has_application(&application.id)? {
                let source: Url = application.source.into();

                let metadata = application.metadata;

                let derived_application_id = {
                    let app_id = match source.scheme() {
                        "http" | "https" => self
                            .node_client
                            .install_application_from_url(source.clone(), metadata.clone(), None)
                            .await
                            .ok(),
                        _ => None,
                    };

                    match app_id {
                        Some(id) => id,
                        None => self.node_client.install_application(
                            &application.blob.bytecode,
                            application.size,
                            &source.into(),
                            metadata,
                        )?,
                    }
                };

                if application.id != derived_application_id {
                    eyre::bail!("application mismatch")
                }
            }
        }

        if should_save_config {
            // todo! we shouldn't be reallocating here
            // todo! but store requires ContextConfig: 'static
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

        let (should_save, application_id, root_hash) = context.map_or_else(
            || {
                (
                    true,
                    application_id.expect("must've been defined if context doesn't exist"),
                    Hash::default(),
                )
            },
            |meta| {
                (
                    application_id.is_some(),
                    application_id.unwrap_or_else(|| meta.application.application_id()),
                    meta.root_hash.into(),
                )
            },
        );

        if should_save {
            handle.put(
                &key::ContextMeta::new(context_id),
                &types::ContextMeta::new(key::ApplicationMeta::new(application_id), *root_hash),
            )?;

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

            receiver.await.expect("Mailbox not to be dropped")
        }

        let context = Context::new(context_id, application_id, root_hash);

        Ok(context)
    }
}
