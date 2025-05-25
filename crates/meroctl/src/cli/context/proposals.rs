use calimero_primitives::alias::Alias;
use calimero_primitives::context::ContextId;
use calimero_primitives::hash::Hash;
use calimero_server_primitives::admin::{
    GetNumberOfActiveProposalsResponse, GetNumberOfProposalApprovalsResponse,
    GetProposalApproversResponse, GetProposalResponse, GetProposalsResponse,
};
use clap::{Parser, Subcommand};
use comfy_table::{Cell, Color, Table};
use eyre::{OptionExt, Result as EyreResult};
use libp2p::identity::Keypair;
use libp2p::Multiaddr;
use reqwest::Client;
use serde_json::Value;

use crate::cli::Environment;
use crate::common::{
    fetch_multiaddr, load_config, make_request, multiaddr_to_url, resolve_alias, RequestType,
};
use crate::output::Report;

#[derive(Parser, Debug)]
#[command(about = "Manage proposals within a context")]
pub struct ProposalsCommand {
    #[command(subcommand)]
    pub command: Option<ProposalsSubcommand>,

    /// Context to query
    #[arg(long, short)]
    #[arg(
        value_name = "CONTEXT",
        help = "Context for which to query",
        default_value = "default"
    )]
    pub context: Alias<ContextId>,

    /// Offset for pagination (when listing proposals)
    #[arg(long, help = "Offset for paginated results", default_value = "0")]
    pub offset: usize,

    /// Limit for pagination (when listing proposals)
    #[arg(long, help = "Limit for paginated results", default_value = "20")]
    pub limit: usize,
}

#[derive(Debug, Subcommand)]
pub enum ProposalsSubcommand {
    #[command(about = "View details of a specific proposal including approvers and actions")]
    View {
        /// Proposal ID to view
        #[arg(help = "ID of the proposal to view")]
        proposal_id: Hash,
    },
}

impl Report for GetProposalResponse {
    fn report(&self) {
        let mut table = Table::new();
        table.load_preset(comfy_table::presets::UTF8_FULL);
        table.apply_modifier(comfy_table::modifiers::UTF8_ROUND_CORNERS);

        let _ = table.set_header(vec![Cell::new("Proposal Details").fg(Color::Blue)]);
        let _ = table.add_row(vec![format!("ID: {}", self.data.id)]);
        let _ = table.add_row(vec![format!("Author: {}", self.data.author_id)]);
        println!("{table}");

        let mut actions_table = Table::new();
        actions_table.load_preset(comfy_table::presets::UTF8_FULL);
        actions_table.apply_modifier(comfy_table::modifiers::UTF8_ROUND_CORNERS);

        let _ = actions_table.set_header(vec![
            Cell::new("#").fg(Color::Blue),
            Cell::new("Action").fg(Color::Blue),
        ]);

        if self.data.actions.is_empty() {
            let _ = actions_table.add_row(vec!["", "No actions"]);
        } else {
            for (i, action) in self.data.actions.iter().enumerate() {
                let _ = actions_table.add_row(vec![format!("{}", i + 1), format!("{:?}", action)]);
            }
        }

        println!("\n{actions_table}");
    }
}

impl Report for GetNumberOfProposalApprovalsResponse {
    fn report(&self) {
        let mut table = Table::new();
        table.load_preset(comfy_table::presets::UTF8_FULL);
        table.apply_modifier(comfy_table::modifiers::UTF8_ROUND_CORNERS);

        let _ = table.set_header(vec![Cell::new("Approval Count").fg(Color::Blue)]);
        let _ = table.add_row(vec![format!("Number of Approvals: {}", self.data.num_approvals)]);
        println!("{table}");
    }
}

impl Report for GetProposalApproversResponse {
    fn report(&self) {
        let mut table = Table::new();
        table.load_preset(comfy_table::presets::UTF8_FULL);
        table.apply_modifier(comfy_table::modifiers::UTF8_ROUND_CORNERS);

        let _ = table.set_header(vec![
            Cell::new("Approver ID").fg(Color::Blue),
        ]);

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
        table.load_preset(comfy_table::presets::UTF8_FULL);
        table.apply_modifier(comfy_table::modifiers::UTF8_ROUND_CORNERS);

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
        let config = load_config(&environment.args.home, &environment.args.node_name).await?;
        let multiaddr = fetch_multiaddr(&config)?;
        let client = Client::new();

        let context_id = resolve_alias(multiaddr, &config.identity, self.context, None)
            .await?
            .value()
            .cloned()
            .ok_or_eyre("unable to resolve context")?;

        match &self.command {
            Some(ProposalsSubcommand::View { proposal_id }) => {
                let proposal_result = self
                    .get_proposal(
                        environment,
                        multiaddr,
                        &client,
                        &config.identity,
                        context_id,
                        proposal_id,
                    )
                    .await;

                if let Err(e) = proposal_result {
                    return Err(e);
                }

                let _approvers_result = self
                    .get_proposal_approvers(
                        environment,
                        multiaddr,
                        &client,
                        &config.identity,
                        context_id,
                        proposal_id,
                    )
                    .await;

                let _approval_count_result = self
                    .get_number_of_proposal_approvals(
                        environment,
                        multiaddr,
                        &client,
                        &config.identity,
                        context_id,
                        proposal_id,
                    )
                    .await;

                Ok(())
            }

            None => {
                let args = serde_json::json!({
                    "offset": self.offset,
                    "limit": self.limit
                });

                self.list_proposals(
                    environment,
                    multiaddr,
                    &client,
                    &config.identity,
                    context_id,
                    args,
                )
                .await
            }
        }
    }

     async fn get_number_of_proposal_approvals(
        &self,
        environment: &Environment,
        multiaddr: &Multiaddr,
        client: &Client,
        keypair: &Keypair,
        context_id: ContextId,
        proposal_id: &Hash,
    ) -> EyreResult<()> {
        let url = multiaddr_to_url(
            multiaddr,
            &format!(
                "admin-api/dev/contexts/{}/proposals/{}/approvals/count",
                context_id, proposal_id
            ),
        )?;
        make_request::<_, GetNumberOfProposalApprovalsResponse>(
            environment,
            client,
            url,
            None::<()>,
            keypair,
            RequestType::Get,
        )
        .await
    }

    async fn get_proposal_approvers(
        &self,
        environment: &Environment,
        multiaddr: &Multiaddr,
        client: &Client,
        keypair: &Keypair,
        context_id: ContextId,
        proposal_id: &Hash,
    ) -> EyreResult<()> {
        let url = multiaddr_to_url(
            multiaddr,
            &format!(
                "admin-api/dev/contexts/{}/proposals/{}/approvals/users",
                context_id, proposal_id
            ),
        )?;
        make_request::<_, GetProposalApproversResponse>(
            environment,
            client,
            url,
            None::<()>,
            keypair,
            RequestType::Get,
        )
        .await
    }

    async fn list_proposals(
        &self,
        environment: &Environment,
        multiaddr: &Multiaddr,
        client: &Client,
        keypair: &Keypair,
        context_id: ContextId,
        args: Value,
    ) -> EyreResult<()> {
        let url = multiaddr_to_url(
            multiaddr,
            &format!("admin-api/dev/contexts/{}/proposals", context_id),
        )?;

        make_request::<_, GetProposalsResponse>(
            environment,
            client,
            url,
            Some(args),
            keypair,
            RequestType::Post,
        )
        .await
    }

    async fn get_proposal(
        &self,
        environment: &Environment,
        multiaddr: &Multiaddr,
        client: &Client,
        keypair: &Keypair,
        context_id: ContextId,
        proposal_id: &Hash,
    ) -> EyreResult<()> {
        let url = multiaddr_to_url(
            multiaddr,
            &format!(
                "admin-api/dev/contexts/{}/proposals/{}",
                context_id, proposal_id
            ),
        )?;
        make_request::<_, GetProposalResponse>(
            environment,
            client,
            url,
            None::<()>,
            keypair,
            RequestType::Get,
        )
        .await
    }
}