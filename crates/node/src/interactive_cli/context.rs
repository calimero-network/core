use calimero_context_primitives::client::ContextClient;
use calimero_node_primitives::client::NodeClient;
use calimero_primitives::alias::Alias;
use calimero_primitives::application::ApplicationId;
use calimero_primitives::context::{ContextId, ContextInvitationPayload};
use calimero_primitives::hash::Hash;
use calimero_primitives::identity::{PrivateKey, PublicKey};
use clap::{Parser, Subcommand, ValueEnum};
use eyre::{OptionExt, Result as EyreResult};
use futures_util::TryStreamExt;
use owo_colors::OwoColorize;
use serde_json::Value;

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
    pub async fn run(self, node_client: &NodeClient, ctx_client: &ContextClient) -> EyreResult<()> {
        let ind = ">>".blue();
        match self.command {
            Commands::List => {
                println!(
                    "{ind} {c1:44} | {c2:44} | {c3:44} | Protocol",
                    c1 = "Context ID",
                    c2 = "Application ID",
                    c3 = "Root Hash"
                );

                let contexts = ctx_client.get_contexts(None).await;
                let mut contexts = Box::pin(contexts);

                while let Some(context_id) = contexts.try_next().await? {
                    let Some(context) = ctx_client.get_context(&context_id)? else {
                        continue;
                    };
                    
                    let Some(config) = ctx_client.context_config(&context_id)? else {
                        continue;
                    };
                    
                    let entry = format!(
                        "{c1:44} | {c2:44} | {c3:44} | {c4:8}",
                        c1 = context.id,
                        c2 = context.application_id,
                        c3 = context.root_hash,
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
                let response = ctx_client
                    .join_context(private_key, invitation_payload)
                    .await?;

                println!(
                    "{ind} Joined context {} as {}, waiting for catchup to complete...",
                    response.context_id, response.member_public_key
                );
            }
            Commands::Leave { context } => {
                let context_id = node_client
                    .resolve_alias(context, None)?
                    .ok_or_eyre("unable to resolve")?;
                let is_deleted = ctx_client.delete_context(&context_id).await?;
                if is_deleted.deleted {
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
                let application_id = node_client
                    .resolve_alias(application, None)?
                    .ok_or_eyre("unable to resolve")?;

                match ctx_client
                    .create_context(
                        protocol.as_str().to_string(),
                        &application_id,
                        None,
                        params
                            .as_ref()
                            .map(serde_json::to_vec)
                            .transpose()?
                            .unwrap_or_default(),
                        context_seed.map(Into::into),
                    )
                    .await
                {
                    Ok(response) => {
                        println!(
                            "{ind} Created context {} with identity {}",
                            response.context_id, response.identity
                        );
                    }
                    Err(err) => {
                        println!("{ind} Unable to create context: {err:?}");
                    }
                }
            }
            Commands::Invite {
                context,
                inviter,
                invitee_id,
            } => {
                let context_id = node_client
                    .resolve_alias(context, None)?
                    .ok_or_eyre("unable to resolve")?;
                let inviter_id = node_client
                    .resolve_alias(inviter, Some(context_id))?
                    .ok_or_eyre("unable to resolve")?;

                if let Some(invitation_payload) = ctx_client
                    .invite_member(&context_id, &inviter_id, &invitee_id)
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
                let context_id = node_client
                    .resolve_alias(context, None)?
                    .ok_or_eyre("unable to resolve")?;

                ctx_client.delete_context(&context_id).await?;
                println!("{ind} Deleted context {context_id}");
            }
            Commands::UpdateProxy { context, identity } => {
                let context_id = node_client
                    .resolve_alias(context, None)?
                    .ok_or_eyre("unable to resolve")?;
                let public_key = node_client
                    .resolve_alias(identity, Some(context_id))?
                    .ok_or_eyre("unable to resolve")?;

                let Some(external_config) = ctx_client.context_config(&context_id)? else {
                    println!("{ind} Context configuration not found for {context_id}");
                    return Ok(());
                };

                let external_client = ctx_client.external_client(&context_id, &external_config)?;

                external_client
                    .config()
                    .update_proxy_contract(&public_key)
                    .await?;
                println!("{ind} Updated proxy for context {context_id}");
            }
            Commands::Alias { command } => {
                handle_alias_command(&node_client, command, &ind.to_string())?
            }
        }
        Ok(())
    }
}

fn handle_alias_command(
    node_client: &NodeClient,
    command: AliasCommands,
    ind: &str,
) -> EyreResult<()> {
    match command {
        AliasCommands::Add { alias, context_id } => {
            node_client.create_alias(alias, None, context_id)?;
            println!("{ind} Successfully created alias '{}'", alias.cyan());
        }
        AliasCommands::Remove { context: alias } => {
            node_client.delete_alias(alias, None)?;
            println!("{ind} Successfully removed alias '{}'", alias.cyan());
        }
        AliasCommands::Get { context: alias } => {
            let Some(context) = node_client.lookup_alias(alias, None)? else {
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

            for (alias, context, _scope) in node_client.list_aliases::<ContextId>(None)? {
                println!(
                    "{ind} {}",
                    format_args!("{c1:44} | {c2}", c1 = context.cyan(), c2 = alias.cyan())
                );
            }
        }
    }

    Ok(())
}
