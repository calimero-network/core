use calimero_primitives::context::{Context, ContextConfigParams, ContextId};
use calimero_primitives::hash::Hash;
use calimero_store::{key, types};
use tokio::sync::oneshot;
use url::Url;
use tracing::{debug, info, warn};

use super::ContextClient;
use crate::messages::sync::SyncRequest;
use crate::messages::ContextMessage;

impl ContextClient {
    pub async fn sync_context_config(
        &self,
        context_id: ContextId,
        config: Option<ContextConfigParams<'_>>,
    ) -> eyre::Result<Context> {
        debug!("🔄 Starting context config sync: context_id={}", context_id);
        
        let mut handle = self.datastore.handle();
        let context = handle.get(&key::ContextMeta::new(context_id))?;
        
        debug!("📋 Context meta lookup: context_id={}, exists={}", context_id, context.is_some());

        let (mut config, mut should_save_config) = config.map_or_else(
            || {
                debug!("🔍 Loading existing config for context_id={}", context_id);
                let Some(config) = handle.get(&key::ContextConfig::new(context_id))? else {
                    warn!("❌ Context config not found for context_id={}", context_id);
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

                debug!("📋 Loaded existing config: protocol={}, network_id={}, contract_id={}, app_rev={}, members_rev={}", 
                       config.protocol, config.network_id, config.contract_id, config.application_revision, config.members_revision);
                Ok((config, false))
            },
            |config| {
                debug!("📋 Using provided config: protocol={}, network_id={}, contract_id={}, app_rev={}, members_rev={}", 
                       config.protocol, config.network_id, config.contract_id, config.application_revision, config.members_revision);
                Ok((config, true))
            },
        )?;

        debug!("🔄 Fetching members revision for context_id={}", context_id);
        let members_revision = {
            let external_client = self.external_client(&context_id, &config)?;
            let config_client = external_client.config();
            config_client.members_revision().await?
        };
        debug!("📊 Members revision: context_id={}, current={}, stored={}", 
               context_id, members_revision, config.members_revision);

        if context.is_none() || members_revision != config.members_revision {
            debug!("🔄 Members revision mismatch, syncing members: context_id={}, current={}, stored={}", 
                   context_id, members_revision, config.members_revision);
            should_save_config = true;
            config.members_revision = members_revision;

            let external_client = self.external_client(&context_id, &config)?;
            let config_client = external_client.config();

            let mut member_count = 0;
            for (offset, length) in (0..).map(|i| (100_usize.saturating_mul(i), 100)) {
                let members = config_client.members(offset, length).await?;

                if members.is_empty() {
                    break;
                }

                for member in members {
                    let key = key::ContextIdentity::new(context_id, member);

                    if !handle.has(&key)? {
                        debug!("👤 Adding new member: context_id={}, member={}", context_id, member);
                        handle.put(
                            &key,
                            &types::ContextIdentity {
                                private_key: None,
                                sender_key: None,
                            },
                        )?;
                    }
                    member_count += 1;
                }
            }
            debug!("✅ Synced {} members for context_id={}", member_count, context_id);
        } else {
            debug!("✅ Members revision up to date: context_id={}, revision={}", context_id, members_revision);
        }

        debug!("🔄 Fetching application revision for context_id={}", context_id);
        let application_revision = {
            let external_client = self.external_client(&context_id, &config)?;
            let config_client = external_client.config();
            config_client.application_revision().await?
        };
        debug!("📊 Application revision: context_id={}, current={}, stored={}", 
               context_id, application_revision, config.application_revision);

        let mut application_id = None;

        if context.is_none() || application_revision != config.application_revision {
            debug!("🔄 Application revision mismatch, syncing application: context_id={}, current={}, stored={}", 
                   context_id, application_revision, config.application_revision);
            should_save_config = true;
            config.application_revision = application_revision;

            let external_client = self.external_client(&context_id, &config)?;
            let config_client = external_client.config();
            let application = config_client.application().await?;
            application_id = Some(application.id);
            
            debug!("📦 Application info: context_id={}, app_id={}, source={}", 
                   context_id, application.id, application.source);

            if !self.node_client.has_application(&application.id)? {
                debug!("📥 Installing application: context_id={}, app_id={}", context_id, application.id);
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
                    warn!("❌ Application ID mismatch: context_id={}, expected={}, got={}", 
                          context_id, application.id, derived_application_id);
                    eyre::bail!("application mismatch")
                }
                debug!("✅ Application installed: context_id={}, app_id={}", context_id, application.id);
            } else {
                debug!("✅ Application already installed: context_id={}, app_id={}", context_id, application.id);
            }
        } else {
            debug!("✅ Application revision up to date: context_id={}, revision={}", context_id, application_revision);
        }

        if should_save_config {
            debug!("💾 Saving updated config: context_id={}", context_id);
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
            debug!("✅ Config saved: context_id={}", context_id);
        }

        let (should_save, application_id, root_hash) = context.map_or_else(
            || {
                debug!("🆕 Creating new context meta: context_id={}", context_id);
                (
                    true,
                    application_id.expect("must've been defined if context doesn't exist"),
                    Hash::default(),
                )
            },
            |meta| {
                debug!("📋 Using existing context meta: context_id={}, app_id={}, root_hash={:?}", 
                       context_id, meta.application.application_id(), meta.root_hash);
                (
                    application_id.is_some(),
                    application_id.unwrap_or_else(|| meta.application.application_id()),
                    meta.root_hash.into(),
                )
            },
        );

        if should_save {
            debug!("💾 Saving context meta: context_id={}, app_id={}, root_hash={}", 
                   context_id, application_id, root_hash);
            handle.put(
                &key::ContextMeta::new(context_id),
                &types::ContextMeta::new(key::ApplicationMeta::new(application_id), *root_hash),
            )?;

            debug!("🔄 Initiating context sync: context_id={}, app_id={}", context_id, application_id);
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
            debug!("✅ Context sync completed: context_id={}", context_id);
        }

        let context = Context::new(context_id, application_id, root_hash);
        info!("🎉 Context config sync completed: context_id={}, app_id={}, root_hash={}", 
              context_id, application_id, root_hash);

        Ok(context)
    }
}
