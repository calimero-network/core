use actix::fut::wrap_future;
use actix::{ActorResponse, Handler, Message};
use calimero_context_primitives::client::external::ExternalClient;
use calimero_context_primitives::client::ContextClient;
use calimero_context_primitives::messages::join_context::{
    JoinContextRequest, JoinContextResponse,
};
use calimero_node_primitives::client::NodeClient;
use calimero_primitives::application::ApplicationId;
use calimero_primitives::blobs::BlobId;
use calimero_primitives::context::{
    Context, ContextConfigParams, ContextId, ContextInvitationPayload,
};
use calimero_primitives::hash::Hash;
use calimero_primitives::identity::PrivateKey;
use calimero_store::{key, types, Store};
use eyre;
use futures_util::AsyncRead;
use reqwest::Url;
use tracing::info;

use crate::ContextManager;

impl Handler<JoinContextRequest> for ContextManager {
    type Result = ActorResponse<Self, <JoinContextRequest as Message>::Result>;

    fn handle(
        &mut self,
        JoinContextRequest {
            identity_secret,
            invitation_payload,
        }: JoinContextRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        let task = join_context(
            self.datastore.clone(),
            self.node_client.clone(),
            self.context_client.clone(),
            invitation_payload,
            identity_secret,
        );

        ActorResponse::r#async(wrap_future::<_, Self>(Box::pin(task)))
    }
}

async fn join_context(
    datastore: Store,
    node_client: NodeClient,
    context_client: ContextClient,
    invitation_payload: ContextInvitationPayload,
    identity_secret: PrivateKey,
) -> eyre::Result<JoinContextResponse> {
    let (context_id, invitee_id, protocol, network_id, contract_id) = invitation_payload.parts()?;

    if identity_secret.public_key() != invitee_id {
        eyre::bail!("identity mismatch")
    }

    let handle = datastore.handle();
    let identity_key = key::ContextIdentity::new(context_id, invitee_id);

    if handle.has(&identity_key)? {
        return Ok(JoinContextResponse {
            context_id,
            member_public_key: invitee_id,
        });
    }

    let Some(external_config) = context_client.context_config(&context_id)? else {
        eyre::bail!("context not found");
    };

    let external_client = context_client.external_client(&context_id, &external_config)?;

    // Check if context exists
    let context_exists = handle.has(&key::ContextMeta::new(context_id))?;
    let mut config = if !context_exists {
        let config_client = external_client.config();
        // If context doesn't exist, get the proxy contract
        let proxy_contract = config_client.get_proxy_contract().await?;

        Some(ContextConfigParams {
            protocol: protocol.into(),
            network_id: network_id.into(),
            contract_id: contract_id.into(),
            proxy_contract: proxy_contract.into(),
            application_revision: 0,
            members_revision: 0,
        })
    } else {
        None
    };

    // Sync context config and get context
    let context = internal_sync_context_config(
        &datastore,
        &external_client,
        &node_client,
        context_id,
        config.as_mut(),
    )
    .await?;

    // Check if we are now a member
    if !handle.has(&identity_key)? {
        eyre::bail!("unable to join context: not a member, invalid invitation?")
    }

    // Add the context and identity
    add_context(&datastore, &context, identity_secret, config)?;

    // Subscribe to network updates for this context
    subscribe(&node_client, &context_id).await?;

    info!(%context_id, "Joined context with pending catchup");

    Ok(JoinContextResponse {
        context_id,
        member_public_key: invitee_id,
    })
}

async fn internal_sync_context_config(
    datastore: &Store,
    external_client: &ExternalClient<'_>,
    node_client: &NodeClient,
    context_id: ContextId,
    config: Option<&mut ContextConfigParams<'_>>,
) -> eyre::Result<Context> {
    let mut handle = datastore.handle();

    let context = handle.get(&key::ContextMeta::new(context_id))?;

    let mut alt_config = config.as_ref().map_or_else(
        || {
            let Some(config) = handle.get(&key::ContextConfig::new(context_id))? else {
                eyre::bail!("Context config not found")
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
        eyre::bail!("Context config not found")
    };

    let config_client = external_client.config();

    let members_revision = config_client.members_revision().await?;

    if !context_exists || members_revision != config.members_revision {
        config.members_revision = members_revision;

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

    let application_revision = config_client.application_revision().await?;

    let mut application_id = None;

    if !context_exists || application_revision != config.application_revision {
        config.application_revision = application_revision;

        let application = config_client.application().await?;

        let application_id = {
            let id = application.id;
            application_id = Some(id);
            id
        };

        if !is_application_installed(&datastore, &node_client, &application_id)? {
            let source: Url = application.source.into();

            let metadata = application.metadata.to_vec();

            let derived_application_id = match source.scheme() {
                "http" | "https" => {
                    node_client
                        .install_application_from_url(source, metadata, None)
                        .await?
                }
                _ => {
                    node_client
                        .install_application_from_path(source.path().into(), metadata)
                        .await?
                }
            };

            if application_id != derived_application_id {
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
            let context = Context::new(
                context_id,
                application_id.unwrap_or_else(|| meta.application.application_id()),
                meta.root_hash.into(),
            );

            save_context(datastore, &context)?;

            Ok(context)
        },
    )
}

pub async fn add_blob<S: AsyncRead>(
    node_client: &NodeClient,
    stream: S,
    expected_size: Option<u64>,
    expected_hash: Option<Hash>,
) -> eyre::Result<(BlobId, u64)> {
    let (blob_id, size) = node_client
        .add_blob(stream, expected_size, expected_hash.as_ref())
        .await?;

    if matches!(expected_size, Some(expected_size) if size != expected_size) {
        eyre::bail!("fatal: blob size mismatch");
    }

    Ok((blob_id, size))
}

pub fn is_application_installed(
    datastore: &Store,
    node_client: &NodeClient,
    application_id: &ApplicationId,
) -> eyre::Result<bool> {
    let handle = datastore.handle();

    if let Some(application) = handle.get(&key::ApplicationMeta::new(*application_id))? {
        if has_blob_available(node_client, &application.blob.blob_id())? {
            return Ok(true);
        }
    }

    Ok(false)
}

pub fn has_blob_available(node_client: &NodeClient, blob_id: &BlobId) -> eyre::Result<bool> {
    node_client.has_blob(blob_id)
}

async fn subscribe(network_client: &NodeClient, context_id: &ContextId) -> eyre::Result<()> {
    network_client.subscribe(context_id).await?;

    info!(%context_id, "Subscribed to context");

    Ok(())
}

fn add_context(
    datastore: &Store,
    context: &Context,
    identity_secret: PrivateKey,
    context_config: Option<ContextConfigParams<'_>>,
) -> eyre::Result<()> {
    let mut handle = datastore.handle();

    if let Some(context_config) = context_config {
        handle.put(
            &key::ContextConfig::new(context.id),
            &types::ContextConfig::new(
                context_config.protocol.into_owned().into_boxed_str(),
                context_config.network_id.into_owned().into_boxed_str(),
                context_config.contract_id.into_owned().into_boxed_str(),
                context_config.proxy_contract.into_owned().into_boxed_str(),
                context_config.application_revision,
                context_config.members_revision,
            ),
        )?;

        save_context(datastore, context)?;
    }

    handle.put(
        &key::ContextIdentity::new(context.id, identity_secret.public_key()),
        &types::ContextIdentity {
            private_key: Some(*identity_secret),
            sender_key: None, // In join_context we initially don't set a sender key
        },
    )?;

    Ok(())
}

fn save_context(datastore: &Store, context: &Context) -> eyre::Result<()> {
    let mut handle = datastore.handle();

    handle.put(
        &key::ContextMeta::new(context.id),
        &types::ContextMeta::new(
            key::ApplicationMeta::new(context.application_id),
            context.root_hash.into(),
        ),
    )?;

    Ok(())
}
