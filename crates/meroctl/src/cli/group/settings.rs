use calimero_primitives::identity::PublicKey;
use calimero_server_primitives::admin::{
    SetDefaultCapabilitiesApiRequest, SetDefaultVisibilityApiRequest,
};
use clap::{Parser, Subcommand, ValueEnum};
use eyre::Result;

use crate::cli::Environment;

#[derive(Clone, Debug, ValueEnum)]
pub enum VisibilityModeArg {
    Open,
    Restricted,
}

#[derive(Debug, Parser)]
#[command(about = "Manage group-level default settings")]
pub struct SettingsCommand {
    #[command(subcommand)]
    pub subcommand: SettingsSubCommands,
}

#[derive(Debug, Subcommand)]
pub enum SettingsSubCommands {
    #[command(about = "Get current default settings for a group")]
    Get(SettingsGetCommand),
    #[command(
        alias = "set-default-caps",
        about = "Set default capabilities for new group members"
    )]
    SetDefaultCapabilities(SetDefaultCapabilitiesCommand),
    #[command(
        alias = "set-default-vis",
        about = "Set default visibility mode for new contexts"
    )]
    SetDefaultVisibility(SetDefaultVisibilityCommand),
}

impl SettingsCommand {
    pub async fn run(self, environment: &mut Environment) -> Result<()> {
        match self.subcommand {
            SettingsSubCommands::Get(cmd) => cmd.run(environment).await,
            SettingsSubCommands::SetDefaultCapabilities(cmd) => cmd.run(environment).await,
            SettingsSubCommands::SetDefaultVisibility(cmd) => cmd.run(environment).await,
        }
    }
}

#[derive(Clone, Debug, Parser)]
#[command(about = "Get current default settings for a group")]
pub struct SettingsGetCommand {
    #[clap(name = "GROUP_ID", help = "The hex-encoded group ID")]
    pub group_id: String,
}

impl SettingsGetCommand {
    pub async fn run(self, environment: &mut Environment) -> Result<()> {
        let client = environment.client()?;
        let response = client.get_group_info(&self.group_id).await?;

        let caps = response.data.default_capabilities;
        let vis = &response.data.default_visibility;

        use comfy_table::{Cell, Color, Table};
        let mut table = Table::new();
        let _ = table.set_header(vec![
            Cell::new("Default Setting").fg(Color::Blue),
            Cell::new("Value").fg(Color::Blue),
        ]);
        let _ = table.add_row(vec!["Default Visibility", vis.as_str()]);
        let _ = table.add_row(vec![
            "CAN_CREATE_CONTEXT",
            if caps & (1 << 0) != 0 {
                "true"
            } else {
                "false"
            },
        ]);
        let _ = table.add_row(vec![
            "CAN_INVITE_MEMBERS",
            if caps & (1 << 1) != 0 {
                "true"
            } else {
                "false"
            },
        ]);
        let _ = table.add_row(vec![
            "CAN_JOIN_OPEN_CONTEXTS",
            if caps & (1 << 2) != 0 {
                "true"
            } else {
                "false"
            },
        ]);
        println!("{table}");

        Ok(())
    }
}

#[derive(Clone, Debug, Parser)]
#[command(about = "Set default capabilities for new group members (admin-only)")]
pub struct SetDefaultCapabilitiesCommand {
    #[clap(name = "GROUP_ID", help = "The hex-encoded group ID")]
    pub group_id: String,

    #[clap(long, help = "Allow new members to create contexts by default")]
    pub can_create_context: bool,

    #[clap(long, help = "Allow new members to invite others by default")]
    pub can_invite_members: bool,

    #[clap(long, help = "Allow new members to join open contexts by default")]
    pub can_join_open_contexts: bool,

    #[clap(
        long,
        help = "Public key of the requester (group admin). Auto-resolved from node group identity if omitted"
    )]
    pub requester: Option<PublicKey>,
}

impl SetDefaultCapabilitiesCommand {
    pub async fn run(self, environment: &mut Environment) -> Result<()> {
        let mut capabilities: u32 = 0;
        if self.can_create_context {
            capabilities |= 1 << 0;
        }
        if self.can_invite_members {
            capabilities |= 1 << 1;
        }
        if self.can_join_open_contexts {
            capabilities |= 1 << 2;
        }

        let request = SetDefaultCapabilitiesApiRequest {
            default_capabilities: capabilities,
            requester: self.requester,
        };

        let client = environment.client()?;
        let response = client
            .set_default_capabilities(&self.group_id, request)
            .await?;

        environment.output.write(&response);

        Ok(())
    }
}

#[derive(Clone, Debug, Parser)]
#[command(about = "Set default visibility mode for new contexts in the group (admin-only)")]
pub struct SetDefaultVisibilityCommand {
    #[clap(name = "GROUP_ID", help = "The hex-encoded group ID")]
    pub group_id: String,

    #[clap(long, value_enum, help = "Default visibility: open or restricted")]
    pub mode: VisibilityModeArg,

    #[clap(
        long,
        help = "Public key of the requester (group admin). Auto-resolved from node group identity if omitted"
    )]
    pub requester: Option<PublicKey>,
}

impl SetDefaultVisibilityCommand {
    pub async fn run(self, environment: &mut Environment) -> Result<()> {
        let mode_str = match self.mode {
            VisibilityModeArg::Open => "open",
            VisibilityModeArg::Restricted => "restricted",
        };

        let request = SetDefaultVisibilityApiRequest {
            default_visibility: mode_str.to_owned(),
            requester: self.requester,
        };

        let client = environment.client()?;
        let response = client
            .set_default_visibility(&self.group_id, request)
            .await?;

        environment.output.write(&response);

        Ok(())
    }
}
