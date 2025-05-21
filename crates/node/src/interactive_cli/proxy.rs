use calimero_context_config::repr::ReprTransmute; // Add this import
use calimero_primitives::context::ContextId;
use calimero_primitives::hash::Hash;
use clap::{Parser, Subcommand, ValueEnum};
use eyre::{OptionExt, Result as EyreResult};
use owo_colors::OwoColorize;
use serde_json::Value;

use crate::Node;

#[derive(Debug, Parser)]
pub struct ProxyCommand {
    #[command(subcommand)]
    command: ProxySubcommands,
}

#[derive(Debug, Subcommand)]
pub enum ProxySubcommands {
    Get(GetCommand),
}

#[derive(Debug, Parser)]
pub struct GetCommand {
    /// Method to fetch details
    #[arg(value_enum)]
    method: GetRequest,

    /// Context for which to query
    #[arg(long, short, default_value = "default")]
    context: String,

    /// proposal_id of the proposal
    proposal_id: Option<Hash>,

    /// JSON arguments to pass to the method
    #[arg(long)]
    args: Option<String>,
}

#[derive(Clone, Debug, ValueEnum)]
pub enum GetRequest {
    /// Get number of approvals for a proposal
    NumProposalApprovals,
    /// Get count of active proposals
    NumActiveProposals,
    /// Get details of a proposal
    Proposal,
    /// List proposals
    Proposals,
    /// Get approvers of a proposal
    ProposalApprovers,
}

impl ProxyCommand {
    pub async fn run(&self, node: &Node) -> EyreResult<()> {
        let ind = ">>";
        match &self.command {
            ProxySubcommands::Get(cmd) => cmd.run(node, ind).await,
        }
    }
}

impl GetCommand {
    pub async fn run(&self, node: &Node, ind: &str) -> EyreResult<()> {
        let context_id = if self.context == "default" {
            match node.ctx_manager.lookup_alias("default".parse()?, None)? {
                Some(id) => id,
                None => {
                    println!("{} Error: Default context not set", ind);
                    return Ok(());
                }
            }
        } else {
            match self.context.parse::<ContextId>() {
                Ok(id) => id,
                Err(_) => match node
                    .ctx_manager
                    .resolve_alias(self.context.parse()?, None)?
                {
                    Some(id) => id,
                    None => {
                        println!(
                            "{} Error: Unable to resolve context '{}'",
                            ind,
                            self.context.cyan()
                        );
                        return Ok(());
                    }
                },
            }
        };

        match &self.method {
            GetRequest::NumProposalApprovals => {
                self.get_number_of_proposal_approvals(node, ind, &context_id)
                    .await
            }
            GetRequest::NumActiveProposals => {
                self.get_number_of_active_proposals(node, ind, &context_id)
                    .await
            }
            GetRequest::Proposal => self.get_proposal(node, ind, &context_id).await,
            GetRequest::Proposals => self.get_proposals(node, ind, &context_id).await,
            GetRequest::ProposalApprovers => {
                self.get_proposal_approvers(node, ind, &context_id).await
            }
        }
    }

    async fn get_number_of_proposal_approvals(
        &self,
        node: &Node,
        ind: &str,
        context_id: &ContextId,
    ) -> EyreResult<()> {
        let proposal_id = self.proposal_id.ok_or_eyre("proposal_id is required")?;
        let proposal_id = proposal_id.rt()?;

        let approvals = node
            .proposal_manager
            .get_number_of_proposal_approvals(context_id, &proposal_id)
            .await?;

        println!(
            "{} Proposal {:?} has {} approvals",
            ind,
            proposal_id.cyan(),
            approvals.num_approvals
        );
        Ok(())
    }

    async fn get_number_of_active_proposals(
        &self,
        node: &Node,
        ind: &str,
        context_id: &ContextId,
    ) -> EyreResult<()> {
        let count = node
            .proposal_manager
            .get_active_proposals_count(context_id)
            .await?;

        println!("{} Active proposals: {}", ind, count);
        Ok(())
    }

    async fn get_proposal(&self, node: &Node, ind: &str, context_id: &ContextId) -> EyreResult<()> {
        let proposal_id = self.proposal_id.ok_or_eyre("proposal_id is required")?;
        let proposal_id = proposal_id.rt()?;

        let proposal = node
            .proposal_manager
            .get_proposal(context_id, &proposal_id)
            .await?;

        if let Some(proposal) = proposal {
            println!("{} Proposal ID: {}", ind, proposal.id.cyan());
            println!("{} Author: {}", ind, proposal.author_id.cyan());
            println!("{} Actions: {}", ind, proposal.actions.len());
        } else {
            println!(
                "{} Proposal {} not found in context {}",
                ind,
                format!("{:?}", proposal_id).cyan(),
                context_id.cyan()
            );
        }
        Ok(())
    }

    async fn get_proposals(
        &self,
        node: &Node,
        ind: &str,
        context_id: &ContextId,
    ) -> EyreResult<()> {
        let args = if let Some(args_str) = &self.args {
            match serde_json::from_str::<Value>(args_str) {
                Ok(v) => v,
                Err(e) => {
                    println!("{} Error parsing arguments: {}", ind, e);
                    return Ok(());
                }
            }
        } else {
            serde_json::json!({
                "offset": 0,
                "limit": 10
            })
        };

        let offset = args.get("offset").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
        let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(10) as usize;

        let proposals = node
            .proposal_manager
            .get_proposals(context_id, offset, limit)
            .await?;

        if proposals.is_empty() {
            println!("{} No proposals found", ind);
            return Ok(());
        }

        println!("{} Proposals in context {}:", ind, context_id.cyan());
        for proposal in proposals {
            println!(
                "{} ID: {}, Author: {}",
                ind,
                proposal.id.cyan(),
                proposal.author_id.cyan()
            );
        }
        Ok(())
    }

    async fn get_proposal_approvers(
        &self,
        node: &Node,
        ind: &str,
        context_id: &ContextId,
    ) -> EyreResult<()> {
        let proposal_id = self.proposal_id.ok_or_eyre("proposal_id is required")?;
        let proposal_id = proposal_id.rt()?;

        let approvers = node
            .proposal_manager
            .get_proposal_approvers(context_id, &proposal_id)
            .await?;

        if approvers.is_empty() {
            println!(
                "{} No approvers for proposal {:?} in context {}",
                ind,
                proposal_id.cyan(),
                context_id.cyan()
            );
            return Ok(());
        }

        println!("{} Approvers for proposal {:?}:", ind, proposal_id.cyan());
        for approver in approvers {
            println!("{} {}", ind, format!("{:?}", approver).cyan());
        }
        Ok(())
    }
}
