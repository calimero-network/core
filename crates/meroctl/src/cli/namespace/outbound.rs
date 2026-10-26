use calimero_primitives::application::ApplicationId;
use calimero_primitives::outbound::OutboundSelection;
use calimero_server_primitives::admin::SetOutboundApprovalApiRequest;
use clap::{Parser, Subcommand};
use eyre::{bail, Result};

use crate::cli::Environment;

/// `manifest` (every prefix the app declares), `none` (nothing), or a
/// comma-separated list of `https` URL prefixes.
pub fn parse_selection(value: &str) -> Result<OutboundSelection> {
    match value.trim() {
        "manifest" => Ok(OutboundSelection::Manifest),
        "none" => Ok(OutboundSelection::Prefixes(Vec::new())),
        "" => bail!("expected `manifest`, `none` or a comma-separated list of URL prefixes"),
        list => Ok(OutboundSelection::Prefixes(
            list.split(',').map(|p| p.trim().to_owned()).collect(),
        )),
    }
}

#[derive(Debug, Parser)]
#[command(about = "Which URL prefixes the namespace lets each publisher's apps send requests to")]
pub struct OutboundCommand {
    #[command(subcommand)]
    pub subcommand: OutboundSubCommands,
}

#[derive(Debug, Subcommand)]
pub enum OutboundSubCommands {
    /// List what the namespace approves, per publisher.
    #[command(alias = "ls")]
    List(ListCommand),
    /// Set what the namespace approves for an app's publisher, exactly.
    Set(SetCommand),
}

#[derive(Debug, Parser)]
pub struct ListCommand {
    #[clap(name = "NAMESPACE_ID", help = "The hex-encoded namespace ID")]
    pub namespace_id: String,
}

#[derive(Debug, Parser)]
pub struct SetCommand {
    #[clap(name = "NAMESPACE_ID", help = "The hex-encoded namespace ID")]
    pub namespace_id: String,

    #[clap(long, help = "The installed application whose publisher this is for")]
    pub application_id: ApplicationId,

    #[clap(
        long,
        value_parser = parse_selection,
        help = "`manifest`, `none` (revoke everything) or a comma-separated list of URL prefixes"
    )]
    pub approve: OutboundSelection,
}

impl OutboundCommand {
    pub async fn run(self, environment: &mut Environment) -> Result<()> {
        let client = environment.client()?;
        match self.subcommand {
            OutboundSubCommands::List(cmd) => {
                let response = client.list_outbound_approvals(&cmd.namespace_id).await?;
                environment.output.write(&response);
            }
            OutboundSubCommands::Set(cmd) => {
                let response = client
                    .set_outbound_approval(
                        &cmd.namespace_id,
                        SetOutboundApprovalApiRequest {
                            application_id: cmd.application_id,
                            approve: cmd.approve,
                        },
                    )
                    .await?;
                environment.output.write(&response);
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_selection_parses_from_the_flag() {
        assert_eq!(
            parse_selection("manifest").unwrap(),
            OutboundSelection::Manifest
        );
        assert_eq!(
            parse_selection("none").unwrap(),
            OutboundSelection::Prefixes(vec![])
        );
        assert_eq!(
            parse_selection("https://a.example/, https://b.example/v1/").unwrap(),
            OutboundSelection::Prefixes(vec![
                "https://a.example/".to_owned(),
                "https://b.example/v1/".to_owned()
            ])
        );
        assert!(parse_selection(" ").is_err());
    }
}
