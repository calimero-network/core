use std::pin::pin;

use calimero_context_config::types::Capability as ConfigCapability;
use calimero_context_primitives::client::ContextClient;
use calimero_node_primitives::client::NodeClient;
use calimero_primitives::alias::Alias;
use calimero_primitives::context::ContextId;
use calimero_primitives::identity::PublicKey;
use clap::{Parser, Subcommand, ValueEnum};
use eyre::{OptionExt, Result as EyreResult, WrapErr};
use futures_util::TryStreamExt;
use owo_colors::OwoColorize;

/// Manage context identities
#[derive(Debug, Parser)]
pub struct ContextIdentityCommand {
    #[command(subcommand)]
    subcommand: ContextIdentitySubcommands,
}

#[derive(Debug, Subcommand)]
enum ContextIdentitySubcommands {
    /// List identities in a context
    #[clap(alias = "ls")]
    List {
        /// The context whose identities we're listing
        #[clap(long, short, default_value = "default")]
        context: Alias<ContextId>,
    },
    /// Generate a new identity keypair
    #[clap(alias = "new")]
    Generate,
    /// Manage identity aliases
    Alias {
        #[command(subcommand)]
        command: ContextIdentityAliasSubcommands,
    },
    /// Set default identity for a context
    Use {
        /// The identity to set as default
        identity: Alias<PublicKey>,
        /// The context to set the default identity for
        #[arg(long, short, default_value = "default")]
        context: Alias<ContextId>,

        #[arg(
            long,
            short,
            help = "Force overwrite if default alias already points elsewhere"
        )]
        force: bool,
    },
    #[command(about = "Grant permissions to a member")]
    Grant {
        #[arg(long, short, default_value = "default")]
        context: Alias<ContextId>,
        #[arg(long = "as", default_value = "default")]
        granter: Alias<PublicKey>,
        #[arg(help = "The member to grant permissions to")]
        grantee: Alias<PublicKey>,
        #[arg(help = "The capability to grant")]
        capability: Capability,
    },
    #[command(about = "Revoke permissions from a member")]
    Revoke {
        #[arg(long, short, default_value = "default")]
        context: Alias<ContextId>,
        #[arg(long = "as", default_value = "default")]
        revoker: Alias<PublicKey>,
        #[arg(help = "The member to revoke permissions from")]
        revokee: Alias<PublicKey>,
        #[arg(help = "The capability to revoke")]
        capability: Capability,
    },
}

#[derive(Debug, Clone, ValueEnum, Copy)]
#[clap(rename_all = "PascalCase")]
pub enum Capability {
    ManageApplication,
    ManageMembers,
    Proxy,
}

impl From<Capability> for ConfigCapability {
    fn from(value: Capability) -> Self {
        match value {
            Capability::ManageApplication => ConfigCapability::ManageApplication,
            Capability::ManageMembers => ConfigCapability::ManageMembers,
            Capability::Proxy => ConfigCapability::Proxy,
        }
    }
}

#[derive(Debug, Subcommand)]
enum ContextIdentityAliasSubcommands {
    #[command(
        about = "Add new alias for an identity in a context",
        aliases = ["create", "new"],
    )]
    Add {
        /// Name for the alias
        name: Alias<PublicKey>,
        /// The identity to create an alias for
        identity: PublicKey,
        /// The context that the identity is a member of
        #[arg(long, short, default_value = "default")]
        context: Alias<ContextId>,

        /// Force overwrite existing alias
        #[arg(long, short)]
        force: bool,
    },
    #[command(about = "Remove an identity alias from a context", aliases = ["rm", "del", "delete"])]
    Remove {
        /// Name of the alias to remove
        identity: Alias<PublicKey>,
        /// The context that the identity is a member of
        #[arg(long, short)]
        context: Alias<ContextId>,
    },
    #[command(about = "Resolve the alias to a context identity")]
    Get {
        /// Name of the alias to look up
        #[arg(default_value = "default")]
        identity: Alias<PublicKey>,
        /// The context that the identity is a member of
        #[arg(long, short, default_value = "default")]
        context: Alias<ContextId>,
    },
    #[command(about = "List context identity aliases", alias = "ls")]
    List {
        /// The context whose aliases we're listing
        #[arg(long, short, default_value = "default")]
        context: Alias<ContextId>,
    },
}

impl ContextIdentityCommand {
    pub async fn run(self, node_client: &NodeClient, ctx_client: &ContextClient) -> EyreResult<()> {
        let ind = ">>".blue();

        match self.subcommand {
            ContextIdentitySubcommands::List { context } => {
                list_identities(node_client, ctx_client, Some(context), &ind.to_string()).await?;
            }
            ContextIdentitySubcommands::Generate => {
                let identity = ctx_client.new_identity()?;
                println!("{ind} Public Key: {}", identity.cyan());
            }
            ContextIdentitySubcommands::Alias { command } => {
                handle_alias_command(node_client, ctx_client, command, &ind.to_string())?;
            }

            ContextIdentitySubcommands::Use {
                identity,
                context,
                force,
            } => {
                let context_id = node_client
                    .resolve_alias(context, None)?
                    .ok_or_eyre("unable to resolve context")?;

                let identity_id = node_client
                    .resolve_alias(identity, Some(context_id))?
                    .ok_or_eyre("unable to resolve identity")?;

                let default_alias: Alias<PublicKey> = "default"
                    .parse()
                    .wrap_err("'default' is a valid alias name")?;

                if let Some(existing_identity) =
                    node_client.lookup_alias(default_alias, Some(context_id))?
                {
                    if existing_identity == identity_id {
                        println!(
                            "{} Default identity already set to: '{}' for context '{}'",
                            ind,
                            identity.cyan(),
                            context_id.cyan()
                        );
                        return Ok(());
                    }
                    if !force {
                        println!(
                            "{} Error: Default alias already points to '{}'. Use --force to overwrite.",
                            ind,
                            existing_identity.cyan()
                        );
                        return Ok(());
                    }
                    println!(
                        "{} Warning: Overwriting default alias from '{}' to '{}'",
                        ind,
                        existing_identity.cyan(),
                        identity_id.cyan()
                    );
                    node_client.delete_alias(default_alias, Some(context_id))?;
                }

                node_client.create_alias(default_alias, Some(context_id), identity_id)?;

                println!(
                    "{} Default identity set to: '{}' for context '{}'",
                    ind,
                    identity.cyan(),
                    context_id.cyan()
                );
            }

            ContextIdentitySubcommands::Grant {
                context,
                granter,
                grantee,
                capability,
            } => {
                let context_id = node_client
                    .resolve_alias(context, None)?
                    .ok_or_eyre("unable to resolve context")?;

                let granter_id = node_client
                    .resolve_alias(granter, Some(context_id))?
                    .ok_or_eyre("unable to resolve granter identity")?;

                let grantee_id = node_client
                    .resolve_alias(grantee, Some(context_id))?
                    .ok_or_eyre("unable to resolve revokee identity")?;

                let config_client = ctx_client
                    .context_config(&context_id)?
                    .ok_or_else(|| eyre::eyre!("context '{}' does not exist", context_id))?;

                let external_client = ctx_client.external_client(&context_id, &config_client)?;

                external_client
                    .config()
                    .grant(&granter_id, &[(grantee_id, capability.into())])
                    .await?;

                println!("{ind} Permission granted successfully");
            }
            ContextIdentitySubcommands::Revoke {
                context,
                revoker,
                revokee,
                capability,
            } => {
                let context_id = node_client
                    .resolve_alias(context, None)?
                    .ok_or_eyre("unable to resolve context")?;

                let revoker_id = node_client
                    .resolve_alias(revoker, Some(context_id))?
                    .ok_or_eyre("unable to resolve revoker identity")?;

                let revokee_id = node_client
                    .resolve_alias(revokee, Some(context_id))?
                    .ok_or_eyre("unable to resolve revokee identity")?;

                let config_client = ctx_client
                    .context_config(&context_id)?
                    .ok_or_else(|| eyre::eyre!("context '{}' does not exist", context_id))?;

                let external_client = ctx_client.external_client(&context_id, &config_client)?;

                external_client
                    .config()
                    .revoke(&revoker_id, &[(revokee_id, capability.into())])
                    .await?;

                println!("{ind} Permission revoked successfully");
            }
        }

        Ok(())
    }
}

async fn list_identities(
    node_client: &NodeClient,
    ctx_client: &ContextClient,
    context: Option<Alias<ContextId>>,
    ind: &str,
) -> EyreResult<()> {
    let context_id = if let Some(ctx) = context {
        // User specified a context - resolve it
        match node_client.resolve_alias(ctx, None)? {
            Some(id) => id,
            None => {
                println!("Error: Unable to resolve context '{}'. Please verify the context ID exists or setup default context.", ctx.cyan());
                return Ok(());
            }
        }
    } else {
        // No context specified fall back to default
        let default_alias: Alias<ContextId> =
            "default".parse().expect("'default' is a valid alias name");

        node_client
            .lookup_alias(default_alias, None)?
            .ok_or_eyre("unable to resolve default context")?
    };

    println!("{ind} {:44} | {}", "Identity", "Owned");

    let members = ctx_client.context_members(&context_id, None);

    let mut members = pin!(members);

    while let Some((identity, is_owned)) = members.try_next().await? {
        let entry = format!("{:44} | {}", identity, if is_owned { "Yes" } else { "No" });

        for line in entry.lines() {
            println!("{ind} {}", line.cyan());
        }
    }

    Ok(())
}

fn handle_alias_command(
    node_client: &NodeClient,
    ctx_client: &ContextClient,
    command: ContextIdentityAliasSubcommands,
    ind: &str,
) -> EyreResult<()> {
    match command {
        ContextIdentityAliasSubcommands::Add {
            name,
            identity,
            context,

            force,
        } => {
            let context_id = node_client
                .resolve_alias(context, None)?
                .ok_or_eyre("unable to resolve context alias")?;

            if !ctx_client.has_member(&context_id, &identity)? {
                println!(
                    "{ind} Error: Identity '{}' does not exist in context '{}'.",
                    identity.cyan(),
                    context_id.cyan()
                );
                return Ok(());
            }

            if let Some(existing_identity) = node_client.lookup_alias(name, Some(context_id))? {
                if existing_identity == identity {
                    println!(
                        "{ind} Alias '{}' already points to '{}'. Doing nothing.",
                        name.cyan(),
                        identity.cyan()
                    );
                    return Ok(());
                }

                if !force {
                    println!(
                        "{ind} Error: Alias '{}' already exists and points to '{}'. Use --force to overwrite.",
                        name.cyan(),
                        existing_identity.cyan()
                    );
                    return Ok(());
                }

                println!(
                    "{ind} Warning: Overwriting existing alias '{}' from '{}' to '{}'",
                    name.cyan(),
                    existing_identity.cyan(),
                    identity.cyan()
                );

                node_client.delete_alias(name, Some(context_id))?;
            }

            node_client.create_alias(name, Some(context_id), identity)?;

            println!("{ind} Successfully created alias '{}'", name.cyan());
        }
        ContextIdentityAliasSubcommands::Remove { identity, context } => {
            let context_id = node_client
                .resolve_alias(context, None)?
                .ok_or_eyre("unable to resolve context alias")?;

            node_client.delete_alias(identity, Some(context_id))?;

            println!("{ind} Successfully removed alias '{}'", identity.cyan());
        }
        ContextIdentityAliasSubcommands::Get { identity, context } => {
            let context_id = node_client
                .resolve_alias(context, None)?
                .ok_or_eyre("unable to resolve context alias")?;

            let Some(identity_id) = node_client.lookup_alias(identity, Some(context_id))? else {
                println!("{ind} Alias '{}' not found", identity.cyan());

                return Ok(());
            };

            println!(
                "{ind} Alias '{}' resolves to: {}",
                identity.cyan(),
                identity_id.cyan()
            );
        }
        ContextIdentityAliasSubcommands::List { context } => {
            println!(
                "{ind} {c1:44} | {c2:44} | {c3}",
                c1 = "Context ID",
                c2 = "Identity",
                c3 = "Alias",
            );
            let context_id = node_client
                .resolve_alias(context, None)?
                .ok_or_eyre("unable to resolve context alias")?;

            for (alias, identity, scope) in
                node_client.list_aliases::<PublicKey>(Some(context_id))?
            {
                let context = scope.as_ref().map_or("---", |s| s.as_str());
                println!(
                    "{ind} {}",
                    format_args!(
                        "{c1:44} | {c2:44} | {c3}",
                        c1 = context.cyan(),
                        c2 = identity.cyan(),
                        c3 = alias.cyan(),
                    )
                );
            }
        }
    }
    Ok(())
}
