use calimero_primitives::alias::Alias;
use calimero_primitives::context::ContextId;
use calimero_primitives::hash::Hash;
use calimero_server_primitives::admin::{
    GetProposalApproversResponse, GetProposalResponse, GetProposalsResponse,
};
use clap::{Parser, Subcommand};
use comfy_table::{Cell, Color, Table};
use eyre::{OptionExt, Result};
use serde::{Deserialize, Serialize};

use crate::cli::Environment;
use crate::common::resolve_alias;
use crate::output::Report;

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProposalDetailsResponse {
    pub proposal: GetProposalResponse,
    pub approvers: GetProposalApproversResponse,
}

impl Report for ProposalDetailsResponse {
    fn report(&self) {
        self.proposal.report();

        println!("\nApprovers:");
        self.approvers.report();
    }
}

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

impl Report for GetProposalResponse {
    fn report(&self) {
        let mut table = Table::new();
        let _ = table.load_preset(comfy_table::presets::UTF8_FULL);
        let _ = table.apply_modifier(comfy_table::modifiers::UTF8_ROUND_CORNERS);

        let _ = table.set_header(vec![Cell::new("Proposal Details").fg(Color::Blue)]);
        let _ = table.add_row(vec![format!("ID: {}", self.data.id)]);
        let _ = table.add_row(vec![format!("Author: {}", self.data.author_id)]);
        println!("{table}");

        let mut actions_table = Table::new();
        let _ = actions_table.load_preset(comfy_table::presets::UTF8_FULL);
        let _ = actions_table.apply_modifier(comfy_table::modifiers::UTF8_ROUND_CORNERS);

        let _ = actions_table.set_header(vec![
            Cell::new("#").fg(Color::Blue),
            Cell::new("Action").fg(Color::Blue),
        ]);

        if self.data.actions.is_empty() {
            let _ = actions_table.add_row(vec!["", "No actions"]);
        } else {
            println!("\nActions: ({} total)", self.data.actions.len());
            for (i, action) in self.data.actions.iter().enumerate() {
                let _ = actions_table.add_row(vec![format!("{}", i + 1), format!("{:?}", action)]);
            }
        }

        println!("\n{actions_table}");
    }
}

impl Report for GetProposalApproversResponse {
    fn report(&self) {
        let mut table = Table::new();
        let _ = table.load_preset(comfy_table::presets::UTF8_FULL);
        let _ = table.apply_modifier(comfy_table::modifiers::UTF8_ROUND_CORNERS);

        let _ = table.set_header(vec![Cell::new("Approver ID").fg(Color::Blue)]);

        if self.data.is_empty() {
            let _ = table.add_row(vec!["No approvers found"]);
        } else {
            for approver in &self.data {
                let _ = table.add_row(vec![format!("{}", approver)]);
            }
        }

        println!("{table}");
    }
}

impl Report for GetProposalsResponse {
    fn report(&self) {
        let mut table = Table::new();
        let _ = table.load_preset(comfy_table::presets::UTF8_FULL);
        let _ = table.apply_modifier(comfy_table::modifiers::UTF8_ROUND_CORNERS);

        let _ = table.set_header(vec![
            Cell::new("ID").fg(Color::Blue),
            Cell::new("Author").fg(Color::Blue),
            Cell::new("Actions").fg(Color::Blue),
        ]);

        if self.data.is_empty() {
            let _ = table.add_row(vec!["No proposals found", "", ""]);
        } else {
            for proposal in &self.data {
                let _ = table.add_row(vec![
                    format!("{}", proposal.id),
                    format!("{}", proposal.author_id),
                    format!("{}", proposal.actions.len()),
                ]);
            }
        }

        println!("{table}");
    }
}

impl ProposalsCommand {
    pub async fn run(&self, environment: &mut Environment) -> Result<()> {
        let connection = environment.connection()?;

        match &self.command {
            ProposalsSubcommand::List {
                context,
                offset,
                limit,
            } => {
                let context_id = resolve_alias(connection, *context, None)
                    .await?
                    .value()
                    .cloned()
                    .ok_or_eyre("unable to resolve context")?;

                let args = serde_json::json!({
                    "offset": offset,
                    "limit": limit
                });

                let mero_client = environment.mero_client()?;
                let response = mero_client.list_proposals(&context_id, args).await?;
                environment.output.write(&response);
                Ok(())
            }
            ProposalsSubcommand::View {
                proposal_id,
                context,
            } => {
                let context_id = resolve_alias(connection, *context, None)
                    .await?
                    .value()
                    .cloned()
                    .ok_or_eyre("unable to resolve context")?;

                let mero_client = environment.mero_client()?;
                let proposal_response = mero_client.get_proposal(&context_id, proposal_id).await?;

                let approvers_response = mero_client
                    .get_proposal_approvers(&context_id, proposal_id)
                    .await?;

                let combined_response = ProposalDetailsResponse {
                    proposal: proposal_response,
                    approvers: approvers_response,
                };

                environment.output.write(&combined_response);
                Ok(())
            }
        }
    }
}
