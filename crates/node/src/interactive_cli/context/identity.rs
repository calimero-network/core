use calimero_context_config::types::Capability as ConfigCapability;
use calimero_primitives::alias::Alias;
use calimero_primitives::context::ContextId;
use calimero_primitives::identity::PublicKey;
use calimero_store::key::ContextIdentity as ContextIdentityKey;
use clap::{Parser, Subcommand, ValueEnum};
use eyre::{OptionExt, Result as EyreResult, WrapErr};
use owo_colors::OwoColorize;

use crate::Node;

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
        grantee: PublicKey,
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
        revokee: PublicKey,
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
        identity: Alias<PublicKey>,
        /// The context that the identity is a member of
        #[arg(long, short)]
        context: Alias<ContextId>,
    },
    #[command(about = "List context identity aliases", alias = "ls")]
    List {
        /// The context whose aliases we're listing
        context: Option<Alias<ContextId>>,
    },
}

impl ContextIdentityCommand {
    pub fn run(self, node: &Node) -> EyreResult<()> {
        let ind = ">>".blue();

        match self.subcommand {
            ContextIdentitySubcommands::List { context } => {
                list_identities(node, Some(context), &ind.to_string())?;
            }
            ContextIdentitySubcommands::Generate => {
                generate_new_identity(node, &ind.to_string());
            }
            ContextIdentitySubcommands::Alias { command } => {
                handle_alias_command(node, command, &ind.to_string())?;
            }

            ContextIdentitySubcommands::Use {
                identity,
                context,
                force,
            } => {
                let context_id = node
                    .ctx_manager
                    .resolve_alias(context, None)?
                    .ok_or_eyre("unable to resolve context")?;

                let identity_id = node
                    .ctx_manager
                    .lookup_alias(identity, Some(context_id))?
                    .ok_or_eyre("unable to resolve identity")?;

                let default_alias: Alias<PublicKey> = "default"
                    .parse()
                    .wrap_err("'default' is a valid alias name")?;

                if let Some(existing_identity) = node
                    .ctx_manager
                    .lookup_alias(default_alias, Some(context_id))?
                {
                    if existing_identity == identity_id {
                        println!(
                            "{} Default identity already set to: {} for context {}",
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
                    node.ctx_manager
                        .delete_alias(default_alias, Some(context_id))?;
                }

                println!(
                    "{} Default identity set to: {} for context {}",
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
                let context_id = node
                    .ctx_manager
                    .resolve_alias(context, None)?
                    .ok_or_eyre("unable to resolve context")?;

                let granter_id = node
                    .ctx_manager
                    .resolve_alias(granter, Some(context_id))?
                    .ok_or_eyre("unable to resolve granter identity")?;

                drop(node.ctx_manager.grant_permission(
                    context_id,
                    granter_id,
                    grantee,
                    capability.into(),
                ));

                println!("{ind} Permission granted successfully");
            }
            ContextIdentitySubcommands::Revoke {
                context,
                revoker,
                revokee,
                capability,
            } => {
                let context_id = node
                    .ctx_manager
                    .resolve_alias(context, None)?
                    .ok_or_eyre("unable to resolve context")?;

                let revoker_id = node
                    .ctx_manager
                    .resolve_alias(revoker, Some(context_id))?
                    .ok_or_eyre("unable to resolve revoker identity")?;

                drop(node.ctx_manager.revoke_permission(
                    context_id,
                    revoker_id,
                    revokee,
                    capability.into(),
                ));

                println!("{ind} Permission revoked successfully");
            }
        }

        Ok(())
    }
}

fn list_identities(node: &Node, context: Option<Alias<ContextId>>, ind: &str) -> EyreResult<()> {
    let context_id = if let Some(ctx) = context {
        // User specified a context - resolve it
        match node.ctx_manager.resolve_alias(ctx, None)? {
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

        node.ctx_manager
            .lookup_alias(default_alias, None)?
            .ok_or_eyre("unable to resolve default context")?
    };

    let handle = node.store.handle();
    let mut iter = handle.iter::<ContextIdentityKey>()?;

    let first = 'first: {
        let Some(k) = iter
            .seek(ContextIdentityKey::new(context_id, [0; 32].into()))
            .transpose()
        else {
            break 'first None;
        };

        Some((k, iter.read()))
    };

    println!("{ind} {:44} | {}", "Identity", "Owned");

    for (k, v) in first.into_iter().chain(iter.entries()) {
        let (k, v) = (k?, v?);

        if k.context_id() != context_id {
            break;
        }

        let entry = format!(
            "{:44} | {}",
            k.public_key(),
            if v.private_key.is_some() { "Yes" } else { "No" }
        );
        for line in entry.lines() {
            println!("{ind} {}", line.cyan());
        }
    }

    Ok(())
}

fn generate_new_identity(node: &Node, ind: &str) {
    let identity = node.ctx_manager.new_private_key();
    println!("{ind} Private Key: {}", identity.cyan());
    println!("{ind} Public Key: {}", identity.public_key().cyan());
}

fn handle_alias_command(
    node: &Node,
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
            let context_id = node
                .ctx_manager
                .resolve_alias(context, None)?
                .ok_or_eyre("unable to resolve context alias")?;

            if !node
                .ctx_manager
                .has_context_identity(context_id, identity)?
            {
                println!(
                    "{ind} Error: Identity '{}' does not exist in context '{}'.",
                    identity.cyan(),
                    context_id.cyan()
                );
                return Ok(());
            }

            if let Some(existing_identity) =
                node.ctx_manager.lookup_alias(name, Some(context_id))?
            {
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

                node.ctx_manager.delete_alias(name, Some(context_id))?;
            }

            node.ctx_manager
                .create_alias(name, Some(context_id), identity)?;

            println!("{ind} Successfully created alias '{}'", name.cyan());
        }
        ContextIdentityAliasSubcommands::Remove { identity, context } => {
            let context_id = node
                .ctx_manager
                .resolve_alias(context, None)?
                .ok_or_eyre("unable to resolve context alias")?;

            node.ctx_manager.delete_alias(identity, Some(context_id))?;

            println!("{ind} Successfully removed alias '{}'", identity.cyan());
        }
        ContextIdentityAliasSubcommands::Get { identity, context } => {
            let context_id = node
                .ctx_manager
                .resolve_alias(context, None)?
                .ok_or_eyre("unable to resolve context alias")?;

            let Some(identity_id) = node.ctx_manager.lookup_alias(identity, Some(context_id))?
            else {
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
            let context_id = if let Some(ctx) = context {
                node.ctx_manager
                    .resolve_alias(ctx, None)?
                    .ok_or_eyre("unable to resolve context alias")?
            } else {
                let default_alias: Alias<ContextId> =
                    "default".parse().expect("'default' is a valid alias name");
                node.ctx_manager
                    .lookup_alias(default_alias, None)?
                    .ok_or_eyre("unable to resolve default context")?
            };
            for (alias, identity, scope) in node
                .ctx_manager
                .list_aliases::<PublicKey>(Some(context_id))?
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
