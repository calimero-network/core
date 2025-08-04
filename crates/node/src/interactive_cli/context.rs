use std::pin::pin;

use calimero_context_config::repr::{ReprBytes, ReprTransmute};
use calimero_context_primitives::client::ContextClient;
use calimero_node_primitives::client::NodeClient;
use calimero_primitives::alias::Alias;
use calimero_primitives::application::ApplicationId;
use calimero_primitives::context::{ContextId, ContextInvitationPayload};
use calimero_primitives::hash::Hash;
use calimero_primitives::identity::PublicKey;
use camino::Utf8PathBuf;
use clap::{Parser, Subcommand, ValueEnum};
use eyre::{OptionExt, Result as EyreResult};
use futures_util::TryStreamExt;
use owo_colors::OwoColorize;
use serde_json::Value;

use crate::interactive_cli::common::pretty_alias;

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
        #[clap(long = "as", default_value = "default")]
        inviter: Alias<PublicKey>,
        /// The name for the invitee
        #[clap(long)]
        name: Option<Alias<PublicKey>>,
        /// The identity being invited
        invitee_id: PublicKey,
    },
    /// Join a context
    Join {
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
    /// Manage proposals
    Proposals {
        #[command(subcommand)]
        command: ProposalsCommands,
    },
}

#[derive(Debug, Subcommand)]
enum ProposalsCommands {
    #[command(about = "List all proposals in a context", alias = "ls")]
    List {
        /// Context to list proposals for
        #[clap(long, short, default_value = "default")]
        context: Alias<ContextId>,

        /// Offset for pagination
        #[clap(
            long,
            help = "Starting position for pagination (skip this many proposals)",
            default_value_t
        )]
        offset: usize,

        /// Limit for pagination
        #[clap(
            long,
            help = "Maximum number of proposals to display in results",
            default_value = "20"
        )]
        limit: usize,
    },
    #[command(about = "View details of a specific proposal including approvers and actions")]
    View {
        /// Proposal ID to view
        #[clap(help = "ID of the proposal to view")]
        proposal_id: Hash,

        /// Context the proposal belongs to
        #[clap(long, short, default_value = "default")]
        context: Alias<ContextId>,
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
        #[clap(default_value = "default")]
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

                let contexts = ctx_client.get_contexts(None);

                let mut contexts = pin!(contexts);

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
                invitation_payload,
                context,
                identity,
            } => {
                let response = ctx_client.join_context(invitation_payload).await?;

                if let Some(context) = context {
                    if let Err(e) = node_client.create_alias(context, None, response.context_id) {
                        eprintln!(
                            "{} Failed to create alias '{}' for '{}': {e}",
                            ind,
                            context.cyan(),
                            response.context_id.cyan(),
                        );
                    } else {
                        println!(
                            "{} Created context alias '{}' for '{}'",
                            ind,
                            context.cyan(),
                            response.context_id.cyan(),
                        );
                    }
                }

                if let Some(identity) = identity {
                    if let Err(e) = node_client.create_alias(
                        identity,
                        Some(response.context_id),
                        response.member_public_key,
                    ) {
                        eprintln!(
                            "{} Failed to create alias '{}' for '{}' in '{}': {e}",
                            ind,
                            identity.cyan(),
                            response.member_public_key.cyan(),
                            pretty_alias(context, &response.context_id),
                        );
                    } else {
                        println!(
                            "{} Created identity alias '{}' for '{}' in context '{}'",
                            ind,
                            identity.cyan(),
                            response.member_public_key.cyan(),
                            pretty_alias(context, &response.context_id)
                        );
                    }
                }

                println!(
                    "{ind} Joined context '{}' as '{}', syncing state...",
                    response.context_id.cyan(),
                    response.member_public_key.cyan()
                );
            }

            Commands::Leave { context } => {
                let context_id = node_client
                    .resolve_alias(context, None)?
                    .ok_or_eyre("unable to resolve context")?;
                let is_deleted = ctx_client.delete_context(&context_id).await?;
                if is_deleted.deleted {
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
                let application_id = node_client
                    .resolve_alias(application, None)?
                    .ok_or_eyre("unable to resolve")?;

                match ctx_client
                    .create_context(
                        protocol.as_str().to_owned(),
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
                        if let Some(name_alias) = context {
                            if let Err(e) =
                                node_client.create_alias(name_alias, None, response.context_id)
                            {
                                eprintln!("{} Failed to create context alias: {e}", ind);
                            }
                        }

                        if let Some(identity_alias) = author {
                            if let Err(e) = node_client.create_alias(
                                identity_alias,
                                Some(response.context_id),
                                response.identity,
                            ) {
                                eprintln!("{} Failed to create identity alias: {e}", ind);
                            }
                        }

                        println!(
                            "{ind} Created context '{}' with identity '{}'",
                            response.context_id.cyan(),
                            response.identity.cyan()
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
                name,
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
                    if let Some(alias) = name {
                        node_client.create_alias(alias, Some(context_id), invitee_id)?;
                    }
                    println!(
                        "{ind} Invited '{}' to '{}'",
                        pretty_alias(name, &invitee_id),
                        pretty_alias(Some(context), &context_id)
                    );
                    println!("{ind} Invitation Payload: '{}'", invitation_payload.cyan());
                } else {
                    println!(
                        "{ind} Unable to invite '{}' to context '{}'",
                        invitee_id,
                        pretty_alias(Some(context), &context_id)
                    );
                }
            }
            Commands::Delete { context } => {
                let context_id = node_client
                    .resolve_alias(context, None)?
                    .ok_or_eyre("unable to resolve")?;

                let result = ctx_client.delete_context(&context_id).await?;

                if !result.deleted {
                    println!(
                        "{ind} Unable to delete context {}",
                        pretty_alias(Some(context), &context_id)
                    );
                    return Ok(());
                }

                println!(
                    "{ind} Deleted context {}",
                    pretty_alias(Some(context), &context_id)
                );
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
                let context_id = node_client
                    .resolve_alias(context, None)?
                    .ok_or_eyre("unable to resolve context")?;
                let public_key = node_client
                    .resolve_alias(identity, Some(context_id))?
                    .ok_or_eyre("unable to resolve identity")?;

                match (application_id, path) {
                    // Update with application ID
                    (Some(app_id), None) => {
                        ctx_client
                            .update_application(&context_id, &app_id, &public_key)
                            .await?;
                        println!(
                            "{ind} Updated application for context '{}'",
                            pretty_alias(Some(context), &context_id)
                        );
                    }

                    // Install application from path
                    (None, Some(app_path)) => {
                        let metadata_bytes = metadata.map(String::into_bytes).unwrap_or_default();

                        let application_id = node_client
                            .install_application_from_path(app_path, metadata_bytes)
                            .await?;

                        println!("{ind} Installed application: {}", application_id);

                        ctx_client
                            .update_application(&context_id, &application_id, &public_key)
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
            Commands::Alias { command } => {
                handle_alias_command(&node_client, &ctx_client, command, &ind.to_string())?
            }
            Commands::Use { context, force } => {
                let default_alias: Alias<ContextId> =
                    "default".parse().expect("'default' is a valid alias name");

                let context_id = node_client
                    .resolve_alias(context, None)?
                    .ok_or_eyre("unable to resolve context")?;

                if let Some(existing_context) = node_client.lookup_alias(default_alias, None)? {
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

                    node_client.delete_alias(default_alias, None)?;
                }

                node_client.create_alias(default_alias, None, context_id)?;

                if context.as_str() != context_id.as_str() {
                    println!(
                        "{} Default context set to '{}' (from alias '{}')",
                        ind,
                        context_id.cyan(),
                        context.cyan()
                    );
                } else {
                    println!("{} Default context set to '{}'", ind, context_id.cyan());
                }
            }
            Commands::Identity(identity) => identity.run(node_client, ctx_client).await?,
            Commands::Proposals { command } => {
                handle_proposals_command(node_client, ctx_client, command, &ind.to_string()).await?
            }
        }
        Ok(())
    }
}

fn handle_alias_command(
    node_client: &NodeClient,
    context_client: &ContextClient,
    command: AliasCommands,
    ind: &str,
) -> EyreResult<()> {
    match command {
        AliasCommands::Add {
            alias,
            context_id,
            force,
        } => {
            if !context_client.has_context(&context_id)? {
                println!(
                    "{ind} Error: Context with ID '{}' does not exist.",
                    context_id.cyan()
                );
                return Ok(());
            }

            if let Some(existing_context) = node_client.lookup_alias(alias, None)? {
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

                node_client.delete_alias(alias, None)?;
            }

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
                "{ind} Alias '{}' resolves to: '{}'",
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

async fn handle_proposals_command(
    node_client: &NodeClient,
    ctx_client: &ContextClient,
    command: ProposalsCommands,
    ind: &str,
) -> EyreResult<()> {
    match command {
        ProposalsCommands::List {
            context,
            offset,
            limit,
        } => {
            let context_id = node_client
                .resolve_alias(context, None)?
                .ok_or_eyre("unable to resolve context")?;

            let Some(external_config) = ctx_client.context_config(&context_id)? else {
                println!("{ind} Context configuration not found for {context_id}");
                return Ok(());
            };

            let external_client = ctx_client.external_client(&context_id, &external_config)?;
            let proxy_client = external_client.proxy();

            let proposals = proxy_client.get_proposals(offset, limit).await?;

            if proposals.is_empty() {
                println!("{ind} No proposals found for context '{}'", context_id);
            } else {
                println!("{ind} Proposals for context '{}':", context_id);
                for proposal in proposals {
                    println!(
                        "{ind} - Proposal ID: {}, Author: {}",
                        Hash::from(proposal.id.as_bytes()).cyan(),
                        Hash::from(proposal.author_id.as_bytes()).cyan()
                    );
                }
            }
        }

        ProposalsCommands::View {
            proposal_id,
            context,
        } => {
            let context_id = node_client
                .resolve_alias(context, None)?
                .ok_or_eyre("unable to resolve context")?;

            let Some(external_config) = ctx_client.context_config(&context_id)? else {
                println!("{ind} Context configuration not found for {context_id}");
                return Ok(());
            };

            let external_client = ctx_client.external_client(&context_id, &external_config)?;
            let proxy_client = external_client.proxy();

            let proposal_id = proposal_id.rt()?;

            let proposal = proxy_client.get_proposal(&proposal_id).await?;

            if let Some(proposal) = proposal {
                let approvers = proxy_client.get_proposal_approvers(&proposal_id).await?;
                let approvers_vec: Vec<_> = approvers.into_iter().collect();

                println!("{ind} Proposal ID: {}", format!("{:?}", proposal.id).cyan());
                println!("{ind} Author: {}", proposal.author_id.cyan());
                println!("{ind} Context ID: {}", context_id);

                println!("{ind} Actions: ({} total)", proposal.actions.len());
                for (i, action) in proposal.actions.iter().enumerate() {
                    println!("{ind}   {}. {:?}", i + 1, action);
                }

                println!("{ind} Approvers: ({})", approvers_vec.len());
                if approvers_vec.is_empty() {
                    println!("{ind}   None");
                } else {
                    for approver in approvers_vec {
                        println!("{ind}   {}", format!("{:?}", approver).cyan());
                    }
                }
            } else {
                println!("{ind} Proposal not found");
            }
        }
    }

    Ok(())
}
