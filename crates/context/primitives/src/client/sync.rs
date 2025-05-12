use calimero_primitives::context::{Context, ContextConfigParams, ContextId};
use calimero_primitives::hash::Hash;
use calimero_store::{key, types};
use url::Url;

use super::ContextClient;

impl ContextClient {
    pub async fn sync_context_config(
        &self,
        context_id: ContextId,
        config: Option<&mut ContextConfigParams<'_>>,
    ) -> eyre::Result<Context> {
        let mut handle = self.datastore.handle();

        let context = handle.get(&key::ContextMeta::new(context_id))?;

        let mut alt_config = config.as_ref().map_or_else(
            || {
                let Some(config) = handle.get(&key::ContextConfig::new(context_id))? else {
                    eyre::bail!("context config not found")
                };

                Ok(Some(ContextConfigParams {
                    protocol: config.protocol.into_string().into(),
                    network_id: config.network.into_string().into(),
                    contract_id: config.contract.into_string().into(),
                    proxy_contract: config.proxy_contract.into_string().into(),
                    application_revision: config.application_revision,
                    members_revision: config.members_revision,
                }))
            },
            |_| Ok(None),
        )?;

        let mut config = config;
        let context_exists = alt_config.is_some();
        let Some(config) = config.as_deref_mut().or(alt_config.as_mut()) else {
            eyre::bail!("context config not found")
        };

        let members_revision = {
            let external_client = self.external_client(&context_id, config)?;

            let config_client = external_client.config();

            config_client.members_revision().await?
        };

        if !context_exists || members_revision != config.members_revision {
            config.members_revision = members_revision;

            let external_client = self.external_client(&context_id, config)?;

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
            let external_client = self.external_client(&context_id, config)?;

            let config_client = external_client.config();

            config_client.application_revision().await?
        };

        let mut application_id = None;

        if !context_exists || application_revision != config.application_revision {
            config.application_revision = application_revision;

            let external_client = self.external_client(&context_id, config)?;

            let config_client = external_client.config();

            let application = config_client.application().await?;

            application_id = Some(application.id);

            if !self.node_client.has_application(&application.id)? {
                let source: Url = application.source.into();

                let metadata = application.metadata.to_vec();

                let derived_application_id = match source.scheme() {
                    "http" | "https" => {
                        self.node_client
                            .install_application_from_url(source, metadata, None)
                            .await?
                    }
                    _ => {
                        self.node_client
                            .install_application_from_path(source.path().into(), metadata)
                            .await?
                    }
                };

                if application.id != derived_application_id {
                    eyre::bail!("application mismatch")
                }
            }
        }

        if let Some(config) = alt_config {
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

        context.map_or_else(
            || {
                Ok(Context::new(
                    context_id,
                    application_id.expect("must've been defined"),
                    Hash::default(),
                ))
            },
            |meta| {
                handle.put(&key::ContextMeta::new(context_id), &meta)?;

                let context = Context::new(
                    context_id,
                    application_id.unwrap_or_else(|| meta.application.application_id()),
                    meta.root_hash.into(),
                );

                Ok(context)
            },
        )
    }
}
