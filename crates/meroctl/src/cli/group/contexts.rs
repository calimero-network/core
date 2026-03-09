use calimero_primitives::context::ContextId;
use calimero_primitives::identity::PublicKey;
use calimero_server_primitives::admin::{
    DetachContextFromGroupApiRequest, ManageContextAllowlistApiRequest,
    SetContextVisibilityApiRequest,
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
#[command(about = "Manage contexts within a group")]
pub struct ContextsCommand {
    #[command(subcommand)]
    pub subcommand: ContextsSubCommands,
}

#[derive(Debug, Subcommand)]
pub enum ContextsSubCommands {
    #[command(alias = "ls", about = "List all contexts in a group")]
    List(ListGroupContextsCommand),
    #[command(about = "Detach a context from a group")]
    Detach(DetachContextCommand),
    #[command(
        alias = "set-vis",
        about = "Set visibility mode for a context (open or restricted)"
    )]
    SetVisibility(SetVisibilityCommand),
    #[command(alias = "get-vis", about = "Get visibility mode for a context")]
    GetVisibility(GetVisibilityCommand),
    #[command(about = "Manage the allowlist for a restricted context")]
    Allowlist(AllowlistCommand),
}

impl ContextsCommand {
    pub async fn run(self, environment: &mut Environment) -> Result<()> {
        match self.subcommand {
            ContextsSubCommands::List(cmd) => cmd.run(environment).await,
            ContextsSubCommands::Detach(cmd) => cmd.run(environment).await,
            ContextsSubCommands::SetVisibility(cmd) => cmd.run(environment).await,
            ContextsSubCommands::GetVisibility(cmd) => cmd.run(environment).await,
            ContextsSubCommands::Allowlist(cmd) => cmd.run(environment).await,
        }
    }
}

#[derive(Clone, Debug, Parser)]
#[command(about = "List all contexts in a group")]
pub struct ListGroupContextsCommand {
    #[clap(name = "GROUP_ID", help = "The hex-encoded group ID")]
    pub group_id: String,
}

impl ListGroupContextsCommand {
    pub async fn run(self, environment: &mut Environment) -> Result<()> {
        let client = environment.client()?;
        let response = client.list_group_contexts(&self.group_id).await?;

        environment.output.write(&response);

        Ok(())
    }
}

#[derive(Clone, Debug, Parser)]
#[command(about = "Detach a context from a group")]
pub struct DetachContextCommand {
    #[clap(name = "GROUP_ID", help = "The hex-encoded group ID")]
    pub group_id: String,

    #[clap(name = "CONTEXT_ID", help = "The context ID (base58)")]
    pub context_id: ContextId,

    #[clap(
        long,
        help = "Public key of the requester (group admin). Auto-resolved from node group identity if omitted"
    )]
    pub requester: Option<PublicKey>,
}

impl DetachContextCommand {
    pub async fn run(self, environment: &mut Environment) -> Result<()> {
        let request = DetachContextFromGroupApiRequest {
            requester: self.requester,
        };

        let client = environment.client()?;
        let response = client
            .detach_context_from_group(&self.group_id, &self.context_id.to_string(), request)
            .await?;

        environment.output.write(&response);

        Ok(())
    }
}

#[derive(Clone, Debug, Parser)]
#[command(about = "Set visibility mode for a context in a group")]
pub struct SetVisibilityCommand {
    #[clap(name = "GROUP_ID", help = "The hex-encoded group ID")]
    pub group_id: String,

    #[clap(name = "CONTEXT_ID", help = "The context ID (base58)")]
    pub context_id: ContextId,

    #[clap(long, value_enum, help = "Visibility mode: open or restricted")]
    pub mode: VisibilityModeArg,

    #[clap(
        long,
        help = "Public key of the requester. Auto-resolved from node group identity if omitted"
    )]
    pub requester: Option<PublicKey>,
}

impl SetVisibilityCommand {
    pub async fn run(self, environment: &mut Environment) -> Result<()> {
        let mode_str = match self.mode {
            VisibilityModeArg::Open => "open",
            VisibilityModeArg::Restricted => "restricted",
        };

        let request = SetContextVisibilityApiRequest {
            mode: mode_str.to_owned(),
            requester: self.requester,
        };

        let context_id_hex = hex::encode(AsRef::<[u8; 32]>::as_ref(&self.context_id));

        let client = environment.client()?;
        let response = client
            .set_context_visibility(&self.group_id, &context_id_hex, request)
            .await?;

        environment.output.write(&response);

        Ok(())
    }
}

#[derive(Clone, Debug, Parser)]
#[command(about = "Get visibility mode for a context in a group")]
pub struct GetVisibilityCommand {
    #[clap(name = "GROUP_ID", help = "The hex-encoded group ID")]
    pub group_id: String,

    #[clap(name = "CONTEXT_ID", help = "The context ID (base58)")]
    pub context_id: ContextId,
}

impl GetVisibilityCommand {
    pub async fn run(self, environment: &mut Environment) -> Result<()> {
        let context_id_hex = hex::encode(AsRef::<[u8; 32]>::as_ref(&self.context_id));

        let client = environment.client()?;
        let response = client
            .get_context_visibility(&self.group_id, &context_id_hex)
            .await?;

        environment.output.write(&response);

        Ok(())
    }
}

// ---- Allowlist subcommands ----

#[derive(Debug, Parser)]
#[command(about = "Manage context allowlist")]
pub struct AllowlistCommand {
    #[command(subcommand)]
    pub subcommand: AllowlistSubCommands,
}

#[derive(Debug, Subcommand)]
pub enum AllowlistSubCommands {
    #[command(alias = "ls", about = "List members on the allowlist")]
    List(ListAllowlistCommand),
    #[command(about = "Add members to the allowlist")]
    Add(AddAllowlistCommand),
    #[command(about = "Remove members from the allowlist")]
    Remove(RemoveAllowlistCommand),
}

impl AllowlistCommand {
    pub async fn run(self, environment: &mut Environment) -> Result<()> {
        match self.subcommand {
            AllowlistSubCommands::List(cmd) => cmd.run(environment).await,
            AllowlistSubCommands::Add(cmd) => cmd.run(environment).await,
            AllowlistSubCommands::Remove(cmd) => cmd.run(environment).await,
        }
    }
}

#[derive(Clone, Debug, Parser)]
#[command(about = "List members on the context allowlist")]
pub struct ListAllowlistCommand {
    #[clap(name = "GROUP_ID", help = "The hex-encoded group ID")]
    pub group_id: String,

    #[clap(name = "CONTEXT_ID", help = "The context ID (base58)")]
    pub context_id: ContextId,
}

impl ListAllowlistCommand {
    pub async fn run(self, environment: &mut Environment) -> Result<()> {
        let context_id_hex = hex::encode(AsRef::<[u8; 32]>::as_ref(&self.context_id));

        let client = environment.client()?;
        let response = client
            .get_context_allowlist(&self.group_id, &context_id_hex)
            .await?;

        environment.output.write(&response);

        Ok(())
    }
}

#[derive(Clone, Debug, Parser)]
#[command(about = "Add members to the context allowlist")]
pub struct AddAllowlistCommand {
    #[clap(name = "GROUP_ID", help = "The hex-encoded group ID")]
    pub group_id: String,

    #[clap(name = "CONTEXT_ID", help = "The context ID (base58)")]
    pub context_id: ContextId,

    #[clap(
        name = "MEMBERS",
        required = true,
        help = "Public keys of members to add (space-separated)"
    )]
    pub members: Vec<PublicKey>,

    #[clap(
        long,
        help = "Public key of the requester. Auto-resolved from node group identity if omitted"
    )]
    pub requester: Option<PublicKey>,
}

impl AddAllowlistCommand {
    pub async fn run(self, environment: &mut Environment) -> Result<()> {
        let context_id_hex = hex::encode(AsRef::<[u8; 32]>::as_ref(&self.context_id));

        let request = ManageContextAllowlistApiRequest {
            add: self.members,
            remove: vec![],
            requester: self.requester,
        };

        let client = environment.client()?;
        let response = client
            .manage_context_allowlist(&self.group_id, &context_id_hex, request)
            .await?;

        environment.output.write(&response);

        Ok(())
    }
}

#[derive(Clone, Debug, Parser)]
#[command(about = "Remove members from the context allowlist")]
pub struct RemoveAllowlistCommand {
    #[clap(name = "GROUP_ID", help = "The hex-encoded group ID")]
    pub group_id: String,

    #[clap(name = "CONTEXT_ID", help = "The context ID (base58)")]
    pub context_id: ContextId,

    #[clap(
        name = "MEMBERS",
        required = true,
        help = "Public keys of members to remove (space-separated)"
    )]
    pub members: Vec<PublicKey>,

    #[clap(
        long,
        help = "Public key of the requester. Auto-resolved from node group identity if omitted"
    )]
    pub requester: Option<PublicKey>,
}

impl RemoveAllowlistCommand {
    pub async fn run(self, environment: &mut Environment) -> Result<()> {
        let context_id_hex = hex::encode(AsRef::<[u8; 32]>::as_ref(&self.context_id));

        let request = ManageContextAllowlistApiRequest {
            add: vec![],
            remove: self.members,
            requester: self.requester,
        };

        let client = environment.client()?;
        let response = client
            .manage_context_allowlist(&self.group_id, &context_id_hex, request)
            .await?;

        environment.output.write(&response);

        Ok(())
    }
}
