use calimero_primitives::application::ApplicationId;
use calimero_primitives::context::{ContextId, ContextInvitationPayload};
use calimero_primitives::hash::Hash;
use calimero_primitives::identity::{PrivateKey, PublicKey};
use calimero_store::key::ContextMeta as ContextMetaKey;
use clap::{Parser, Subcommand};
use eyre::Result;
use owo_colors::OwoColorize;
use serde_json::Value;
use tokio::sync::oneshot;

use crate::Node;

/// Manage contexts
#[derive(Debug, Parser)]
pub struct ContextCommand {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// List contexts
    Ls,
    /// Create a context
    Create {
        /// The application ID to create the context with
        application_id: ApplicationId,
        /// The initialization parameters for the context
        params: Option<Value>,
        /// The seed for the context (to derive a deterministic context ID)
        #[clap(long = "seed")]
        context_seed: Option<Hash>,
        /// The protocol to use for the context
        #[clap(long = "protocol")]
        protocol: String,
    },
    /// Invite a user to a context
    Invite {
        /// The context ID to invite the user to
        context_id: ContextId,
        /// The ID of the inviter
        inviter_id: PublicKey,
        /// The ID of the invitee
        invitee_id: PublicKey,
    },
    /// Join a context
    Join {
        /// The private key of the user
        private_key: PrivateKey,
        /// The invitation payload from the inviter
        invitation_payload: ContextInvitationPayload,
    },
    /// Leave a context
    Leave {
        /// The context ID to leave
        context_id: ContextId,
    },
    /// Delete a context
    Delete {
        /// The context ID to delete
        context_id: ContextId,
    },
    /// Update the proxy for a context
    UpdateProxy {
        /// The context ID to update the proxy for
        context_id: ContextId,
        /// The identity requesting the update
        public_key: PublicKey,
    },
}

impl ContextCommand {
    #[expect(clippy::similar_names, reason = "Acceptable here")]
    #[expect(clippy::too_many_lines, reason = "TODO: Will be refactored")]
    pub async fn run(self, node: &Node) -> Result<()> {
        let ind = ">>".blue();

        match self.command {
            Commands::Ls => {
                println!(
                    "{ind} {c1:44} | {c2:44} | Root Hash",
                    c1 = "Context ID",
                    c2 = "Application ID",
                );

                let handle = node.store.handle();

                for (k, v) in handle.iter::<ContextMetaKey>()?.entries() {
                    let (k, v) = (k?, v?);
                    let (cx, app_id, last_tx) =
                        (k.context_id(), v.application.application_id(), v.root_hash);
                    let entry = format!(
                        "{c1:44} | {c2:44} | {c3}",
                        c1 = cx,
                        c2 = app_id,
                        c3 = Hash::from(last_tx)
                    );
                    for line in entry.lines() {
                        println!("{ind} {}", line.cyan());
                    }
                }
            }
            Commands::Join {
                private_key,
                invitation_payload,
            } => {
                let response = node
                    .ctx_manager
                    .join_context(private_key, invitation_payload)
                    .await?;

                if let Some((context_id, identity)) = response {
                    println!(
                        "{ind} Joined context {context_id} as {identity}, waiting for catchup to complete..."
                    );
                } else {
                    println!(
                        "{ind} Unable to join context at this time, a catchup is in progress."
                    );
                }
            }
            Commands::Leave { context_id } => {
                if node.ctx_manager.delete_context(&context_id).await? {
                    println!("{ind} Successfully deleted context {context_id}");
                } else {
                    println!("{ind} Failed to delete context {context_id}");
                }
                println!("{ind} Left context {context_id}");
            }
            Commands::Create {
                application_id,
                params,
                context_seed,
                protocol,
            } => {
                let (tx, rx) = oneshot::channel();

                node.ctx_manager.create_context(
                    &protocol,
                    context_seed.map(Into::into),
                    application_id,
                    None,
                    params
                        .as_ref()
                        .map(serde_json::to_vec)
                        .transpose()?
                        .unwrap_or_default(),
                    tx,
                )?;

                let _ignored = tokio::spawn(async move {
                    let err: eyre::Report = match rx.await {
                        Ok(Ok((context_id, identity))) => {
                            println!("{ind} Created context {context_id} with identity {identity}");
                            return;
                        }
                        Ok(Err(err)) => err,
                        Err(err) => err.into(),
                    };

                    println!("{ind} Unable to create context: {err:?}");
                });
            }
            Commands::Invite {
                context_id,
                inviter_id,
                invitee_id,
            } => {
                if let Some(invitation_payload) = node
                    .ctx_manager
                    .invite_to_context(context_id, inviter_id, invitee_id)
                    .await?
                {
                    println!("{ind} Invited {invitee_id} to context {context_id}");
                    println!("{ind} Invitation Payload: {invitation_payload}");
                } else {
                    println!("{ind} Unable to invite {invitee_id} to context {context_id}");
                }
            }
            Commands::Delete { context_id } => {
                let _ = node.ctx_manager.delete_context(&context_id).await?;
                println!("{ind} Deleted context {context_id}");
            }
            Commands::UpdateProxy {
                context_id,
                public_key,
            } => {
                node.ctx_manager
                    .update_context_proxy(context_id, public_key)
                    .await?;
                println!("{ind} Updated proxy for context {context_id}");
            }
        }
        Ok(())
    }
}
