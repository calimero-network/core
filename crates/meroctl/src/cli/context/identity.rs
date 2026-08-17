use calimero_primitives::alias::Alias;
use calimero_primitives::context::ContextId;
use clap::Parser;
use eyre::Result;

use crate::cli::Environment;

pub mod generate;

#[derive(Copy, Clone, Debug, Parser)]
#[command(about = "Manage context identities")]
pub struct ContextIdentityCommand {
    #[command(subcommand)]
    command: ContextIdentitySubcommand,
}

#[derive(Copy, Clone, Debug, Parser)]
pub enum ContextIdentitySubcommand {
    #[command(about = "List identities in a context", alias = "ls")]
    List {
        #[arg(help = "The context whose identities we're listing")]
        #[arg(long, short, default_value = "default")]
        context: Alias<ContextId>,
        #[arg(long, help = "Show only owned identities")]
        owned: bool,
    },
    #[command(about = "Generate a new identity keypair", alias = "new")]
    Generate(generate::GenerateCommand),
}

impl ContextIdentityCommand {
    pub async fn run(self, environment: &mut Environment) -> Result<()> {
        match self.command {
            ContextIdentitySubcommand::List { context, owned } => {
                list_identities(environment, Some(context), owned).await
            }
            ContextIdentitySubcommand::Generate(cmd) => cmd.run(environment).await,
        }
    }
}

async fn list_identities(
    environment: &mut Environment,
    context: Option<Alias<ContextId>>,
    owned: bool,
) -> Result<()> {
    let client = environment.client()?.clone();
    let resolve_response = client
        .resolve_alias(
            context.unwrap_or_else(|| "default".parse().expect("valid alias")),
            None,
        )
        .await?;

    let context_id = match resolve_response.value().cloned() {
        Some(id) => id,
        None => {
            let context_display = context
                .as_ref()
                .map(|alias| alias.to_string())
                .unwrap_or_else(|| "default".to_owned());
            eyre::bail!("Error: Unable to resolve context '{}'. Please verify the context ID exists or setup default context.", context_display)
        }
    };

    let client = environment.client()?;
    let response = client.get_context_identities(&context_id, owned).await?;

    environment.output.write(&response);
    Ok(())
}
