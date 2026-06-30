use calimero_context_config::MemberCapabilities;
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
    ReadOnly,
}

impl From<MemberRoleArg> for GroupMemberRole {
    fn from(arg: MemberRoleArg) -> Self {
        match arg {
            MemberRoleArg::Admin => GroupMemberRole::Admin,
            MemberRoleArg::Member => GroupMemberRole::Member,
            MemberRoleArg::ReadOnly => GroupMemberRole::ReadOnly,
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
    #[command(about = "Check if an identity can join a context in this group")]
    CheckAccess(CheckAccessCommand),
}

impl MembersCommand {
    pub async fn run(self, environment: &mut Environment) -> Result<()> {
        crate::cli::dispatch_subcommands!(
            self.subcommand,
            environment,
            MembersSubCommands::List,
            MembersSubCommands::Add,
            MembersSubCommands::Remove,
            MembersSubCommands::SetRole,
            MembersSubCommands::SetCapabilities,
            MembersSubCommands::GetCapabilities,
            MembersSubCommands::CheckAccess,
        )
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

    #[clap(long, help = "Allow member to join open subgroups")]
    pub can_join_open_subgroups: bool,

    #[clap(
        long,
        help = "Allow member to create a subgroup directly under the namespace root"
    )]
    pub can_create_subgroup: bool,

    #[clap(
        long,
        help = "Allow member to cascade-delete a subgroup and its subtree"
    )]
    pub can_delete_subgroup: bool,

    #[clap(
        long,
        help = "Allow member to change a subgroup's visibility (open/restricted)"
    )]
    pub can_manage_visibility: bool,

    #[clap(
        long,
        help = "Allow member to set name/data on the group, its members, or its contexts"
    )]
    pub can_manage_metadata: bool,

    #[clap(
        long,
        help = "Public key of the requester (group admin). Auto-resolved from node group identity if omitted"
    )]
    pub requester: Option<PublicKey>,
}

impl SetCapabilitiesCommand {
    pub async fn run(self, environment: &mut Environment) -> Result<()> {
        let capabilities = encode_capabilities(
            self.can_create_context,
            self.can_invite_members,
            self.can_join_open_subgroups,
            self.can_create_subgroup,
            self.can_delete_subgroup,
            self.can_manage_visibility,
            self.can_manage_metadata,
        );

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

#[derive(Clone, Debug, Parser)]
#[command(about = "Diagnostic: check an identity's role and capabilities in a group")]
pub struct CheckAccessCommand {
    #[clap(name = "GROUP_ID", help = "The hex-encoded group ID")]
    pub group_id: String,

    #[clap(name = "IDENTITY", help = "Public key of the identity to check")]
    pub identity: PublicKey,
}

impl CheckAccessCommand {
    pub async fn run(self, environment: &mut Environment) -> Result<()> {
        let identity_hex = hex::encode(self.identity.digest());

        let client = environment.client()?;

        let caps_response = client
            .get_member_capabilities(&self.group_id, &identity_hex)
            .await?;
        let members_response = client.list_group_members(&self.group_id).await?;

        let caps = caps_response.data.capabilities;
        let role = members_response
            .members
            .iter()
            .find(|m| m.identity == self.identity)
            .map(|m| format!("{:?}", m.role).to_lowercase())
            .unwrap_or_else(|| "not a member".to_owned());

        println!("Role:                    {role}");
        println!(
            "CAN_CREATE_CONTEXT:      {}",
            caps & MemberCapabilities::CAN_CREATE_CONTEXT.bits() != 0
        );
        println!(
            "CAN_INVITE_MEMBERS:      {}",
            caps & MemberCapabilities::CAN_INVITE_MEMBERS.bits() != 0
        );
        println!(
            "CAN_JOIN_OPEN_SUBGROUPS: {}",
            caps & MemberCapabilities::CAN_JOIN_OPEN_SUBGROUPS.bits() != 0
        );
        println!(
            "CAN_CREATE_SUBGROUP:     {}",
            caps & MemberCapabilities::CAN_CREATE_SUBGROUP.bits() != 0
        );
        println!(
            "CAN_DELETE_SUBGROUP:     {}",
            caps & MemberCapabilities::CAN_DELETE_SUBGROUP.bits() != 0
        );
        println!(
            "CAN_MANAGE_VISIBILITY:   {}",
            caps & MemberCapabilities::CAN_MANAGE_VISIBILITY.bits() != 0
        );
        println!(
            "CAN_MANAGE_METADATA:     {}",
            caps & MemberCapabilities::CAN_MANAGE_METADATA.bits() != 0
        );

        Ok(())
    }
}

/// Encode the seven member-capability flags into the `MemberCapabilities`
/// bitmask sent to the node.
fn encode_capabilities(
    can_create_context: bool,
    can_invite_members: bool,
    can_join_open_subgroups: bool,
    can_create_subgroup: bool,
    can_delete_subgroup: bool,
    can_manage_visibility: bool,
    can_manage_metadata: bool,
) -> u32 {
    let mut capabilities: u32 = 0;
    if can_create_context {
        capabilities |= MemberCapabilities::CAN_CREATE_CONTEXT.bits();
    }
    if can_invite_members {
        capabilities |= MemberCapabilities::CAN_INVITE_MEMBERS.bits();
    }
    if can_join_open_subgroups {
        capabilities |= MemberCapabilities::CAN_JOIN_OPEN_SUBGROUPS.bits();
    }
    if can_create_subgroup {
        capabilities |= MemberCapabilities::CAN_CREATE_SUBGROUP.bits();
    }
    if can_delete_subgroup {
        capabilities |= MemberCapabilities::CAN_DELETE_SUBGROUP.bits();
    }
    if can_manage_visibility {
        capabilities |= MemberCapabilities::CAN_MANAGE_VISIBILITY.bits();
    }
    if can_manage_metadata {
        capabilities |= MemberCapabilities::CAN_MANAGE_METADATA.bits();
    }
    capabilities
}

#[cfg(test)]
mod tests {
    use super::encode_capabilities;
    use calimero_context_config::MemberCapabilities;

    #[test]
    fn no_flags_encodes_to_zero() {
        assert_eq!(
            encode_capabilities(false, false, false, false, false, false, false),
            0
        );
    }

    #[test]
    fn each_flag_maps_to_its_bit() {
        assert_eq!(
            encode_capabilities(true, false, false, false, false, false, false),
            MemberCapabilities::CAN_CREATE_CONTEXT.bits()
        );
        assert_eq!(
            encode_capabilities(false, true, false, false, false, false, false),
            MemberCapabilities::CAN_INVITE_MEMBERS.bits()
        );
        assert_eq!(
            encode_capabilities(false, false, false, false, false, false, true),
            MemberCapabilities::CAN_MANAGE_METADATA.bits()
        );
    }

    #[test]
    fn all_flags_or_together() {
        let all = encode_capabilities(true, true, true, true, true, true, true);
        let expected = (MemberCapabilities::CAN_CREATE_CONTEXT
            | MemberCapabilities::CAN_INVITE_MEMBERS
            | MemberCapabilities::CAN_JOIN_OPEN_SUBGROUPS
            | MemberCapabilities::CAN_CREATE_SUBGROUP
            | MemberCapabilities::CAN_DELETE_SUBGROUP
            | MemberCapabilities::CAN_MANAGE_VISIBILITY
            | MemberCapabilities::CAN_MANAGE_METADATA)
            .bits();
        assert_eq!(all, expected);
        // Every set bit is distinct (no two flags collide on a bit).
        assert_eq!(all.count_ones(), 7);
    }
}
