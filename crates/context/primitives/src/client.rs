#![allow(clippy::multiple_inherent_impl, reason = "better readability")]

use async_stream::try_stream;
use calimero_context_config::client::{AnyTransport, Client as ExternalClient};
use calimero_context_config::types::{
    BlockHeight, InvitationFromMember, RevealPayloadData, SignedOpenInvitation, SignedRevealPayload,
};
use calimero_node_primitives::client::NodeClient;
use calimero_primitives::alias::Alias;
use calimero_primitives::application::ApplicationId;
use calimero_primitives::common::DIGEST_SIZE;
use calimero_primitives::context::{
    Context, ContextConfigParams, ContextId, ContextInvitationPayload,
};
use calimero_primitives::identity::{PrivateKey, PublicKey};
use calimero_store::{key, Store};
use calimero_utils_actix::LazyRecipient;
use eyre::{bail, ContextCompat, WrapErr};
use futures_util::Stream;
use rand::Rng;
use sha2::{Digest, Sha256};
use tokio::sync::oneshot;

use crate::messages::{
    ContextMessage, CreateContextRequest, CreateContextResponse, DeleteContextRequest,
    DeleteContextResponse, ExecuteError, ExecuteRequest, ExecuteResponse, JoinContextRequest,
    JoinContextResponse, UpdateApplicationRequest,
};
use crate::ContextAtomic;

pub mod crypto;
pub mod external;
mod sync;

/// A client for interacting with the context management system.
///
/// This struct serves as the primary public API, providing methods to create,
/// join, query, and manage contexts and their members. It orchestrates
/// interactions between the datastore, background actors, and external networks.
#[derive(Clone, Debug)]
pub struct ContextClient {
    /// A handle to the persistent key-value store for all context-related data.
    datastore: Store,
    /// A client for communicating with the underlying Calimero node.
    node_client: NodeClient,
    /// A client for interacting with external services, such as on-chain smart contracts.
    external_client: ExternalClient<AnyTransport>,
    /// A lazy-initialized sender handle to the `ContextManager` actor. This is used
    /// to send asynchronous messages for processing.
    context_manager: LazyRecipient<ContextMessage>,
}

impl ContextClient {
    #[must_use]
    pub const fn new(
        datastore: Store,
        node_client: NodeClient,
        external_client: ExternalClient<AnyTransport>,
        context_manager: LazyRecipient<ContextMessage>,
    ) -> Self {
        Self {
            datastore,
            node_client,
            external_client,
            context_manager,
        }
    }

    /// Returns a handle to the datastore for direct access.
    /// Used by node components that need to read stored data.
    pub fn datastore_handle(&self) -> calimero_store::Handle<Store> {
        self.datastore.handle()
    }

    /// Sends a request to create a new context.
    ///
    /// This operation is asynchronous and is handled by the `ContextManager` actor.
    ///
    /// # Arguments
    ///
    /// * `protocol` - The name of the protocol that will be used for the new context.
    /// * `application_id` - The ID of the application that will run in the context.
    /// * `identity_secret` - An optional private key to use for the initial identity. If not
    ///   provided, a new identity will be generated.
    /// * `init_params` - Raw byte parameters for initializing the application state.
    /// * `seed` - An optional 32-byte seed for deterministic context ID and identity creation.
    ///
    /// # Returns
    ///
    /// A `Result` containing the `CreateContextResponse` from the actor upon completion.
    pub async fn create_context(
        &self,
        protocol: String,
        application_id: &ApplicationId,
        identity_secret: Option<PrivateKey>,
        init_params: Vec<u8>,
        seed: Option<[u8; DIGEST_SIZE]>,
    ) -> eyre::Result<CreateContextResponse> {
        let (sender, receiver) = oneshot::channel();

        self.context_manager
            .send(ContextMessage::CreateContext {
                request: CreateContextRequest {
                    protocol,
                    seed,
                    application_id: *application_id,
                    identity_secret,
                    init_params,
                },
                outcome: sender,
            })
            .await
            .expect("Mailbox not to be dropped");

        receiver.await.expect("Mailbox not to be dropped")
    }

    /// Invites a new member to an existing context.
    ///
    /// This involves an external call to the on-chain contract to register the new member.
    ///
    /// # Arguments
    ///
    /// * `context_id` - The context to invite the member to.
    /// * `inviter_id` - The public key of an existing member who is performing the invitation.
    /// * `invitee_id` - The public key of the identity being invited.
    ///
    /// # Returns
    ///
    /// * A `Result` containing an `Option` with the shareable `ContextInvitationPayload`.
    /// * Returns `Ok(None)` if the context configuration cannot be found locally.
    pub async fn invite_member(
        &self,
        context_id: &ContextId,
        inviter_id: &PublicKey,
        invitee_id: &PublicKey,
    ) -> eyre::Result<Option<ContextInvitationPayload>> {
        let Some(external_config) = self.context_config(context_id)? else {
            return Ok(None);
        };

        let external_client = self.external_client(context_id, &external_config)?;

        external_client
            .config()
            .add_members(inviter_id, &[*invitee_id])
            .await?;

        let invitation_payload = ContextInvitationPayload::new(
            *context_id,
            *invitee_id,
            external_config.protocol,
            external_config.network_id,
            external_config.contract_id,
        )?;

        Ok(Some(invitation_payload))
    }

    /// Creates and signs a one-time, expiring open invitation for a new member.
    ///
    /// This method allows an existing member of a context (the inviter) to generate a
    /// shareable invitation. The method fetches the inviter's private key managed
    /// by the local node, signs the invitation details, and returns the resulting
    /// payload and signature.
    ///
    /// # Arguments
    /// * `context_id` - The context to invite the new member to.
    /// * `inviter_id` - The public key of the existing member creating the invitation.
    ///                  This node must have the corresponding private key for this identity.
    /// * `valid_for_blocks` - A number of blocks from the current block height for which the
    ///   invitation is considered to be valid.
    /// * `secret_salt` - A 32-byte random value to ensure the invitation is unique.
    ///
    /// # Returns
    /// * A `Result` containing the `SignedOpenInvitation` if successful, or an error if
    /// the inviter's private key is not found or signing fails.
    /// * Returns `Ok(None)` if the context configuration cannot be found locally.
    pub async fn invite_member_by_open_invitation(
        &self,
        context_id: &ContextId,
        inviter_id: &PublicKey,
        valid_for_blocks: BlockHeight,
        _secret_salt: [u8; DIGEST_SIZE],
    ) -> eyre::Result<Option<SignedOpenInvitation>> {
        // TODO(identity): figure out the best place to generate salt.
        // We temporarily ignore the passed `secret_salt` as we can't generate it in admin
        // `invite_to_context_open_invitation::handler` as `Rng` is not thread-safe.
        let mut rng = rand::thread_rng();
        let salt: [u8; DIGEST_SIZE] = rng.gen::<[_; DIGEST_SIZE]>();
        let secret_salt = salt;

        let Some(external_config) = self.context_config(context_id)? else {
            return Ok(None);
        };

        if external_config.protocol != "near" {
            bail!("Failed to create an open invitaiton: only NEAR Protocol currently supports this feature");
        }

        //let external_client = self.external_client(context_id, &external_config)?;
        // TODO: query the current block height from the NEAR client and use it to calculate the
        // real expiration block height.
        let current_block_height: BlockHeight = 999_999_999;
        let expiration_block_height = current_block_height + valid_for_blocks;

        // 1. Fetch the inviter's identity to get their private key for signing.
        let inviter_identity = self
            .get_identity(context_id, inviter_id)?
            .with_context(|| format!("Inviter identity {inviter_id} not found"))?;
        let inviter_private_key = inviter_identity.private_key()?;

        let inviter_identity: [u8; DIGEST_SIZE] = **inviter_id;
        let inviter_identity_context_type = inviter_identity.into();
        let context_id = **context_id;

        // 2. Construct the invitation payload.
        let invitation = InvitationFromMember {
            inviter_identity: inviter_identity_context_type,
            context_id: context_id.into(),
            expiration_height: expiration_block_height,
            secret_salt,
            protocol: external_config.protocol.to_string(),
            network: external_config.network_id.to_string(),
            contract_id: external_config.contract_id.to_string(),
        };

        // 3. Sign the invitation payload.
        // The process is: borsh-serialize -> sha256-hash -> sign the hash.
        let invitation_bytes =
            borsh::to_vec(&invitation).context("Failed to serialize invitation")?;
        let hash = Sha256::digest(&invitation_bytes);
        let signature = inviter_private_key.sign(&hash).context("Signing failed")?;

        // 4. Hex-encode the signature and return the complete package.
        Ok(Some(SignedOpenInvitation {
            invitation,
            inviter_signature: hex::encode(signature.to_bytes()),
        }))
    }

    /// Sends a request to join a context using an invitation payload.
    ///
    /// This is an asynchronous operation handled by the `ContextManager` actor. The actor
    /// will parse the payload, validate the information, and configure the local node
    /// to participate in the specified context.
    ///
    /// # Arguments
    ///
    /// * `invitation_payload` - The opaque `ContextInvitationPayload` received from an inviter.
    ///
    /// # Returns
    ///
    /// A `Result` containing the `JoinContextResponse` from the actor upon completion.
    pub async fn join_context(
        &self,
        invitation_payload: ContextInvitationPayload,
    ) -> eyre::Result<JoinContextResponse> {
        let (sender, receiver) = oneshot::channel();

        self.context_manager
            .send(ContextMessage::JoinContext {
                request: JoinContextRequest { invitation_payload },
                outcome: sender,
            })
            .await
            .expect("Mailbox not to be dropped");

        receiver.await.expect("Mailbox not to be dropped")
    }

    // TODO(invitation): implement
    /// Sends a request to join a context using the new commit-reveal open invitation flow.
    ///
    /// This is an asynchronous operation handled by the `ContextManager` actor. The actor
    /// will parse the payload, validate the information, and configure the local node
    /// to participate in the specified context.
    ///
    /// # Arguments
    ///
    /// * `invitation_payload` - The opaque `ContextInvitationPayload` received from an inviter.
    ///
    /// # Returns
    ///
    /// * A `Result` containing the `JoinContextResponse` from the actor upon completion.
    /// * Returns `Ok(None)` if the context configuration cannot be found locally.
    pub async fn join_context_by_open_invitation(
        &self,
        signed_invitation: SignedOpenInvitation,
        new_member_public_key: &PublicKey,
    ) -> eyre::Result<Option<JoinContextResponse>> {
        let invitation = signed_invitation.invitation.clone();
        // Convert `config::types::ContextId` to `crypto::ContextId`
        let context_id = invitation.context_id.to_bytes().into();
        println!("Try to join by open invitation the Context ID: {context_id}");

        // At this step the identity should be at the zeroth context:
        // it should exist on the node, but available to be assigned for a new context.
        let new_member_identity = self
            .get_identity(&ContextId::zero(), new_member_public_key)?
            .with_context(|| format!("New member's identity {new_member_public_key} not found"))?;
        let new_member_private_key = new_member_identity.private_key()?;

        // Convert `crypto::ContextIdentity` to `calimero_contex_config::types::ContextId`
        let new_member_identity_bytes: [u8; DIGEST_SIZE] = *new_member_identity.public_key;
        let new_member_identity_context_type = new_member_identity_bytes.into();

        let reveal_payload_data = RevealPayloadData {
            signed_open_invitation: signed_invitation,
            new_member_identity: new_member_identity_context_type,
        };

        let reveal_payload_data_bytes =
            borsh::to_vec(&reveal_payload_data).context("Failed to serialize invitation")?;
        let commitment_hash = hex::encode(Sha256::digest(&reveal_payload_data_bytes));
        println!("New member commitment hash: {commitment_hash:?}");

        // Create a config for the external client.
        // We don't have a config for that context ID yet, as we are about to join it.
        let mut external_config_params = None;
        if !self.has_context(&context_id)? {
            let mut external_config = ContextConfigParams {
                protocol: invitation.protocol.into(),
                network_id: invitation.network.into(),
                contract_id: invitation.contract_id.into(),
                proxy_contract: "".into(),
                application_revision: 0,
                members_revision: 0,
            };

            let external_client = self.external_client(&context_id, &external_config)?;
            let config_client = external_client.config();
            let proxy_contract = config_client.get_proxy_contract().await?;
            external_config.proxy_contract = proxy_contract.into();

            external_config_params = Some(external_config);
        }

        let external_config_params =
            external_config_params.context("External config is None while it should be set")?;
        let external_client = self.external_client(&context_id, &external_config_params)?;

        external_client
            .config()
            .join_context_commit_invitation(
                &new_member_identity.public_key,
                commitment_hash,
                reveal_payload_data
                    .signed_open_invitation
                    .invitation
                    .expiration_height,
            )
            .await?;
        println!("Successfully committed the invitation payload");

        // The new member that is going to join the context by open invitation, signs the payload
        // that will be committed and revealed later
        let new_member_signature = {
            let hash = Sha256::digest(&reveal_payload_data_bytes);
            let signature = new_member_private_key
                .sign(&hash)
                .context("Signing reveal payload data failed")?;
            hex::encode(signature.to_bytes())
        };
        println!("New member signature: {new_member_signature:?}");

        let signed_payload = SignedRevealPayload {
            data: reveal_payload_data,
            invitee_signature: new_member_signature,
        };

        external_client
            .config()
            .join_context_reveal_invitation(&new_member_identity.public_key, signed_payload)
            .await?;
        println!("Successfully submitted the revealed invitation payload");

        // Create the ContextInvitationPayload
        let invitation_payload = ContextInvitationPayload::new(
            context_id,
            new_member_identity.public_key,
            external_config_params.protocol,
            external_config_params.network_id,
            external_config_params.contract_id,
        )?;

        // Join the context in the node
        let (sender, receiver) = oneshot::channel();

        self.context_manager
            .send(ContextMessage::JoinContext {
                request: JoinContextRequest { invitation_payload },
                outcome: sender,
            })
            .await
            .expect("Mailbox not to be dropped");

        let response = receiver.await.expect("Mailbox not to be dropped")?;
        Ok(Some(response))
    }

    /// Checks if a context's metadata exists in the local datastore.
    ///
    /// # Arguments
    ///
    /// * `context_id` - The ID of the context to check for.
    ///
    /// # Returns
    ///
    /// A `Result` containing `true` if the context exists locally, `false` otherwise.
    pub fn has_context(&self, context_id: &ContextId) -> eyre::Result<bool> {
        let handle = self.datastore.handle();

        let key = key::ContextMeta::new(*context_id);

        Ok(handle.has(&key)?)
    }

    /// Retrieves a context metadata from the local datastore.
    ///
    /// # Arguments
    ///
    /// * `context_id` - The ID of the context to retrieve.
    ///
    /// # Returns
    ///
    /// A `Result` containing `Some(Context)` if the context is found, or `None` if it is not.
    pub fn get_context(&self, context_id: &ContextId) -> eyre::Result<Option<Context>> {
        let handle = self.datastore.handle();

        let key = key::ContextMeta::new(*context_id);

        let Some(meta) = handle.get(&key)? else {
            return Ok(None);
        };

        let context = Context::with_dag_heads(
            *context_id,
            meta.application.application_id(),
            meta.root_hash.into(),
            meta.dag_heads.clone(),
        );

        tracing::debug!(
            %context_id,
            dag_heads_count = meta.dag_heads.len(),
            "Loaded context from database"
        );

        Ok(Some(context))
    }

    /// Updates the DAG heads for a context after applying a delta.
    ///
    /// # Arguments
    ///
    /// * `context_id` - The ID of the context to update.
    /// * `dag_heads` - The new DAG heads (typically the delta ID that was just applied).
    ///
    /// # Returns
    ///
    /// A `Result` indicating success or failure.
    pub fn update_dag_heads(
        &self,
        context_id: &ContextId,
        dag_heads: Vec<[u8; 32]>,
    ) -> eyre::Result<()> {
        let handle = self.datastore.handle();

        let key = key::ContextMeta::new(*context_id);

        let Some(mut meta) = handle.get(&key)? else {
            eyre::bail!("Context not found: {}", context_id);
        };

        // Update dag_heads
        meta.dag_heads = dag_heads.clone();

        // Write back to database
        self.datastore.clone().handle().put(&key, &meta)?;

        tracing::debug!(
            %context_id,
            dag_heads_count = dag_heads.len(),
            "Updated dag_heads in database"
        );

        Ok(())
    }

    /// Returns a stream of all context IDs stored locally.
    ///
    /// # Arguments
    ///
    /// * `start` - An optional `ContextId` from which to begin the stream. If `None`,
    ///    the stream starts from the beginning.
    ///
    /// # Returns
    ///
    /// An implementation of `Stream` that yields `Result<ContextId>`.
    pub fn get_context_ids(
        &self,
        start: Option<ContextId>,
    ) -> impl Stream<Item = eyre::Result<ContextId>> {
        let handle = self.datastore.handle();

        try_stream! {
            let mut iter = handle.iter::<key::ContextMeta>()?;

            let start = start.and_then(|s| iter.seek(key::ContextMeta::new(s)).transpose());

            for key in start.into_iter().chain(iter.keys()) {
                yield key?.context_id();
            }
        }
    }

    /// Checks if a given public key is a member of a context in the local datastore.
    ///
    /// # Arguments
    ///
    /// * `context_id` - The context to check within.
    /// * `public_key` - The public key of the potential member.
    ///
    /// # Returns
    ///
    /// A `Result` containing `true` if the identity is a known member, `false` otherwise.
    pub fn has_member(
        &self,
        context_id: &ContextId,
        public_key: &PublicKey,
        // is_owned: Option<bool>,
    ) -> eyre::Result<bool> {
        let handle = self.datastore.handle();

        let key = key::ContextIdentity::new(*context_id, *public_key);

        Ok(handle.has(&key)?)
    }

    /// Retrieves and returns a stream of all members of a given context.
    ///
    /// # Arguments
    ///
    /// * `context_id` - The context to query for members.
    /// * `owned` - If `Some(true)`, the stream returns only members for which this node holds
    ///    the private key. If `Some(false)` or `None`, it returns all members.
    ///
    /// # Returns
    ///
    /// A stream of tuples `(PublicKey, bool)`, where the boolean indicates if the identity is owned.
    pub fn get_context_members(
        &self,
        context_id: &ContextId,
        owned: Option<bool>,
    ) -> impl Stream<Item = eyre::Result<(PublicKey, bool)>> {
        let handle = self.datastore.handle();
        let context_id = *context_id;
        let only_owned = owned.unwrap_or(false);

        try_stream! {
            let mut iter = handle.iter::<key::ContextIdentity>()?;

            let first = iter
                .seek(key::ContextIdentity::new(context_id, [0; DIGEST_SIZE].into()))
                .transpose()
                .map(|k| (k, iter.read()));

            for (k, v) in first.into_iter().chain(iter.entries()) {
                let (k, v) = (k?, v?);

                if k.context_id() != context_id {
                    break;
                }

                let is_owned = v.private_key.is_some();
                if !only_owned || is_owned {
                    yield (k.public_key(), is_owned);
                }
            }
        }
    }

    /// Sends a request to execute a method within a context.
    ///
    /// This is the primary way to interact with the application running inside a context.
    /// The request is handled asynchronously by the `ContextManager` actor.
    ///
    /// # Arguments
    ///
    /// * `context_id` - The ID of the context where the execution should occur.
    /// * `executor` - The public key of the identity performing the execution. The executor
    ///   must be a member of the context.
    /// * `method` - The string name of the application method to call.
    /// * `payload` - The input data (e.g., serialized JSON) for the method.
    /// * `aliases` - A list of public key aliases to use for this specific execution.
    /// * `atomic` - An optional handle for batching multiple executions into an atomic transaction.
    ///
    /// # Returns
    ///
    /// A `Result` containing the `ExecuteResponse` on success, or an `ExecuteError` on failure.
    pub async fn execute(
        &self,
        context_id: &ContextId,
        executor: &PublicKey,
        method: String,
        payload: Vec<u8>,
        aliases: Vec<Alias<PublicKey>>,
        atomic: Option<ContextAtomic>,
    ) -> Result<ExecuteResponse, ExecuteError> {
        let (sender, receiver) = oneshot::channel();

        self.context_manager
            .send(ContextMessage::Execute {
                request: ExecuteRequest {
                    context: *context_id,
                    executor: *executor,
                    method,
                    payload,
                    aliases,
                    atomic,
                },
                outcome: sender,
            })
            .await
            .expect("Mailbox not to be dropped");

        receiver.await.expect("Mailbox not to be dropped")
    }

    /// Sends a request to update the application for a given context.
    /// This is an asynchronous operation handled by the `ContextManager` actor.
    ///
    /// # Arguments
    /// * `context_id` - The ID of the context where to update the application.
    /// * `application_id` - The ID of the new application to switch to.
    /// * `identity` - The public key of the member authorizing the update.
    ///
    /// # Returns
    ///
    /// An empty `Result` indicating the outcome of the application update request.
    pub async fn update_application(
        &self,
        context_id: &ContextId,
        application_id: &ApplicationId,
        identity: &PublicKey,
    ) -> eyre::Result<()> {
        let (sender, receiver) = oneshot::channel();

        self.context_manager
            .send(ContextMessage::UpdateApplication {
                request: UpdateApplicationRequest {
                    context_id: *context_id,
                    application_id: *application_id,
                    public_key: *identity,
                },
                outcome: sender,
            })
            .await
            .expect("Mailbox not to be dropped");

        receiver.await.expect("Mailbox not to be dropped")
    }

    /// Sends a request to delete a context from the local node.
    /// This is an asynchronous operation handled by the `ContextManager` actor. It will remove
    /// all associated data for the context from the local datastore.
    ///
    /// # Arguments
    /// * `context_id` - The ID of the context to delete.
    ///
    /// # Returns
    ///
    ///A `Result` containing the `DeleteContextResponse` from the actor.
    pub async fn delete_context(
        &self,
        context_id: &ContextId,
    ) -> eyre::Result<DeleteContextResponse> {
        let (sender, receiver) = oneshot::channel();

        self.context_manager
            .send(ContextMessage::DeleteContext {
                request: DeleteContextRequest {
                    context_id: *context_id,
                },
                outcome: sender,
            })
            .await
            .expect("Mailbox not to be dropped");

        receiver.await.expect("Mailbox not to be dropped")
    }
}
