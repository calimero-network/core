use calimero_primitives::alias::Alias;
use calimero_primitives::context::ContextId;
use calimero_primitives::hash::Hash;
use calimero_server_primitives::admin::{
    GetNumberOfActiveProposalsResponse, GetNumberOfProposalApprovalsResponse,
    GetProposalApproversResponse, GetProposalResponse, GetProposalsResponse,
};
use clap::{Parser, ValueEnum};
use eyre::{OptionExt, Result as EyreResult};
use libp2p::identity::Keypair;
use libp2p::Multiaddr;
use reqwest::Client;

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

    #[arg(value_name = "CONTEXT", help = "Context for which to query")]
    pub context: Alias<ContextId>,

    #[arg(value_name = "PROPOSAL_ID", help = "proposal_id of the proposal")]
    pub proposal_id: Hash,
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
        println!("{:?}", self.data);
    }
}

impl Report for GetNumberOfProposalApprovalsResponse {
    fn report(&self) {
        println!("{:?}", self.data);
    }
}

impl Report for GetProposalApproversResponse {
    fn report(&self) {
        for user in &self.data {
            println!("{}", user);
        }
    }
}

impl Report for GetProposalsResponse {
    fn report(&self) {
        for proposal in &self.data {
            println!("{:#?}", proposal);
        }
    }
}

impl Report for GetProposalResponse {
    fn report(&self) {
        println!("{:#?}", self.data);
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
        let url = multiaddr_to_url(
            multiaddr,
            &format!(
                "admin-api/dev/contexts/{}/proposals/{}/approvals/count",
                context_id, self.proposal_id
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
        let url = multiaddr_to_url(
            multiaddr,
            &format!(
                "admin-api/dev/contexts/{}/proposals/{}/approvals/users",
                context_id, self.proposal_id
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
        make_request::<_, GetProposalsResponse>(
            environment,
            client,
            url,
            None::<()>,
            keypair,
            RequestType::Get,
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
        let url = multiaddr_to_url(
            multiaddr,
            &format!(
                "admin-api/dev/contexts/{}/proposals/{}",
                context_id, self.proposal_id
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
