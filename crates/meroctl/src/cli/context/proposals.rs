use calimero_context_config::Proposal;
use calimero_primitives::alias::Alias;
use calimero_primitives::context::ContextId;
use calimero_primitives::hash::Hash;
use calimero_primitives::identity::PublicKey;
use calimero_server_primitives::admin::{ApproveProposalRequest, CreateAndApproveProposalRequest};
use clap::{Parser, Subcommand};
use eyre::{OptionExt, Result};
use std::fs::File;

use crate::cli::Environment;
use crate::output::ProposalDetailsResponse;

#[derive(Clone, Parser, Debug)]
#[command(about = "Manage proposals within a context")]
pub struct ProposalsCommand {
    #[command(subcommand)]
    pub command: ProposalsSubcommand,
}

#[derive(Clone, Debug, Subcommand)]
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
            default_value = "1"
        )]
        limit: usize,
    },
    #[command(about = "Create a proposal and immediately approve it")]
    CreateAndApprove {
        /// Signer public key (hex)
        #[arg(long, help = "Signer public key in hex")]
        signer: PublicKey,

        /// Path to a JSON file containing the proposal (calimero_context_config::Proposal)
        #[arg(long, help = "Path to proposal JSON file")]
        proposal_file: String,

        /// Context the proposal belongs to
        #[arg(long, short, default_value = "default")]
        context: Alias<ContextId>,
    },
    #[command(about = "Approve an existing proposal")]
    Approve {
        /// Signer public key (hex)
        #[arg(long, help = "Signer public key in hex")]
        signer: PublicKey,

        /// Proposal ID to approve
        #[arg(long, help = "Proposal ID to approve")]
        proposal_id: Hash,

        /// Context the proposal belongs to
        #[arg(long, short, default_value = "default")]
        context: Alias<ContextId>,
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
            ProposalsSubcommand::CreateAndApprove {
                signer,
                proposal_file,
                context,
            } => {
                let client = environment.client()?;

                let context_id = client
                    .resolve_alias(context, None)
                    .await?
                    .value()
                    .copied()
                    .ok_or_eyre("unable to resolve")?;

                let file = File::open(&proposal_file)?;
                let proposal: Proposal = serde_json::from_reader(file)?;

                let req = CreateAndApproveProposalRequest {
                    signer_id: signer,
                    proposal,
                };

                let resp = client.create_and_approve_proposal(&context_id, req).await?;
                // Print JSON since Report is not implemented for this response
                println!(
                    "{}",
                    serde_json::to_string_pretty(&resp)
                        .unwrap_or_else(|_| "<serialize-error>".to_owned())
                );
            }
            ProposalsSubcommand::Approve {
                signer,
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

                use calimero_context_config::repr::ReprBytes;
                let id_bytes: [u8; 32] = (*proposal_id).into();
                let proposal_id: calimero_context_config::types::ProposalId =
                    ReprBytes::from_bytes(|buf: &mut [u8; 32]| {
                        *buf = id_bytes;
                        Ok(buf.as_ref().len())
                    })
                    .map_err(|e| eyre::eyre!("failed to construct proposal id: {}", e))?;

                let req = ApproveProposalRequest {
                    signer_id: signer,
                    proposal_id: proposal_id,
                };

                let resp = client.approve_proposal(&context_id, req).await?;
                // Print JSON since Report is not implemented for this response
                println!(
                    "{}",
                    serde_json::to_string_pretty(&resp)
                        .unwrap_or_else(|_| "<serialize-error>".to_owned())
                );
            }
        }

        Ok(())
    }
}
