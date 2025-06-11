use calimero_primitives::alias::Alias;
use calimero_primitives::context::ContextId;
use calimero_primitives::hash::Hash;
use calimero_server_primitives::admin::{
    GetProposalApproversResponse, GetProposalResponse, GetProposalsResponse,
};
use clap::{Parser, Subcommand};
use comfy_table::{Cell, Color, Table};
use eyre::{OptionExt, Result as EyreResult};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::cli::Environment;
use crate::common::resolve_alias;
use crate::connection::ConnectionInfo;
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

#[derive(Parser, Debug)]
#[command(about = "Manage proposals within a context")]
pub struct ProposalsCommand {
    #[command(subcommand)]
    pub command: ProposalsSubcommand,
}

#[derive(Debug, Subcommand)]
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
    pub async fn run(&self, environment: &Environment) -> EyreResult<()> {
        let connection = environment
            .connection
            .as_ref()
            .ok_or_eyre("No connection configured")?;

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

                self.list_proposals(environment, connection, context_id, args)
                    .await
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

                let proposal_response = self
                    .get_proposal(connection, context_id, proposal_id)
                    .await?;

                let approvers_response = self
                    .get_proposal_approvers_data(connection, context_id, proposal_id)
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

    async fn get_proposal_approvers_data(
        &self,
        connection: &ConnectionInfo,
        context_id: ContextId,
        proposal_id: &Hash,
    ) -> EyreResult<GetProposalApproversResponse> {
        let response = connection
            .get(&format!(
                "admin-api/dev/contexts/{}/proposals/{}/approvals/users",
                context_id, proposal_id
            ))
            .await?;

        Ok(response)
    }

    async fn list_proposals(
        &self,
        environment: &Environment,
        connection: &ConnectionInfo,
        context_id: ContextId,
        args: Value,
    ) -> EyreResult<()> {
        let response: GetProposalsResponse = connection
            .post(
                &format!("admin-api/dev/contexts/{}/proposals", context_id),
                args,
            )
            .await?;

        environment.output.write(&response);
        Ok(())
    }

    async fn get_proposal(
        &self,
        connection: &ConnectionInfo,
        context_id: ContextId,
        proposal_id: &Hash,
    ) -> EyreResult<GetProposalResponse> {
        let response = connection
            .get(&format!(
                "admin-api/dev/contexts/{}/proposals/{}",
                context_id, proposal_id
            ))
            .await?;

        Ok(response)
    }
}
