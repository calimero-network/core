use actix::{ActorResponse, Handler, Message, WrapFuture};
use calimero_context_config::repr::ReprTransmute;
use calimero_context_primitives::client::crypto::ContextIdentity;
use calimero_context_primitives::group::{JoinGroupContextRequest, JoinGroupContextResponse};
use calimero_primitives::context::ContextConfigParams;
use calimero_primitives::identity::PrivateKey;
use eyre::bail;
use tracing::info;

use crate::{group_store, ContextManager};

impl Handler<JoinGroupContextRequest> for ContextManager {
    type Result = ActorResponse<Self, <JoinGroupContextRequest as Message>::Result>;

    fn handle(
        &mut self,
        JoinGroupContextRequest {
            group_id,
            context_id,
        }: JoinGroupContextRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        // Resolve joiner identity from node group identity.
        let (joiner_identity, effective_signing_key) = match self.node_group_identity() {
            Some((pk, sk)) => (pk, Some(sk)),
            None => {
                return ActorResponse::reply(Err(eyre::eyre!(
                    "joiner_identity not provided and node has no configured group identity"
                )));
            }
        };

        // Validate: group exists and joiner is a member.
        if let Err(err) = (|| -> eyre::Result<()> {
            if group_store::load_group_meta(&self.datastore, &group_id)?.is_none() {
                bail!("group not found");
            }
            if !group_store::check_group_membership(&self.datastore, &group_id, &joiner_identity)? {
                bail!("identity is not a member of the group");
            }
            Ok(())
        })() {
            return ActorResponse::reply(Err(err));
        }

        let group_client_result = effective_signing_key.map(|sk| self.group_client(group_id, sk));

        let datastore = self.datastore.clone();
        let context_client = self.context_client.clone();
        let node_client = self.node_client.clone();

        let protocol = "near".to_owned();
        let params = match self.external_config.params.get("near") {
            Some(p) => p.clone(),
            None => {
                return ActorResponse::reply(Err(eyre::eyre!("no 'near' protocol config")));
            }
        };

        ActorResponse::r#async(
            async move {
                // Generate a context identity for this context.
                let mut rng = rand::thread_rng();
                let identity_secret = PrivateKey::random(&mut rng);
                let identity_pk = identity_secret.public_key();
                let sender_key = PrivateKey::random(&mut rng);

                let context_identity: calimero_context_config::types::ContextIdentity =
                    identity_pk.rt()?;

                // Call contract to add the new member to the context via group.
                if let Some(client_result) = group_client_result {
                    let group_client = client_result?;
                    group_client
                        .join_context_via_group(context_id, context_identity)
                        .await?;
                }

                // Register the context-group mapping locally so that
                // maybe_lazy_upgrade can find the group for this context.
                group_store::register_context_in_group(&datastore, &group_id, &context_id)?;

                // Ensure we have context config locally.
                // If the context is unknown, build config from protocol params
                // and fetch the proxy contract so sync_context_config can
                // bootstrap the context from on-chain state.
                let config = if !context_client.has_context(&context_id)? {
                    let mut external_config = ContextConfigParams {
                        protocol: protocol.clone().into(),
                        network_id: params.network.clone().into(),
                        contract_id: params.contract_id.clone().into(),
                        proxy_contract: "".into(),
                        application_revision: 0,
                        members_revision: 0,
                    };

                    let external_client =
                        context_client.external_client(&context_id, &external_config)?;
                    let proxy_contract = external_client.config().get_proxy_contract().await?;
                    external_config.proxy_contract = proxy_contract.into();

                    Some(external_config)
                } else {
                    None
                };

                let _ignored = context_client
                    .sync_context_config(context_id, config)
                    .await?;

                // Store the context identity locally.
                context_client.update_identity(
                    &context_id,
                    &ContextIdentity {
                        public_key: identity_pk,
                        private_key: Some(identity_secret),
                        sender_key: Some(sender_key),
                    },
                )?;

                // Subscribe to context and trigger sync.
                node_client.subscribe(&context_id).await?;
                node_client.sync(Some(&context_id), None).await?;

                info!(
                    ?group_id,
                    ?context_id,
                    %identity_pk,
                    "joined context via group membership"
                );

                Ok(JoinGroupContextResponse {
                    context_id,
                    member_public_key: identity_pk,
                })
            }
            .into_actor(self),
        )
    }
}
