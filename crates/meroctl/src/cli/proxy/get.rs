use calimero_server::admin::handlers::proposals::{
    GetNumberOfActiveProposalsResponse, GetNumberOfProposalApprovalsResponse,
    GetProposalApproversResponse, GetProposalResponse, GetProposalsResponse,
};
use clap::{Parser, ValueEnum};
use eyre::Result as EyreResult;
use libp2p::identity::Keypair;
use libp2p::Multiaddr;
use reqwest::Client;
use serde::de::DeserializeOwned;
use serde::Serialize;

use crate::cli::Environment;
use crate::common::{do_request, fetch_multiaddr, load_config, multiaddr_to_url, RequestType};
use crate::output::Report;

#[derive(Parser, Debug)]
#[command(about = "Fetch details about the proxy contract")]
pub struct GetCommand {
    #[arg(value_name = "METHOD", help = "Method to fetch details", value_enum)]
    pub method: GetRequest,

    #[arg(value_name = "CONTEXT_ID", help = "context_id of the context")]
    pub context_id: String,

    #[arg(value_name = "PROPOSAL_ID", help = "proposal_id of the proposal")]
    pub proposal_id: String,
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
        println!("{}", self.data);
    }
}

impl Report for GetNumberOfProposalApprovalsResponse {
    fn report(&self) {
        println!("{}", self.data);
    }
}

impl Report for GetProposalApproversResponse {
    fn report(&self) {
        for user in &self.data {
            println!("{}", user.identity_public_key);
        }
    }
}

impl Report for GetProposalsResponse {
    fn report(&self) {
        for proposal in &self.data {
            println!("{:#?}", proposal.report());
        }
    }
}

impl Report for GetProposalResponse {
    fn report(&self) {
        println!("{:#?}", self.data.report());
    }
}

impl GetCommand {
    pub async fn run(&self, environment: &Environment) -> EyreResult<()> {
        let config = load_config(&environment.args.home, &environment.args.node_name)?;
        let multiaddr = fetch_multiaddr(&config)?;
        let client = Client::new();

        match &self.method {
            GetRequest::NumProposalApprovals => {
                self.get_number_of_proposal_approvals(
                    environment,
                    multiaddr,
                    &client,
                    &config.identity,
                )
                .await
            }
            GetRequest::NumActiveProposals => {
                self.get_number_of_active_proposals(
                    environment,
                    multiaddr,
                    &client,
                    &config.identity,
                )
                .await
            }
            GetRequest::Proposal => {
                self.get_proposal(environment, multiaddr, &client, &config.identity)
                    .await
            }
            GetRequest::Proposals => {
                self.get_proposals(environment, multiaddr, &client, &config.identity)
                    .await
            }
            GetRequest::ProposalApprovers => {
                self.get_proposal_approvers(environment, multiaddr, &client, &config.identity)
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
    ) -> EyreResult<()> {
        let url = multiaddr_to_url(
            multiaddr,
            &format!(
                "admin-api/dev/contexts/{}/proposals/{}/approvals/count",
                self.context_id, self.proposal_id
            ),
        )?;
        self.make_request::<GetNumberOfProposalApprovalsResponse>(environment, client, url, keypair)
            .await
    }

    async fn get_number_of_active_proposals(
        &self,
        environment: &Environment,
        multiaddr: &Multiaddr,
        client: &Client,
        keypair: &Keypair,
    ) -> EyreResult<()> {
        let url = multiaddr_to_url(
            multiaddr,
            &format!("admin-api/dev/contexts/{}/proposals/count", self.context_id),
        )?;
        self.make_request::<GetNumberOfActiveProposalsResponse>(environment, client, url, keypair)
            .await
    }

    async fn get_proposal_approvers(
        &self,
        environment: &Environment,
        multiaddr: &Multiaddr,
        client: &Client,
        keypair: &Keypair,
    ) -> EyreResult<()> {
        let url = multiaddr_to_url(
            multiaddr,
            &format!(
                "admin-api/dev/contexts/{}/proposals/{}/approvals/users",
                self.context_id, self.proposal_id
            ),
        )?;
        self.make_request::<GetProposalApproversResponse>(environment, client, url, keypair)
            .await
    }

    async fn get_proposals(
        &self,
        environment: &Environment,
        multiaddr: &Multiaddr,
        client: &Client,
        keypair: &Keypair,
    ) -> EyreResult<()> {
        let url = multiaddr_to_url(
            multiaddr,
            &format!("admin-api/dev/contexts/{}/proposals", self.context_id),
        )?;
        self.make_request::<GetProposalsResponse>(environment, client, url, keypair)
            .await
    }

    async fn get_proposal(
        &self,
        environment: &Environment,
        multiaddr: &Multiaddr,
        client: &Client,
        keypair: &Keypair,
    ) -> EyreResult<()> {
        let url = multiaddr_to_url(
            multiaddr,
            &format!(
                "admin-api/dev/contexts/{}/proposals/{}",
                self.context_id, self.proposal_id
            ),
        )?;
        self.make_request::<GetProposalResponse>(environment, client, url, keypair)
            .await
    }

    async fn make_request<O>(
        &self,
        environment: &Environment,
        client: &Client,
        url: reqwest::Url,
        keypair: &Keypair,
    ) -> EyreResult<()>
    where
        O: DeserializeOwned + Report + Serialize,
    {
        let response =
            do_request::<(), O>(client, url, None::<()>, keypair, RequestType::Get).await?;

        environment.output.write(&response);

        Ok(())
    }
}
