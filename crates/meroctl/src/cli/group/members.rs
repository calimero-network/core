use calimero_primitives::context::GroupMemberRole;
use calimero_primitives::identity::PublicKey;
use calimero_server_primitives::admin::{
    AddGroupMembersApiRequest, GroupMemberApiInput, RemoveGroupMembersApiRequest,
    SetMemberCapabilitiesApiRequest, UpdateMemberRoleApiRequest,
};
use clap::{Parser, Subcommand, ValueEnum};
use eyre::Result;

use crate::cli::Environment;

#[derive(Clone, Debug, ValueEnum)]
pub enum MemberRoleArg {
    Admin,
    Member,
}

impl From<MemberRoleArg> for GroupMemberRole {
    fn from(arg: MemberRoleArg) -> Self {
        match arg {
            MemberRoleArg::Admin => GroupMemberRole::Admin,
            MemberRoleArg::Member => GroupMemberRole::Member,
        }
    }
}

#[derive(Debug, Parser)]
#[command(about = "Manage group members")]
pub struct MembersCommand {
    #[command(subcommand)]
    pub subcommand: MembersSubCommands,
}

#[derive(Debug, Subcommand)]
pub enum MembersSubCommands {
    #[command(alias = "ls", about = "List all members of a group")]
    List(ListMembersCommand),
    #[command(about = "Add a member to a group")]
    Add(AddMembersCommand),
    #[command(about = "Remove members from a group")]
    Remove(RemoveMembersCommand),
    #[command(about = "Update the role of a group member")]
    SetRole(SetRoleCommand),
    #[command(
        alias = "set-caps",
        about = "Set capabilities for a group member (admin-only)"
    )]
    SetCapabilities(SetCapabilitiesCommand),
    #[command(alias = "get-caps", about = "Get capabilities of a group member")]
    GetCapabilities(GetCapabilitiesCommand),
}

impl MembersCommand {
    pub async fn run(self, environment: &mut Environment) -> Result<()> {
        match self.subcommand {
            MembersSubCommands::List(cmd) => cmd.run(environment).await,
            MembersSubCommands::Add(cmd) => cmd.run(environment).await,
            MembersSubCommands::Remove(cmd) => cmd.run(environment).await,
            MembersSubCommands::SetRole(cmd) => cmd.run(environment).await,
            MembersSubCommands::SetCapabilities(cmd) => cmd.run(environment).await,
            MembersSubCommands::GetCapabilities(cmd) => cmd.run(environment).await,
        }
    }
}

#[derive(Clone, Debug, Parser)]
#[command(about = "List all members of a group")]
pub struct ListMembersCommand {
    #[clap(name = "GROUP_ID", help = "The hex-encoded group ID")]
    pub group_id: String,
}

impl ListMembersCommand {
    pub async fn run(self, environment: &mut Environment) -> Result<()> {
        let client = environment.client()?;
        let response = client.list_group_members(&self.group_id).await?;

        environment.output.write(&response);

        Ok(())
    }
}

#[derive(Clone, Debug, Parser)]
#[command(about = "Add a member to a group")]
pub struct AddMembersCommand {
    #[clap(name = "GROUP_ID", help = "The hex-encoded group ID")]
    pub group_id: String,

    #[clap(name = "IDENTITY", help = "Public key of the identity to add")]
    pub identity: PublicKey,

    #[clap(
        name = "ROLE",
        value_enum,
        default_value = "member",
        help = "Role to assign to the new member"
    )]
    pub role: MemberRoleArg,

    #[clap(
        long,
        help = "Public key of the requester (group admin). Auto-resolved from node group identity if omitted"
    )]
    pub requester: Option<PublicKey>,
}

impl AddMembersCommand {
    pub async fn run(self, environment: &mut Environment) -> Result<()> {
        let request = AddGroupMembersApiRequest {
            members: vec![GroupMemberApiInput {
                identity: self.identity,
                role: self.role.into(),
            }],
            requester: self.requester,
        };

        let client = environment.client()?;
        let response = client.add_group_members(&self.group_id, request).await?;

        environment.output.write(&response);

        Ok(())
    }
}

#[derive(Clone, Debug, Parser)]
#[command(about = "Remove members from a group")]
pub struct RemoveMembersCommand {
    #[clap(name = "GROUP_ID", help = "The hex-encoded group ID")]
    pub group_id: String,

    #[clap(
        name = "IDENTITIES",
        required = true,
        help = "Public keys of identities to remove (space-separated)"
    )]
    pub identities: Vec<PublicKey>,

    #[clap(
        long,
        help = "Public key of the requester (group admin). Auto-resolved from node group identity if omitted"
    )]
    pub requester: Option<PublicKey>,
}

impl RemoveMembersCommand {
    pub async fn run(self, environment: &mut Environment) -> Result<()> {
        let request = RemoveGroupMembersApiRequest {
            members: self.identities,
            requester: self.requester,
        };

        let client = environment.client()?;
        let response = client.remove_group_members(&self.group_id, request).await?;

        environment.output.write(&response);

        Ok(())
    }
}

#[derive(Clone, Debug, Parser)]
#[command(about = "Update the role of a group member")]
pub struct SetRoleCommand {
    #[clap(name = "GROUP_ID", help = "The hex-encoded group ID")]
    pub group_id: String,

    #[clap(
        name = "IDENTITY",
        help = "Public key of the member whose role to update"
    )]
    pub identity: PublicKey,

    #[clap(name = "ROLE", value_enum, help = "New role to assign")]
    pub role: MemberRoleArg,

    #[clap(
        long,
        help = "Public key of the requester (group admin). Auto-resolved from node group identity if omitted"
    )]
    pub requester: Option<PublicKey>,
}

impl SetRoleCommand {
    pub async fn run(self, environment: &mut Environment) -> Result<()> {
        let identity_hex = hex::encode(self.identity.digest());

        let request = UpdateMemberRoleApiRequest {
            role: self.role.into(),
            requester: self.requester,
        };

        let client = environment.client()?;
        let response = client
            .update_member_role(&self.group_id, &identity_hex, request)
            .await?;

        environment.output.write(&response);

        Ok(())
    }
}

#[derive(Clone, Debug, Parser)]
#[command(about = "Set capabilities for a group member")]
pub struct SetCapabilitiesCommand {
    #[clap(name = "GROUP_ID", help = "The hex-encoded group ID")]
    pub group_id: String,

    #[clap(name = "IDENTITY", help = "Public key of the member")]
    pub identity: PublicKey,

    #[clap(long, help = "Allow member to create contexts in the group")]
    pub can_create_context: bool,

    #[clap(long, help = "Allow member to invite others to the group")]
    pub can_invite_members: bool,

    #[clap(long, help = "Allow member to join open contexts")]
    pub can_join_open_contexts: bool,

    #[clap(
        long,
        help = "Public key of the requester (group admin). Auto-resolved from node group identity if omitted"
    )]
    pub requester: Option<PublicKey>,
}

impl SetCapabilitiesCommand {
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

        let identity_hex = hex::encode(self.identity.digest());

        let request = SetMemberCapabilitiesApiRequest {
            capabilities,
            requester: self.requester,
        };

        let client = environment.client()?;
        let response = client
            .set_member_capabilities(&self.group_id, &identity_hex, request)
            .await?;

        environment.output.write(&response);

        Ok(())
    }
}

#[derive(Clone, Debug, Parser)]
#[command(about = "Get capabilities of a group member")]
pub struct GetCapabilitiesCommand {
    #[clap(name = "GROUP_ID", help = "The hex-encoded group ID")]
    pub group_id: String,

    #[clap(name = "IDENTITY", help = "Public key of the member")]
    pub identity: PublicKey,
}

impl GetCapabilitiesCommand {
    pub async fn run(self, environment: &mut Environment) -> Result<()> {
        let identity_hex = hex::encode(self.identity.digest());

        let client = environment.client()?;
        let response = client
            .get_member_capabilities(&self.group_id, &identity_hex)
            .await?;

        environment.output.write(&response);

        Ok(())
    }
}
