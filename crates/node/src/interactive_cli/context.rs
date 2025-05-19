use calimero_primitives::alias::Alias;
use calimero_primitives::application::ApplicationId;
use calimero_primitives::context::{ContextId, ContextInvitationPayload};
use calimero_primitives::hash::Hash;
use calimero_primitives::identity::{PrivateKey, PublicKey};
use calimero_store::key::{ContextConfig as ContextConfigKey, ContextMeta as ContextMetaKey};
use camino::Utf8PathBuf;
use clap::{Parser, Subcommand, ValueEnum};
use eyre::{OptionExt, Result as EyreResult};
use owo_colors::OwoColorize;
use serde_json::Value;
use tokio::sync::oneshot;

use crate::interactive_cli::common::pretty_alias;
use crate::Node;

mod identity;

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
    Ethereum,
}

impl Protocol {
    fn as_str(&self) -> &'static str {
        match self {
            Protocol::Near => "near",
            Protocol::Starknet => "starknet",
            Protocol::Icp => "icp",
            Protocol::Stellar => "stellar",
            Protocol::Ethereum => "ethereum",
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
        /// Name alias for the new context
        #[clap(long = "name")]
        context: Option<Alias<ContextId>>,
        /// Identity alias for the creator
        #[clap(long = "as")]
        author: Option<Alias<PublicKey>>,
        /// The initialization parameters for the context
        params: Option<Value>,
        /// The seed for the context (to derive a deterministic context ID)
        #[clap(long = "seed")]
        context_seed: Option<Hash>,
        /// The protocol to use for the context
        #[clap(long, value_enum)]
        protocol: Protocol,
    },
    /// Invite a user to a context
    Invite {
        /// The context to invite the user to
        #[clap(long, short, default_value = "default")]
        context: Alias<ContextId>,
        /// The identity inviting the other
        #[clap(long = "as")]
        inviter: Alias<PublicKey>,
        /// The name for the invitee
        #[clap(long)]
        name: Option<Alias<PublicKey>>,
        /// The identity being invited
        invitee_id: PublicKey,
    },
    /// Join a context
    Join {
        /// The private key of the user
        private_key: PrivateKey,
        /// The invitation payload from the inviter
        invitation_payload: ContextInvitationPayload,
        /// Alias for the newly joined context
        #[clap(long = "name")]
        context: Option<Alias<ContextId>>,
        /// Alias for the newly joined identity
        #[clap(long = "as")]
        identity: Option<Alias<PublicKey>>,
    },
    /// Leave a context
    Leave {
        /// The context to leave
        #[clap(long, short)]
        context: Alias<ContextId>,
    },
    /// Delete a context
    Delete {
        /// The context to delete
        #[clap(long, short)]
        context: Alias<ContextId>,
    },
    /// Update the proxy for a context
    UpdateProxy {
        /// The context to update the proxy for
        #[clap(long, short, default_value = "default")]
        context: Alias<ContextId>,
        #[clap(long = "as")]
        /// The identity requesting the update
        identity: Alias<PublicKey>,
    },
    Update {
        /// The context to update
        #[clap(long, short, default_value = "default")]
        context: Alias<ContextId>,

        /// The application ID to update in the context
        #[clap(long, short = 'a', conflicts_with = "path")]
        application_id: Option<ApplicationId>,

        /// Path to the application file to install locally
        #[clap(long, conflicts_with = "application_id")]
        path: Option<Utf8PathBuf>,

        /// Metadata needed for the application installation
        #[clap(long, conflicts_with = "application_id")]
        metadata: Option<String>,

        /// The identity requesting the update
        #[clap(long = "as", default_value = "default")]
        identity: Alias<PublicKey>,
    },
    /// Manage context aliases
    Alias {
        #[command(subcommand)]
        command: AliasCommands,
    },
    /// Set a context as the default context
    Use {
        /// The context to set as default
        context: Alias<ContextId>,

        /// Force overwrite if default alias already exists
        #[clap(
            long,
            short,
            help = "Force overwrite if default alias already points elsewhere"
        )]
        force: bool,
    },
    /// Manage context identities
    Identity(identity::ContextIdentityCommand),
}

#[derive(Debug, Subcommand)]
enum AliasCommands {
    #[command(about = "Add new alias for a context", aliases = ["new", "create"])]
    Add {
        /// Name for the alias
        alias: Alias<ContextId>,
        /// The context to create an alias for
        context_id: ContextId,
        /// Force overwrite
        #[clap(long, short)]
        force: bool,
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
                context,
                identity,
            } => {
                let response = node
                    .ctx_manager
                    .join_context(private_key, invitation_payload)
                    .await?;

                if let Some((context_id, public_key)) = response {
                    // Create context alias if --name is specified
                    if let Some(context) = context.as_ref() {
                        if let Err(e) = node.ctx_manager.create_alias(*context, None, context_id) {
                            eprintln!(
                                "{} Failed to create alias '{}' for '{}': {e}",
                                ind,
                                context.cyan(),
                                context_id.cyan(),
                            );
                        } else {
                            println!(
                                "{} Created context alias '{}' for '{}'",
                                ind,
                                context.cyan(),
                                context_id.cyan(),
                            );
                        }
                    }
                    if let Some(identity) = identity.as_ref() {
                        if let Err(e) =
                            node.ctx_manager
                                .create_alias(*identity, Some(context_id), public_key)
                        {
                            eprintln!(
                                "{} Failed to create alias '{}' for '{}' in '{}': {e}",
                                ind,
                                identity.cyan(),
                                public_key.cyan(),
                                pretty_alias(context, &context_id),
                            );
                        } else {
                            println!(
                                "{} Created identity alias '{}' for '{}' in context '{}'",
                                ind,
                                identity.cyan(),
                                public_key.cyan(),
                                pretty_alias(context, &context_id)
                            );
                        }
                    }

                    println!(
                        "{} Joined context '{}' as '{}'",
                        ind,
                        pretty_alias(context, &context_id),
                        pretty_alias(identity, &public_key)
                    );
                } else {
                    println!(
                        "{} Unable to join context at this time, a catchup is in progress.",
                        ind
                    );
                }
            }

            Commands::Leave { context } => {
                let context_id = node
                    .ctx_manager
                    .resolve_alias(context, None)?
                    .ok_or_eyre("unable to resolve context")?;
                if node.ctx_manager.delete_context(&context_id).await? {
                    println!(
                        "{ind} Successfully deleted context '{}'",
                        pretty_alias(Some(context), &context_id)
                    );
                } else {
                    println!(
                        "{ind} Failed to delete context '{}'",
                        pretty_alias(Some(context), &context_id)
                    );
                }
                println!(
                    "{ind} Left context '{}'",
                    pretty_alias(Some(context), &context_id)
                );
            }
            Commands::Create {
                application,
                params,
                context_seed,
                protocol,
                context,
                author,
            } => {
                let application_id = node
                    .ctx_manager
                    .resolve_alias(application, None)?
                    .ok_or_eyre("unable to resolve")?;
                let ctx_manager = node.ctx_manager.clone();

                let (tx, rx) = oneshot::channel();

                node.ctx_manager.create_context(
                    protocol.as_str().to_owned(),
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
                            // Create context alias if --name provided
                            if let Some(name_alias) = context {
                                if let Err(e) =
                                    ctx_manager.create_alias(name_alias, None, context_id)
                                {
                                    eprintln!("{} Failed to create context alias: {e}", ind);
                                }
                            }
                            // Handle identity alias creation
                            if let Some(identity_alias) = author {
                                if let Err(e) = ctx_manager.create_alias(
                                    identity_alias,
                                    Some(context_id),
                                    identity,
                                ) {
                                    eprintln!("{} Failed to create identity alias: {e}", ind);
                                }
                            }

                            println!(
                                "{} Created context '{}' as '{}'",
                                ind,
                                pretty_alias(context, &context_id),
                                pretty_alias(author, &identity)
                            );
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
                name,
                invitee_id,
            } => {
                let context_id = node
                    .ctx_manager
                    .resolve_alias(context, None)?
                    .ok_or_eyre("unable to resolve context")?;
                let inviter_id = node
                    .ctx_manager
                    .resolve_alias(inviter, Some(context_id))?
                    .ok_or_eyre("unable to resolve")?;

                if let Some(invitation_payload) = node
                    .ctx_manager
                    .invite_to_context(context_id, inviter_id, invitee_id)
                    .await?
                {
                    if let Some(alias) = name {
                        node.ctx_manager
                            .create_alias(alias, Some(context_id), invitee_id)?;
                    }
                    println!(
                        "{ind} Invited '{}' to '{}'",
                        pretty_alias(name, &invitee_id),
                        pretty_alias(Some(context), &context_id)
                    );
                    println!("{ind} Invitation Payload: {invitation_payload}");
                } else {
                    println!(
                        "{ind} Unable to invite '{}' to context '{}'",
                        invitee_id,
                        pretty_alias(Some(context), &context_id)
                    );
                }
            }
            Commands::Delete { context } => {
                let context_id = node
                    .ctx_manager
                    .resolve_alias(context, None)?
                    .ok_or_eyre("unable to resolve context")?;
                let _ = node.ctx_manager.delete_context(&context_id).await?;
                println!(
                    "{ind} Deleted context '{}'",
                    pretty_alias(Some(context), &context_id)
                );
            }
            Commands::UpdateProxy { context, identity } => {
                let context_id = node
                    .ctx_manager
                    .resolve_alias(context, None)?
                    .ok_or_eyre("unable to resolve context")?;
                let public_key = node
                    .ctx_manager
                    .resolve_alias(identity, Some(context_id))?
                    .ok_or_eyre("unable to resolve")?;

                node.ctx_manager
                    .update_context_proxy(context_id, public_key)
                    .await?;
                println!(
                    "{ind} Updated proxy for context '{}'",
                    pretty_alias(Some(context), &context_id)
                );
            }
            Commands::Update {
                context,
                application_id,
                path,
                metadata,
                identity,
            } => {
                let context_id = node
                    .ctx_manager
                    .resolve_alias(context, None)?
                    .ok_or_eyre("unable to resolve context")?;
                let public_key = node
                    .ctx_manager
                    .resolve_alias(identity, Some(context_id))?
                    .ok_or_eyre("unable to resolve identity")?;

                match (application_id, path) {
                    // Update with application ID
                    (Some(app_id), None) => {
                        node.ctx_manager
                            .update_application_id(context_id, app_id, public_key)
                            .await?;
                        println!(
                            "{ind} Updated application for context '{}'",
                            pretty_alias(Some(context), &context_id)
                        );
                    }

                    // Install application from path
                    (None, Some(app_path)) => {
                        let metadata_bytes = metadata.map(String::into_bytes).unwrap_or_default();

                        let application_id = node
                            .ctx_manager
                            .install_application_from_path(app_path, metadata_bytes)
                            .await?;

                        println!("{ind} Installed application: {}", application_id);

                        node.ctx_manager
                            .update_application_id(context_id, application_id, public_key)
                            .await?;

                        println!(
                            "{ind} Installed and updated application for context '{}'",
                            pretty_alias(Some(context), &context_id)
                        );
                    }

                    _ => {
                        return Err(eyre::eyre!(
                            "Either application_id or path must be provided"
                        ));
                    }
                }
            }
            Commands::Alias { command } => handle_alias_command(node, command, &ind.to_string())?,
            Commands::Use { context, force } => {
                let default_alias: Alias<ContextId> =
                    "default".parse().expect("'default' is a valid alias name");

                let context_id = node
                    .ctx_manager
                    .resolve_alias(context, None)?
                    .ok_or_eyre("unable to resolve context")?;

                if let Some(existing_context) =
                    node.ctx_manager.lookup_alias(default_alias, None)?
                {
                    if existing_context == context_id {
                        println!(
                            "{} Default alias already points to '{}'. Doing nothing.",
                            ind,
                            context_id.cyan()
                        );
                        return Ok(());
                    }

                    if !force {
                        println!(
                            "{} Error: Default alias already points to '{}'. Use --force to overwrite.",
                            ind,
                            existing_context.cyan()
                        );
                        return Ok(());
                    }

                    println!(
                        "{} Warning: Overwriting default alias from '{}' to '{}'",
                        ind,
                        existing_context.cyan(),
                        context_id.cyan()
                    );

                    node.ctx_manager.delete_alias(default_alias, None)?;
                }

                node.ctx_manager
                    .create_alias(default_alias, None, context_id)?;

                if context.as_str() != context_id.as_str() {
                    println!(
                        "{} Default context set to: {} (from alias '{}')",
                        ind, context_id, context
                    );
                } else {
                    println!("{} Default context set to: {}", ind, context_id);
                }
            }
            Commands::Identity(identity) => identity.run(node)?,
        }
        Ok(())
    }
}

fn handle_alias_command(node: &Node, command: AliasCommands, ind: &str) -> EyreResult<()> {
    match command {
        AliasCommands::Add {
            alias,
            context_id,
            force,
        } => {
            let handle = node.store.handle();

            if !handle.has(&ContextMetaKey::new(context_id))? {
                println!(
                    "{ind} Error: Context with ID '{}' does not exist.",
                    context_id.cyan()
                );
                return Ok(());
            }

            if let Some(existing_context) = node.ctx_manager.lookup_alias(alias, None)? {
                if existing_context == context_id {
                    println!(
                        "{ind} Alias '{}' already points to '{}'. Doing nothing.",
                        alias.cyan(),
                        context_id.cyan()
                    );
                    return Ok(());
                }

                if !force {
                    println!(
                        "{ind} Error: Alias '{}' already exists and points to '{}'. Use --force to overwrite.",
                        alias.cyan(),
                        existing_context.to_string().cyan()
                    );
                    return Ok(());
                }

                println!(
                    "{ind} Warning: Overwriting existing alias '{}' from '{}' to '{}'",
                    alias.cyan(),
                    existing_context.to_string().cyan(),
                    context_id.cyan()
                );

                node.ctx_manager.delete_alias(alias, None)?;
            }

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
                "{ind} Alias '{}' resolves to: '{}'",
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
