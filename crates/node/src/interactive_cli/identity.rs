use calimero_context_primitives::client::ContextClient;
use calimero_node_primitives::client::NodeClient;
use calimero_primitives::alias::Alias;
use calimero_primitives::context::ContextId;
use calimero_primitives::identity::PublicKey;
use clap::{Parser, Subcommand};
use eyre::{OptionExt, Result as EyreResult};
use futures_util::TryStreamExt;
use owo_colors::OwoColorize;

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
        /// The context whose identities we're listing
        context: Alias<ContextId>,
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
        /// The context that the identity is a member of
        #[arg(long, short)]
        context: Alias<ContextId>,
    },
    /// Remove an alias
    #[command(
        about = "Remove an identity alias from a context",
        aliases = ["rm", "del", "delete"],
    )]
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

impl IdentityCommand {
    pub async fn run(self, node_client: &NodeClient, ctx_client: &ContextClient) -> EyreResult<()> {
        let ind = ">>".blue();

        match self.subcommand {
            IdentitySubcommands::List { context } => {
                list_identities(node_client, ctx_client, context, &ind.to_string()).await?;
            }
            IdentitySubcommands::New => {
                create_new_identity(ctx_client, &ind.to_string());
            }
            IdentitySubcommands::Alias { command } => {
                handle_alias_command(node_client, command, &ind.to_string())?;
            }
        }

        Ok(())
    }
}

async fn list_identities(
    node_client: &NodeClient,
    ctx_client: &ContextClient,
    context: Alias<ContextId>,
    ind: &str,
) -> EyreResult<()> {
    let context_id = node_client
        .resolve_alias(context, None)?
        .ok_or_eyre("unable to resolve")?;

    println!("{ind} {:44} | {}", "Identity", "Owned");

    let stream = ctx_client.context_members(&context_id, None).await;
    let mut stream = Box::pin(stream);
    
    while let Some(result) = stream.try_next().await? {
        let (identity, is_owned) = result;
        let entry = format!("{:44} | {}", identity, if is_owned { "Yes" } else { "No" });
        
        for line in entry.lines() {
            println!("{ind} {}", line.cyan());
        }
    }

    Ok(())
}

fn create_new_identity(ctx_client: &ContextClient, ind: &str) {
    let identity = ctx_client.new_private_key();
    println!("{ind} Private Key: {}", identity.cyan());
    println!("{ind} Public Key: {}", identity.public_key().cyan());
}

fn handle_alias_command(
    node_client: &NodeClient,
    command: AliasSubcommands,
    ind: &str,
) -> EyreResult<()> {
    match command {
        AliasSubcommands::Add {
            name,
            identity,
            context,
        } => {
            let context_id = node_client
                .resolve_alias(context, None)?
                .ok_or_eyre("unable to resolve")?;

            node_client.create_alias(name, Some(context_id), identity)?;

            println!("{ind} Successfully created alias '{}'", name.cyan());
        }
        AliasSubcommands::Remove { identity, context } => {
            let context_id = node_client
                .resolve_alias(context, None)?
                .ok_or_eyre("unable to resolve")?;

            node_client.delete_alias(identity, Some(context_id))?;

            println!("{ind} Successfully removed alias '{}'", identity.cyan());
        }
        AliasSubcommands::Get { identity, context } => {
            let context_id = node_client
                .resolve_alias(context, None)?
                .ok_or_eyre("unable to resolve")?;

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
        AliasSubcommands::List { context } => {
            println!(
                "{ind} {c1:44} | {c2:44} | {c3}",
                c1 = "Context ID",
                c2 = "Identity",
                c3 = "Alias",
            );

            let context_id = context
                .map(|context| node_client.resolve_alias(context, None))
                .transpose()?
                .flatten();

            for (alias, identity, scope) in node_client.list_aliases::<PublicKey>(context_id)? {
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
