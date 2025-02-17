use calimero_primitives::alias::{Alias, Kind};
use calimero_primitives::application::ApplicationId;
use calimero_primitives::context::{ContextId, ContextInvitationPayload};
use calimero_primitives::hash::Hash;
use calimero_primitives::identity::{PrivateKey, PublicKey};
use calimero_store::key::{
    Alias as AliasKey, ContextConfig as ContextConfigKey, ContextMeta as ContextMetaKey,
};
use clap::{Parser, Subcommand, ValueEnum};
use eyre::Result as EyreResult;
use owo_colors::OwoColorize;
use serde_json::Value;
use tokio::sync::oneshot;

use crate::interactive_cli::commons::resolve_identifier;
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
        /// The protocol to use for the context - possible values: near|starknet|icp|stellar
        #[clap(long, value_enum)]
        protocol: Protocol,
    },
    /// Invite a user to a context
    Invite {
        /// The context ID or alias to invite the user to
        context_id: String,
        /// The ID or alias of the inviter
        inviter_id: String,
        /// The ID or alias of the invitee
        invitee_id: String,
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
        /// The context ID or alias to leave
        context_id: String,
    },
    /// Delete a context
    Delete {
        /// The context ID or alias to delete
        context_id: String,
    },
    /// Update the proxy for a context
    UpdateProxy {
        /// The context ID or alias to update the proxy for
        context_id: String,
        /// The identity or alias requesting the update
        public_key: String,
    },
    /// Manage context aliases
    Alias {
        #[command(subcommand)]
        command: AliasCommands,
    },
}

#[derive(Debug, Subcommand)]
enum AliasCommands {
    #[command(about = "Add new alias for a context", alias = "create", alias = "new")]
    Add {
        /// Name for the alias
        name: Alias,
        /// The context to create an alias for
        context_id: ContextId,
    },
    #[command(about = "Remove a context alias", alias = "delete", alias = "rm")]
    Remove {
        /// Name of the alias to remove
        name: Alias,
    },
    #[command(about = "Get the hash attached to a context alias")]
    Get {
        /// Name of the alias to look up
        name: Alias,
    },
}

impl ContextCommand {
    #[expect(clippy::similar_names, reason = "Acceptable here")]
    #[expect(clippy::too_many_lines, reason = "TODO: Will be refactored")]
    pub async fn run(self, node: &Node) -> EyreResult<()> {
        let ind = ">>".blue();

        match self.command {
            Commands::Ls => {
                println!(
                    "{ind} {c1:44} | {c2:44} | {c3:44} | Protocol | Aliases",
                    c1 = "Context ID",
                    c2 = "Application ID",
                    c3 = "Root Hash"
                );

                let handle = node.store.handle();

                for (k, v) in handle.iter::<ContextMetaKey>()?.entries() {
                    let (k, v) = (k?, v?);
                    let context_id = k.context_id();

                    // Get aliases for this context
                    let mut alias_iter = handle.iter::<AliasKey>()?;
                    let mut aliases = Vec::new();

                    for key_result in alias_iter.keys() {
                        let key = key_result?;
                        if let Ok(value) = handle.get(&key) {
                            if let Some(hash) = value {
                                if hash == context_id.into() {
                                    aliases.push(key.alias().as_str().to_owned());
                                }
                            }
                        }
                    }

                    // Get the config for this context
                    let protocol = handle
                        .get(&ContextConfigKey::new(context_id))?
                        .expect("Context config must exist with protocol")
                        .protocol;

                    let alias_list = if aliases.is_empty() {
                        "None".to_owned()
                    } else {
                        aliases.join(", ")
                    };

                    let entry = format!(
                        "{c1:44} | {c2:44} | {c3:44} | {c4:8} | {c5}",
                        c1 = context_id,
                        c2 = v.application.application_id(),
                        c3 = Hash::from(v.root_hash),
                        c4 = protocol,
                        c5 = alias_list
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
                let resolved_context =
                    ContextId::from(resolve_identifier(node, &context_id, Kind::Context, None)?);
                if node.ctx_manager.delete_context(&resolved_context).await? {
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
                context_id,
                inviter_id,
                invitee_id,
            } => {
                let resolved_context =
                    ContextId::from(resolve_identifier(node, &context_id, Kind::Context, None)?);
                let resolved_inviter = PublicKey::from(resolve_identifier(
                    node,
                    &inviter_id,
                    Kind::Identity,
                    Some(resolved_context),
                )?);
                let resolved_invitee = PublicKey::from(resolve_identifier(
                    node,
                    &invitee_id,
                    Kind::Identity,
                    Some(resolved_context),
                )?);

                if let Some(invitation_payload) = node
                    .ctx_manager
                    .invite_to_context(resolved_context, resolved_inviter, resolved_invitee)
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
            Commands::Delete { context_id } => {
                let resolved_context =
                    ContextId::from(resolve_identifier(node, &context_id, Kind::Context, None)?);
                let _ = node.ctx_manager.delete_context(&resolved_context).await?;
                println!("{ind} Deleted context {context_id}");
            }
            Commands::UpdateProxy {
                context_id,
                public_key,
            } => {
                let resolved_context =
                    ContextId::from(resolve_identifier(node, &context_id, Kind::Context, None)?);
                let resolved_public_key = PublicKey::from(resolve_identifier(
                    node,
                    &public_key,
                    Kind::Identity,
                    Some(resolved_context),
                )?);

                node.ctx_manager
                    .update_context_proxy(resolved_context, resolved_public_key)
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
        AliasCommands::Add { name, context_id } => {
            let mut store = node.store.handle();
            let key = AliasKey::context(name.clone());

            store.put(&key, &context_id.into())?;
            println!("{ind} Successfully created alias '{}'", name.cyan());
        }
        AliasCommands::Remove { name } => {
            let mut store = node.store.handle();
            let key = AliasKey::context(name.clone());

            if store.delete(&key).is_ok() {
                println!("{ind} Successfully removed alias '{}'", name.cyan());
            } else {
                println!("{ind} Alias '{}' not found", name.cyan());
            }
        }
        AliasCommands::Get { name } => {
            let store = node.store.handle();
            let key = AliasKey::context(name.clone());

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
