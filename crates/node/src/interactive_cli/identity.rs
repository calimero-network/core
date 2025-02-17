use std::collections::HashMap;

use calimero_primitives::alias::{Alias, Kind};
use calimero_primitives::context::ContextId;
use calimero_store::key::{Alias as AliasKey, ContextIdentity as ContextIdentityKey};
use clap::{Parser, Subcommand};
use eyre::Result as EyreResult;
use owo_colors::OwoColorize;

use super::commons::resolve_identifier;
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
    Ls {
        /// The context ID or alias to list identities in
        context_id: String,
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
        about = "Add new alias for an identity",
        alias = "create",
        alias = "new"
    )]
    Add {
        /// Name for the alias
        name: Alias,
        /// The identity to create an alias for
        identity: String,
        /// Optional context ID (required for identity aliases)
        #[arg(long, short)]
        context: String,
    },
    /// Remove an alias
    #[command(about = "Remove an alias", alias = "delete", alias = "rm")]
    Remove {
        /// Name of the alias to remove
        name: Alias,
        /// Optional context ID (required for identity aliases)
        #[arg(long, short)]
        context: String,
    },
    #[command(about = "Get the hash attached to an alias")]
    Get {
        /// Name of the alias to look up
        name: Alias,
        /// Optional context ID (required for identity aliases)
        #[arg(long, short)]
        context: String,
    },
}

impl IdentityCommand {
    pub fn run(self, node: &Node) -> EyreResult<()> {
        let ind = ">>".blue();

        match self.subcommand {
            IdentitySubcommands::Ls { context_id } => {
                let context_hash = resolve_identifier(node, &context_id, Kind::Context, None)?;
                list_identities(node, &ContextId::from(context_hash), &ind.to_string())?;
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

fn list_identities(node: &Node, context_id: &ContextId, ind: &str) -> EyreResult<()> {
    let handle = node.store.handle();
    let mut iter = handle.iter::<ContextIdentityKey>()?;

    let first = 'first: {
        let Some(k) = iter
            .seek(ContextIdentityKey::new(*context_id, [0; 32].into()))
            .transpose()
        else {
            break 'first None;
        };

        Some((k, iter.read()))
    };

    println!("{ind} {:44} | {:10} | {}", "Identity", "Owned", "Aliases");

    let mut alias_iter = handle.iter::<AliasKey>()?;
    let mut aliases = HashMap::new();

    let first_alias = alias_iter
        .seek(AliasKey::identity(
            *context_id,
            Alias::try_from("".to_owned())?,
        ))
        .transpose();
    for key_result in first_alias.into_iter().chain(alias_iter.keys()) {
        let key = key_result?;
        if key.scope() != context_id.as_ref() {
            break;
        }

        if let Ok(value) = handle.get(&key) {
            if let Some(hash) = value {
                aliases
                    .entry(hash)
                    .or_insert_with(Vec::new)
                    .push(key.alias().to_string());
            }
        }
    }

    for (k, v) in first.into_iter().chain(iter.entries()) {
        let (k, v) = (k?, v?);

        if k.context_id() != *context_id {
            break;
        }

        let alias_list = aliases
            .get(&(*k.public_key()).into())
            .map(|aliases| aliases.join(", "))
            .unwrap_or_default();

        let entry = format!(
            "{:44} | {:10} | {}",
            k.public_key(),
            if v.private_key.is_some() { "Yes" } else { "No" },
            alias_list,
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
            let context_id = resolve_identifier(node, &context, Kind::Context, None)?.into();

            let identity_hash =
                resolve_identifier(node, &identity, Kind::Identity, Some(context_id))?;

            let mut store = node.store.handle();
            let key = AliasKey::identity(context_id, name.clone());

            store.put(&key, &identity_hash)?;
            println!("{ind} Successfully created alias '{}'", name.cyan());
        }
        AliasSubcommands::Remove { name, context } => {
            let context_id = resolve_identifier(node, &context, Kind::Context, None)?.into();

            let mut store = node.store.handle();
            let key = AliasKey::identity(context_id, name.clone());

            if store.delete(&key).is_ok() {
                println!("{ind} Successfully removed alias '{}'", name.cyan());
            } else {
                println!("{ind} Alias '{}' not found", name.cyan());
            }
        }
        AliasSubcommands::Get { name, context } => {
            let context_id = resolve_identifier(node, &context, Kind::Context, None)?.into();

            let store = node.store.handle();
            let key = AliasKey::identity(context_id, name.clone());

            match store.get(&key)? {
                Some(hash) => {
                    println!(
                        "{ind} Alias '{}' resolves to: {}",
                        name.cyan(),
                        hash.to_string().cyan()
                    );
                }
                None => {
                    println!("{ind} Alias '{}' not found", name.cyan());
                }
            }
        }
    }
    Ok(())
}
