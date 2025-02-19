use calimero_primitives::alias::Alias;
use calimero_primitives::application::ApplicationId;
use calimero_primitives::context::{ContextId, ContextInvitationPayload};
use calimero_primitives::hash::Hash;
use calimero_primitives::identity::{PrivateKey, PublicKey};
use calimero_store::key::{ContextConfig as ContextConfigKey, ContextMeta as ContextMetaKey};
use clap::{Parser, Subcommand, ValueEnum};
use eyre::{OptionExt, Result as EyreResult};
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

#[derive(Debug, Clone, ValueEnum)]
enum Protocol {
    Near,
    Starknet,
    Icp,
    Stellar,
}

impl Protocol {
    fn as_str(&self) -> &'static str {
        match self {
            Protocol::Near => "near",
            Protocol::Starknet => "starknet",
            Protocol::Icp => "icp",
            Protocol::Stellar => "stellar",
        }
    }
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// List contexts
    #[clap(alias = "ls")]
    List,
    /// Create a context
    Create {
        /// The application to create the context with
        application: Alias<ApplicationId>,
        /// The initialization parameters for the context
        params: Option<Value>,
        /// The seed for the context (to derive a deterministic context ID)
        #[clap(long = "seed")]
        context_seed: Option<Hash>,
        /// The protocol to use for the context - possible values: near|starknet|icp|stellar
        #[clap(long, value_enum)]
        protocol: Protocol,
    },
    /// Invite a user to a context
    Invite {
        /// The context to invite the user to
        context: Alias<ContextId>,
        /// The identity inviting the other
        #[clap(long = "as")]
        inviter: Alias<PublicKey>,
        /// The identity being invited
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
        /// The context to leave
        context: Alias<ContextId>,
    },
    /// Delete a context
    Delete {
        /// The context to delete
        context: Alias<ContextId>,
    },
    /// Update the proxy for a context
    UpdateProxy {
        /// The context to update the proxy for
        context: Alias<ContextId>,
        #[clap(long = "as")]
        /// The identity requesting the update
        identity: Alias<PublicKey>,
    },
    /// Manage context aliases
    Alias {
        #[command(subcommand)]
        command: AliasCommands,
    },
}

#[derive(Debug, Subcommand)]
enum AliasCommands {
    #[command(about = "Add new alias for a context", aliases = ["new", "create"])]
    Add {
        /// Name for the alias
        alias: Alias<ContextId>,
        /// The context to create an alias for
        context_id: ContextId,
    },
    #[command(about = "Remove a context alias", aliases = ["rm", "del", "delete"])]
    Remove {
        /// Name of the alias to remove
        context: Alias<ContextId>,
    },
    #[command(about = "Resolve the alias to a context")]
    Get {
        /// Name of the alias to look up
        context: Alias<ContextId>,
    },
    #[command(about = "List all context aliases", alias = "ls")]
    List,
}

impl ContextCommand {
    #[expect(clippy::similar_names, reason = "Acceptable here")]
    #[expect(clippy::too_many_lines, reason = "TODO: Will be refactored")]
    pub async fn run(self, node: &Node) -> EyreResult<()> {
        let ind = ">>".blue();

        match self.command {
            Commands::List => {
                println!(
                    "{ind} {c1:44} | {c2:44} | {c3:44} | Protocol",
                    c1 = "Context ID",
                    c2 = "Application ID",
                    c3 = "Root Hash"
                );

                let handle = node.store.handle();

                for (k, v) in handle.iter::<ContextMetaKey>()?.entries() {
                    let (k, v) = (k?, v?);
                    let context_id = k.context_id();

                    // Get the config for this context
                    let config = handle
                        .get(&ContextConfigKey::new(context_id))?
                        .expect("Context config must exist with protocol");

                    let entry = format!(
                        "{c1:44} | {c2:44} | {c3:44} | {c4:8}",
                        c1 = context_id,
                        c2 = v.application.application_id(),
                        c3 = Hash::from(v.root_hash),
                        c4 = config.protocol,
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
            Commands::Leave { context } => {
                let context_id = node
                    .ctx_manager
                    .resolve_alias(context, None)?
                    .ok_or_eyre("unable to resolve")?;
                if node.ctx_manager.delete_context(&context_id).await? {
                    println!("{ind} Successfully deleted context {context_id}");
                } else {
                    println!("{ind} Failed to delete context {context_id}");
                }
                println!("{ind} Left context {context_id}");
            }
            Commands::Create {
                application,
                params,
                context_seed,
                protocol,
            } => {
                let application_id = node
                    .ctx_manager
                    .resolve_alias(application, None)?
                    .ok_or_eyre("unable to resolve")?;

                let (tx, rx) = oneshot::channel();

                node.ctx_manager.create_context(
                    &protocol.as_str(),
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
                context,
                inviter,
                invitee_id,
            } => {
                let context_id = node
                    .ctx_manager
                    .resolve_alias(context, None)?
                    .ok_or_eyre("unable to resolve")?;
                let inviter_id = node
                    .ctx_manager
                    .resolve_alias(inviter, Some(context_id))?
                    .ok_or_eyre("unable to resolve")?;

                if let Some(invitation_payload) = node
                    .ctx_manager
                    .invite_to_context(context_id, inviter_id, invitee_id)
                    .await?
                {
                    println!("{ind} Invited {} to context {}", invitee_id, context_id);
                    println!("{ind} Invitation Payload: {invitation_payload}");
                } else {
                    println!(
                        "{ind} Unable to invite {} to context {}",
                        invitee_id, context_id
                    );
                }
            }
            Commands::Delete { context } => {
                let context_id = node
                    .ctx_manager
                    .resolve_alias(context, None)?
                    .ok_or_eyre("unable to resolve")?;

                let _ = node.ctx_manager.delete_context(&context_id).await?;
                println!("{ind} Deleted context {context_id}");
            }
            Commands::UpdateProxy { context, identity } => {
                let context_id = node
                    .ctx_manager
                    .resolve_alias(context, None)?
                    .ok_or_eyre("unable to resolve")?;
                let public_key = node
                    .ctx_manager
                    .resolve_alias(identity, Some(context_id))?
                    .ok_or_eyre("unable to resolve")?;

                node.ctx_manager
                    .update_context_proxy(context_id, public_key)
                    .await?;
                println!("{ind} Updated proxy for context {context_id}");
            }
            Commands::Alias { command } => handle_alias_command(node, command, &ind.to_string())?,
        }
        Ok(())
    }
}

fn handle_alias_command(node: &Node, command: AliasCommands, ind: &str) -> EyreResult<()> {
    match command {
        AliasCommands::Add { alias, context_id } => {
            node.ctx_manager.create_alias(alias, None, context_id)?;
            println!("{ind} Successfully created alias '{}'", alias.cyan());
        }
        AliasCommands::Remove { context: alias } => {
            node.ctx_manager.delete_alias(alias, None)?;
            println!("{ind} Successfully removed alias '{}'", alias.cyan());
        }
        AliasCommands::Get { context: alias } => {
            let Some(context) = node.ctx_manager.lookup_alias(alias, None)? else {
                println!("{ind} Alias '{}' not found", alias.cyan());

                return Ok(());
            };

            println!(
                "{ind} Alias '{}' resolves to: {}",
                alias.cyan(),
                context.to_string().cyan()
            );
        }
        AliasCommands::List => {
            println!("{ind} {c1:44} | {c2}", c1 = "Context ID", c2 = "Alias");

            for (alias, context, _scope) in node.ctx_manager.list_aliases::<ContextId>(None)? {
                println!(
                    "{ind} {}",
                    format_args!("{c1:44} | {c2}", c1 = context.cyan(), c2 = alias.cyan())
                );
            }
        }
    }

    Ok(())
}
