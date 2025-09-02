use calimero_server_primitives::admin::{
    GetProposalApproversResponse, GetProposalResponse, GetProposalsResponse,
};
use comfy_table::{Cell, Color, Table};
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
        let mut table = Table::new();
        let _ = table.set_header(vec![
            Cell::new("Proposal Details").fg(Color::Blue),
            Cell::new("Value").fg(Color::Blue),
        ]);

        // Show proposal information
        let proposal = &self.proposal.data;
        let _ = table.add_row(vec!["Proposal ID", &proposal.id.to_string()]);
        let _ = table.add_row(vec!["Author ID", &proposal.author_id.to_string()]);
        let _ = table.add_row(vec!["Actions Count", &proposal.actions.len().to_string()]);

        // Show approvers count
        let approvers_count = self.approvers.data.len();
        let _ = table.add_row(vec!["Approvers Count", &approvers_count.to_string()]);

        println!("{table}");

        // Show detailed approvers if any
        if !self.approvers.data.is_empty() {
            let mut approvers_table = Table::new();
            let _ = approvers_table.set_header(vec![
                Cell::new("Approver ID").fg(Color::Blue),
                Cell::new("Type").fg(Color::Blue),
            ]);

            for approver in &self.approvers.data {
                let _ = approvers_table
                    .add_row(vec![approver.to_string(), "Context Identity".to_owned()]);
            }

            println!("\nApprovers:");
            println!("{approvers_table}");
        }
    }
}

impl Report for GetProposalResponse {
    fn report(&self) {
        let proposal = &self.data;
        let mut table = Table::new();
        let _ = table.set_header(vec![
            Cell::new("Proposal Information").fg(Color::Blue),
            Cell::new("Value").fg(Color::Blue),
        ]);

        let _ = table.add_row(vec!["Proposal ID", &proposal.id.to_string()]);
        let _ = table.add_row(vec!["Author ID", &proposal.author_id.to_string()]);
        let _ = table.add_row(vec!["Actions Count", &proposal.actions.len().to_string()]);

        println!("{table}");
    }
}

impl Report for GetProposalApproversResponse {
    fn report(&self) {
        if self.data.is_empty() {
            println!("No approvers found for this proposal");
        } else {
            let mut table = Table::new();
            let _ = table.set_header(vec![
                Cell::new("Approver ID").fg(Color::Blue),
                Cell::new("Type").fg(Color::Blue),
            ]);

            for approver in &self.data {
                let _ = table.add_row(vec![approver.to_string(), "Context Identity".to_owned()]);
            }

            println!("{table}");
        }
    }
}

impl Report for GetProposalsResponse {
    fn report(&self) {
        if self.data.is_empty() {
            println!("No proposals found");
        } else {
            let mut table = Table::new();
            let _ = table.set_header(vec![
                Cell::new("Proposal ID").fg(Color::Blue),
                Cell::new("Author ID").fg(Color::Blue),
                Cell::new("Actions Count").fg(Color::Blue),
            ]);

            for proposal in &self.data {
                let _ = table.add_row(vec![
                    proposal.id.to_string(),
                    proposal.author_id.to_string(),
                    proposal.actions.len().to_string(),
                ]);
            }

            println!("{table}");
        }
    }
}
