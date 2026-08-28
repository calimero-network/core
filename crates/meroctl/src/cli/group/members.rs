use calimero_context_config::MemberCapabilities;
use calimero_primitives::context::GroupMemberRole;
use calimero_primitives::identity::MemberIdentity;
use calimero_server_primitives::admin::{
    AddGroupMembersApiRequest, GroupMemberApiInput, RemoveGroupMembersApiRequest,
    SetMemberCapabilitiesApiRequest, UpdateMemberRoleApiRequest,
};
use clap::{Parser, Subcommand, ValueEnum};
use eyre::Result;

use crate::cli::Environment;
use crate::confirm::confirm;
use crate::output::InfoLine;

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
    #[clap(
        name = "GROUP_ID",
        value_parser = crate::cli::validation::group_id,
        help = "The hex-encoded group ID"
    )]
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
    #[clap(
        name = "GROUP_ID",
        value_parser = crate::cli::validation::group_id,
        help = "The hex-encoded group ID"
    )]
    pub group_id: String,

    #[clap(
        name = "IDENTITY",
        help = "Account (64 hex), or public key as key:<64 hex>, of the member to add"
    )]
    pub identity: MemberIdentity,

    #[clap(
        name = "ROLE",
        value_enum,
        default_value = "member",
        help = "Role to assign to the new member"
    )]
    pub role: MemberRoleArg,
}

impl AddMembersCommand {
    pub async fn run(self, environment: &mut Environment) -> Result<()> {
        let request = AddGroupMembersApiRequest {
            members: vec![GroupMemberApiInput {
                identity: self.identity,
                role: self.role.into(),
            }],
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
    #[clap(
        name = "GROUP_ID",
        value_parser = crate::cli::validation::group_id,
        help = "The hex-encoded group ID"
    )]
    pub group_id: String,

    #[clap(
        name = "ACCOUNTS",
        required = true,
        help = "Accounts to remove, 64 hex characters each (space-separated). \
                `meroctl group members list` prints them."
    )]
    pub identities: Vec<calimero_account::AccountId>,

    #[clap(long, short = 'y', help = "Skip the confirmation prompt")]
    pub yes: bool,
}

impl RemoveMembersCommand {
    pub async fn run(self, environment: &mut Environment) -> Result<()> {
        if !confirm(
            &format!(
                "Remove {} member(s) from group '{}'?",
                self.identities.len(),
                self.group_id
            ),
            self.yes,
        )? {
            environment.output.write(&InfoLine("Aborted."));
            return Ok(());
        }

        let request = RemoveGroupMembersApiRequest {
            members: self.identities,
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
    #[clap(
        name = "GROUP_ID",
        value_parser = crate::cli::validation::group_id,
        help = "The hex-encoded group ID"
    )]
    pub group_id: String,

    #[clap(
        name = "ACCOUNT",
        help = "Account of the member whose role to update (64 hex characters)"
    )]
    pub identity: calimero_account::AccountId,

    #[clap(name = "ROLE", value_enum, help = "New role to assign")]
    pub role: MemberRoleArg,
}

impl SetRoleCommand {
    pub async fn run(self, environment: &mut Environment) -> Result<()> {
        let identity_hex = self.identity.to_string();

        let request = UpdateMemberRoleApiRequest {
            role: self.role.into(),
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
    #[clap(
        name = "GROUP_ID",
        value_parser = crate::cli::validation::group_id,
        help = "The hex-encoded group ID"
    )]
    pub group_id: String,

    #[clap(name = "ACCOUNT", help = "Account of the member (64 hex characters)")]
    pub identity: calimero_account::AccountId,

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
}

impl SetCapabilitiesCommand {
    pub async fn run(self, environment: &mut Environment) -> Result<()> {
        let capabilities = encode_capabilities(&self);

        let identity_hex = self.identity.to_string();

        let request = SetMemberCapabilitiesApiRequest { capabilities };

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
    #[clap(
        name = "GROUP_ID",
        value_parser = crate::cli::validation::group_id,
        help = "The hex-encoded group ID"
    )]
    pub group_id: String,

    #[clap(name = "ACCOUNT", help = "Account of the member (64 hex characters)")]
    pub identity: calimero_account::AccountId,
}

impl GetCapabilitiesCommand {
    pub async fn run(self, environment: &mut Environment) -> Result<()> {
        let identity_hex = self.identity.to_string();

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
    #[clap(
        name = "GROUP_ID",
        value_parser = crate::cli::validation::group_id,
        help = "The hex-encoded group ID"
    )]
    pub group_id: String,

    #[clap(name = "ACCOUNT", help = "Account to check (64 hex characters)")]
    pub identity: calimero_account::AccountId,
}

impl CheckAccessCommand {
    pub async fn run(self, environment: &mut Environment) -> Result<()> {
        let identity_hex = self.identity.to_string();

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

/// Encode the member-capability flags into the `MemberCapabilities` bitmask sent
/// to the node.
///
/// Takes the command rather than seven `bool`s. Positionally, every one of those
/// arguments had the same type and the call site passed them in declaration
/// order — so transposing any two compiled, passed every test, and granted the
/// wrong capability. Nothing in a bitmask of `u32` reveals afterwards which flag
/// was meant. Reading each field by name at the point of use makes the mistake
/// unrepresentable rather than merely unlikely.
fn encode_capabilities(cmd: &SetCapabilitiesCommand) -> u32 {
    let mut capabilities: u32 = 0;
    if cmd.can_create_context {
        capabilities |= MemberCapabilities::CAN_CREATE_CONTEXT.bits();
    }
    if cmd.can_invite_members {
        capabilities |= MemberCapabilities::CAN_INVITE_MEMBERS.bits();
    }
    if cmd.can_join_open_subgroups {
        capabilities |= MemberCapabilities::CAN_JOIN_OPEN_SUBGROUPS.bits();
    }
    if cmd.can_create_subgroup {
        capabilities |= MemberCapabilities::CAN_CREATE_SUBGROUP.bits();
    }
    if cmd.can_delete_subgroup {
        capabilities |= MemberCapabilities::CAN_DELETE_SUBGROUP.bits();
    }
    if cmd.can_manage_visibility {
        capabilities |= MemberCapabilities::CAN_MANAGE_VISIBILITY.bits();
    }
    if cmd.can_manage_metadata {
        capabilities |= MemberCapabilities::CAN_MANAGE_METADATA.bits();
    }
    capabilities
}

#[cfg(test)]
mod tests {
    use super::{encode_capabilities, SetCapabilitiesCommand};
    use calimero_context_config::MemberCapabilities;

    /// A command with every flag off, to be turned on by name.
    ///
    /// Built field-by-field on purpose. The previous tests called
    /// `encode_capabilities(false, true, false, …)` positionally, which is the
    /// same hazard the signature had — a test written that way cannot catch a
    /// transposition, because it makes the identical mistake.
    fn cmd() -> SetCapabilitiesCommand {
        SetCapabilitiesCommand {
            group_id: String::new(),
            identity: calimero_account::AccountId::from([0u8; 32]),
            can_create_context: false,
            can_invite_members: false,
            can_join_open_subgroups: false,
            can_create_subgroup: false,
            can_delete_subgroup: false,
            can_manage_visibility: false,
            can_manage_metadata: false,
        }
    }

    #[test]
    fn no_flags_encodes_to_zero() {
        assert_eq!(encode_capabilities(&cmd()), 0);
    }

    /// **Every** flag, not a sample of three.
    ///
    /// The old test covered `can_create_context`, `can_invite_members` and
    /// `can_manage_metadata`, so swapping the four untested flags with each other
    /// passed. Each entry here sets exactly one field by name and asserts exactly
    /// one bit, which is what makes a mis-wiring visible.
    /// One row of [`each_flag_maps_to_its_own_bit`]: the field to switch on, the
    /// single bit that must result, and the name to report.
    struct FlagCase {
        set: fn(&mut SetCapabilitiesCommand),
        expected: MemberCapabilities,
        name: &'static str,
    }

    #[test]
    fn each_flag_maps_to_its_own_bit() {
        let cases = [
            FlagCase {
                set: |c| c.can_create_context = true,
                expected: MemberCapabilities::CAN_CREATE_CONTEXT,
                name: "can_create_context",
            },
            FlagCase {
                set: |c| c.can_invite_members = true,
                expected: MemberCapabilities::CAN_INVITE_MEMBERS,
                name: "can_invite_members",
            },
            FlagCase {
                set: |c| c.can_join_open_subgroups = true,
                expected: MemberCapabilities::CAN_JOIN_OPEN_SUBGROUPS,
                name: "can_join_open_subgroups",
            },
            FlagCase {
                set: |c| c.can_create_subgroup = true,
                expected: MemberCapabilities::CAN_CREATE_SUBGROUP,
                name: "can_create_subgroup",
            },
            FlagCase {
                set: |c| c.can_delete_subgroup = true,
                expected: MemberCapabilities::CAN_DELETE_SUBGROUP,
                name: "can_delete_subgroup",
            },
            FlagCase {
                set: |c| c.can_manage_visibility = true,
                expected: MemberCapabilities::CAN_MANAGE_VISIBILITY,
                name: "can_manage_visibility",
            },
            FlagCase {
                set: |c| c.can_manage_metadata = true,
                expected: MemberCapabilities::CAN_MANAGE_METADATA,
                name: "can_manage_metadata",
            },
        ];

        for case in cases {
            let mut c = cmd();
            (case.set)(&mut c);
            assert_eq!(
                encode_capabilities(&c),
                case.expected.bits(),
                "{} must set exactly its own bit and no other",
                case.name
            );
        }
    }

    #[test]
    fn all_flags_or_together() {
        let mut c = cmd();
        c.can_create_context = true;
        c.can_invite_members = true;
        c.can_join_open_subgroups = true;
        c.can_create_subgroup = true;
        c.can_delete_subgroup = true;
        c.can_manage_visibility = true;
        c.can_manage_metadata = true;

        let all = encode_capabilities(&c);
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
