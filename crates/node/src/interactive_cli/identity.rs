use calimero_primitives::alias::Alias;
use calimero_primitives::context::ContextId;
use calimero_primitives::identity::PublicKey;
use calimero_store::key::ContextIdentity as ContextIdentityKey;
use clap::{Parser, Subcommand};
use eyre::{OptionExt, Result as EyreResult};
use owo_colors::OwoColorize;

use crate::Node;

/// Manage identities
#[derive(Debug, Parser)]
pub struct IdentityCommand {
    #[command(subcommand)]
    subcommand: IdentitySubcommands,
}

#[derive(Debug, Subcommand)]
enum IdentitySubcommands {
    /// List identities in a context
    #[clap(alias = "ls")]
    List {
        /// The context whose identities we're listing (omit to use default context)
        #[clap(long = "context", short = 'c')]
        context: Option<Alias<ContextId>>,
    },
    /// Create a new identity
    New,
    /// Manage identity aliases
    Alias {
        #[command(subcommand)]
        command: AliasSubcommands,
    },
}

#[derive(Debug, Subcommand)]
enum AliasSubcommands {
    #[command(
        about = "Add new alias for an identity in a context",
        aliases = ["create", "new"],
    )]
    Add {
        /// Name for the alias
        name: Alias<PublicKey>,
        /// The identity to create an alias for
        identity: PublicKey,
        /// The context that the identity is a member of (omit to use default context)
        #[clap(long = "context", short = 'c')]
        context: Option<Alias<ContextId>>,
    },
    /// Remove an alias
    #[command(
        about = "Remove an identity alias from a context",
        aliases = ["rm", "del", "delete"],
    )]
    Remove {
        /// Name of the alias to remove
        identity: Alias<PublicKey>,
        /// The context that the identity is a member of (omit to use default context)
        #[clap(long = "context", short = 'c')]
        context: Option<Alias<ContextId>>,
    },
    #[command(about = "Resolve the alias to a context identity")]
    Get {
        /// Name of the alias to look up
        identity: Alias<PublicKey>,
        /// The context that the identity is a member of (omit to use default context)
        #[clap(long = "context", short = 'c')]
        context: Option<Alias<ContextId>>,
    },
    #[command(about = "List context identity aliases", alias = "ls")]
    List {
        /// The context whose aliases we're listing (omit to use default context)
        #[clap(long = "context", short = 'c')]
        context: Option<Alias<ContextId>>,
    },
}

impl IdentityCommand {
    pub fn run(self, node: &Node) -> EyreResult<()> {
        let ind = ">>".blue();

        match self.subcommand {
            IdentitySubcommands::List { context } => {
                list_identities(node, context, &ind.to_string())?;
            }
            IdentitySubcommands::New => {
                create_new_identity(node, &ind.to_string());
            }
            IdentitySubcommands::Alias { command } => {
                handle_alias_command(node, command, &ind.to_string())?;
            }
        }

        Ok(())
    }
}

fn list_identities(node: &Node, context: Option<Alias<ContextId>>, ind: &str) -> EyreResult<()> {
    let context_id = resolve_context_id(node, context)?;

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
            if v.private_key.is_some() { "Yes" } else { "No" },
        );
        for line in entry.lines() {
            println!("{ind} {}", line.cyan());
        }
    }

    Ok(())
}

fn create_new_identity(node: &Node, ind: &str) {
    let identity = node.ctx_manager.new_private_key();
    println!("{ind} Private Key: {}", identity.cyan());
    println!("{ind} Public Key: {}", identity.public_key().cyan());
}

fn handle_alias_command(node: &Node, command: AliasSubcommands, ind: &str) -> EyreResult<()> {
    match command {
        AliasSubcommands::Add {
            name,
            identity,
            context,
        } => {
            let context_id = resolve_context_id(node, context)?;

            node.ctx_manager
                .create_alias(name, Some(context_id), identity)?;

            println!("{ind} Successfully created alias '{}'", name.cyan());
        }
        AliasSubcommands::Remove { identity, context } => {
            let context_id = resolve_context_id(node, context)?;

            node.ctx_manager.delete_alias(identity, Some(context_id))?;

            println!("{ind} Successfully removed alias '{}'", identity.cyan());
        }
        AliasSubcommands::Get { identity, context } => {
            let context_id = resolve_context_id(node, context)?;

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
        AliasSubcommands::List { context } => {
            println!(
                "{ind} {c1:44} | {c2:44} | {c3}",
                c1 = "Context ID",
                c2 = "Identity",
                c3 = "Alias",
            );

            // Get context_id from specified alias or default
            let context_id = if let Some(ctx) = context {
                node.ctx_manager
                    .resolve_alias(ctx, None)?
            } else {
                let default_alias: Alias<ContextId> = "default".parse()
                    .expect("'default' is a valid alias name");
                
                node.ctx_manager
                    .lookup_alias(default_alias, None)?
            };

            for (alias, identity, scope) in
                node.ctx_manager.list_aliases::<PublicKey>(context_id)?
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

// Helper function to resolve context from alias or use default
fn resolve_context_id(
    node: &Node, 
    context: Option<Alias<ContextId>>
) -> EyreResult<ContextId> {
    if let Some(alias) = context {
        // If context is provided, resolve it
        node.ctx_manager
            .resolve_alias(alias, None)?
            .ok_or_eyre("Unable to resolve context alias")
    } else {
        // Otherwise, use the default alias
        let default_alias: Alias<ContextId> = "default".parse()
            .expect("'default' is a valid alias name");
            
        node.ctx_manager
            .lookup_alias(default_alias, None)?
            .ok_or_eyre("No default context set. Please set one with 'context use <context-id>' or specify a context explicitly")
    }
}
