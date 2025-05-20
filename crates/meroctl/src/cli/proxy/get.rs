use calimero_primitives::alias::Alias;
use calimero_primitives::context::ContextId;
use calimero_primitives::hash::Hash;
use calimero_server_primitives::admin::{
    GetNumberOfActiveProposalsResponse, GetNumberOfProposalApprovalsResponse,
    GetProposalApproversResponse, GetProposalResponse, GetProposalsResponse,
};
use clap::{Parser, ValueEnum};
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
#[command(about = "Fetch details about the proxy contract")]
pub struct GetCommand {
    #[arg(value_name = "METHOD", help = "Method to fetch details", value_enum)]
    pub method: GetRequest,

    #[arg(long, short)]
    #[arg(
        value_name = "CONTEXT",
        help = "Context for which to query",
        default_value = "default"
    )]
    pub context: Alias<ContextId>,

    #[arg(value_name = "PROPOSAL_ID", help = "proposal_id of the proposal")]
    pub proposal_id: Option<Hash>,

    #[arg(long, value_parser = serde_value, help = "JSON arguments to pass to the method (e.g., {\"offset\": 0, \"limit\": 10})")]
    pub args: Option<Value>,
}

fn serde_value(s: &str) -> serde_json::Result<Value> {
    serde_json::from_str(s)
}

#[derive(Clone, Debug, ValueEnum)]
pub enum GetRequest {
    NumProposalApprovals,
    NumActiveProposals,
    Proposal,
    Proposals,
    ProposalApprovers,
}

impl Report for GetNumberOfActiveProposalsResponse {
    fn report(&self) {
        let mut table = Table::new();
        let _ = table.set_header(vec![Cell::new("Active Proposals Count").fg(Color::Blue)]);
        let _ = table.add_row(vec![self.data.to_string()]);
        println!("{table}");
    }
}

impl Report for GetNumberOfProposalApprovalsResponse {
    fn report(&self) {
        let mut table = Table::new();
        let _ = table.set_header(vec![Cell::new("Proposal Approvals").fg(Color::Blue)]);
        let _ = table.add_row(vec![format!("Approvals: {:?}", self.data)]);
        println!("{table}");
    }
}

impl Report for GetProposalApproversResponse {
    fn report(&self) {
        let mut table = Table::new();
        let _ = table.set_header(vec![
            Cell::new("Proposal Approvers").fg(Color::Blue),
            Cell::new("Type").fg(Color::Blue),
        ]);

        for user in &self.data {
            let _ = table.add_row(vec![user.to_string(), "ContextIdentity".to_owned()]);
        }
        println!("{table}");
    }
}

impl Report for GetProposalsResponse {
    fn report(&self) {
        let mut table = Table::new();
        let _ = table.set_header(vec![
            Cell::new("Proposals").fg(Color::Blue),
            Cell::new("ID").fg(Color::Blue),
            Cell::new("Status").fg(Color::Blue),
        ]);

        for proposal in &self.data {
            let _ = table.add_row(vec![proposal.id.to_string(), format!("{:?}", proposal)]);
        }
        println!("{table}");
    }
}

impl Report for GetProposalResponse {
    fn report(&self) {
        let mut table = Table::new();
        let _ = table.set_header(vec![Cell::new("Proposal Details").fg(Color::Blue)]);
        let _ = table.add_row(vec![format!("ID: {}", self.data.id)]);
        let _ = table.add_row(vec![format!("Status: {:?}", self.data)]);
        println!("{table}");
    }
}

impl GetCommand {
    pub async fn run(&self, environment: &Environment) -> EyreResult<()> {
        let config = load_config(&environment.args.home, &environment.args.node_name)?;
        let multiaddr = fetch_multiaddr(&config)?;
        let client = Client::new();

        let context_id = resolve_alias(multiaddr, &config.identity, self.context, None)
            .await?
            .value()
            .cloned()
            .ok_or_eyre("unable to resolve")?;

        match &self.method {
            GetRequest::NumProposalApprovals => {
                self.get_number_of_proposal_approvals(
                    environment,
                    multiaddr,
                    &client,
                    &config.identity,
                    context_id,
                )
                .await
            }
            GetRequest::NumActiveProposals => {
                self.get_number_of_active_proposals(
                    environment,
                    multiaddr,
                    &client,
                    &config.identity,
                    context_id,
                )
                .await
            }
            GetRequest::Proposal => {
                self.get_proposal(
                    environment,
                    multiaddr,
                    &client,
                    &config.identity,
                    context_id,
                )
                .await
            }
            GetRequest::Proposals => {
                self.get_proposals(
                    environment,
                    multiaddr,
                    &client,
                    &config.identity,
                    context_id,
                )
                .await
            }
            GetRequest::ProposalApprovers => {
                self.get_proposal_approvers(
                    environment,
                    multiaddr,
                    &client,
                    &config.identity,
                    context_id,
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
    ) -> EyreResult<()> {
        let proposal_id = self.proposal_id.ok_or_eyre("proposal_id is required")?;
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

    async fn get_number_of_active_proposals(
        &self,
        environment: &Environment,
        multiaddr: &Multiaddr,
        client: &Client,
        keypair: &Keypair,
        context_id: ContextId,
    ) -> EyreResult<()> {
        let url = multiaddr_to_url(
            multiaddr,
            &format!("admin-api/dev/contexts/{}/proposals/count", context_id),
        )?;
        make_request::<_, GetNumberOfActiveProposalsResponse>(
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
    ) -> EyreResult<()> {
        let proposal_id = self.proposal_id.ok_or_eyre("proposal_id is required")?;
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

    async fn get_proposals(
        &self,
        environment: &Environment,
        multiaddr: &Multiaddr,
        client: &Client,
        keypair: &Keypair,
        context_id: ContextId,
    ) -> EyreResult<()> {
        let url = multiaddr_to_url(
            multiaddr,
            &format!("admin-api/dev/contexts/{}/proposals", context_id),
        )?;

        let params = self.args.clone().ok_or_eyre("arguments are required")?;

        make_request::<_, GetProposalsResponse>(
            environment,
            client,
            url,
            Some(params),
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
    ) -> EyreResult<()> {
        let proposal_id = self.proposal_id.ok_or_eyre("proposal_id is required")?;
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
