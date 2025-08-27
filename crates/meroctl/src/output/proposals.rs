use calimero_server_primitives::admin::{
    GetProposalApproversResponse, GetProposalResponse, GetProposalsResponse,
};
use serde::Serialize;

use super::Report;

// Define ProposalDetailsResponse locally since it's not exported from the admin module
#[derive(Debug, Serialize)]
pub struct ProposalDetailsResponse {
    pub proposal: GetProposalResponse,
    pub approvers: GetProposalApproversResponse,
}

impl Report for ProposalDetailsResponse {
    fn report(&self) {
        println!("Proposal details retrieved");
    }
}

impl Report for GetProposalResponse {
    fn report(&self) {
        println!("Proposal information retrieved");
    }
}

impl Report for GetProposalApproversResponse {
    fn report(&self) {
        println!("Proposal approvers information retrieved");
    }
}

impl Report for GetProposalsResponse {
    fn report(&self) {
        println!("Proposals list retrieved");
    }
}
