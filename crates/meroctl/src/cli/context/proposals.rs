use calimero_primitives::alias::Alias;
use calimero_primitives::context::ContextId;
use calimero_primitives::hash::Hash;
use clap::{Parser, Subcommand};
use eyre::{OptionExt, Result};

use crate::cli::Environment;
use crate::output::ProposalDetailsResponse;





#[derive(Copy, Clone, Parser, Debug)]
#[command(about = "Manage proposals within a context")]
pub struct ProposalsCommand {
    #[command(subcommand)]
    pub command: ProposalsSubcommand,
}

#[derive(Copy, Clone, Debug, Subcommand)]
pub enum ProposalsSubcommand {
    #[command(about = "List all proposals in a context", alias = "ls")]
    List {
        /// Context to list proposals for
        #[arg(long, short, default_value = "default")]
        context: Alias<ContextId>,

        /// Offset for pagination
        #[arg(
            long,
            help = "Starting position for pagination (skip this many proposals)",
            default_value_t
        )]
        offset: usize,

        /// Limit for pagination
        #[arg(
            long,
            help = "Maximum number of proposals to display in results",
            default_value = "20"
        )]
        limit: usize,
    },
    #[command(about = "View details of a specific proposal including approvers and actions")]
    View {
        /// Proposal ID to view
        #[arg(help = "ID of the proposal to view")]
        proposal_id: Hash,

        /// Context the proposal belongs to
        #[arg(long, short, default_value = "default")]
        context: Alias<ContextId>,
    },
}







impl ProposalsCommand {
    pub async fn run(self, environment: &mut Environment) -> Result<()> {
        match self.command {
            ProposalsSubcommand::List {
                context,
                offset,
                limit,
            } => {
                let client = environment.client()?;

                let context_id = client
                    .resolve_alias(context, None)
                    .await?
                    .value()
                    .copied()
                    .ok_or_eyre("unable to resolve")?;

                let args = serde_json::json!({
                    "offset": offset,
                    "limit": limit
                });
                let response = client.list_proposals(&context_id, args).await?;
                environment.output.write(&response);
            }
            ProposalsSubcommand::View {
                proposal_id,
                context,
            } => {
                let client = environment.client()?;

                let context_id = client
                    .resolve_alias(context, None)
                    .await?
                    .value()
                    .copied()
                    .ok_or_eyre("unable to resolve")?;

                let proposal = client.get_proposal(&context_id, &proposal_id).await?;
                let approvers = client
                    .get_proposal_approvers(&context_id, &proposal_id)
                    .await?;

                let details = ProposalDetailsResponse {
                    proposal,
                    approvers,
                };

                environment.output.write(&details);
            }
        }

        Ok(())
    }
}
