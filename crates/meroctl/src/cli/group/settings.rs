use calimero_context_config::MemberCapabilities;
use calimero_server_primitives::admin::{
    SetDefaultCapabilitiesApiRequest, SetSubgroupVisibilityApiRequest,
    SetTeeAdmissionPolicyApiRequest, SignedReleaseTeePolicy, TeeAdmissionMode,
};
use clap::{Parser, Subcommand, ValueEnum};
use eyre::Result;

use crate::cli::Environment;

#[derive(Clone, Debug, ValueEnum)]
pub enum VisibilityModeArg {
    Open,
    Restricted,
}

/// The role a namespace admits attested TEE nodes with.
#[derive(Clone, Copy, Debug, Default, ValueEnum)]
pub enum TeeAdmissionModeArg {
    /// `ReadOnlyTee`: replicates and anchors sync, never relays writes.
    #[default]
    Replica,
    /// `RelayTee`: a replica that also authors members' writes under their
    /// warrants.
    Relay,
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
        alias = "set-subgroup-vis",
        about = "Set this subgroup's visibility (Open inherits parent members; \
                 Restricted requires explicit add)"
    )]
    SetSubgroupVisibility(SetSubgroupVisibilityCommand),
    #[command(
        alias = "get-tee-policy",
        about = "Show a namespace's TEE admission policy, including its replica/relay mode"
    )]
    GetTeeAdmissionPolicy(GetTeeAdmissionPolicyCommand),
    #[command(
        alias = "set-tee-policy",
        about = "Set a namespace's TEE admission policy (admin-only)"
    )]
    SetTeeAdmissionPolicy(SetTeeAdmissionPolicyCommand),
}

impl SettingsCommand {
    pub async fn run(self, environment: &mut Environment) -> Result<()> {
        match self.subcommand {
            SettingsSubCommands::Get(cmd) => cmd.run(environment).await,
            SettingsSubCommands::SetDefaultCapabilities(cmd) => cmd.run(environment).await,
            SettingsSubCommands::SetSubgroupVisibility(cmd) => cmd.run(environment).await,
            SettingsSubCommands::GetTeeAdmissionPolicy(cmd) => cmd.run(environment).await,
            SettingsSubCommands::SetTeeAdmissionPolicy(cmd) => cmd.run(environment).await,
        }
    }
}

#[derive(Clone, Debug, Parser)]
#[command(about = "Get current default settings for a group")]
pub struct SettingsGetCommand {
    #[clap(
        name = "GROUP_ID",
        value_parser = crate::cli::validation::group_id,
        help = "The hex-encoded group ID"
    )]
    pub group_id: String,
}

impl SettingsGetCommand {
    pub async fn run(self, environment: &mut Environment) -> Result<()> {
        let client = environment.client()?;
        let response = client.get_group_info(&self.group_id).await?;

        let caps = response.data.default_capabilities;
        let vis = &response.data.subgroup_visibility;

        use comfy_table::{Cell, Color, Table};
        let mut table = Table::new();
        let _ = table.set_header(vec![
            Cell::new("Default Setting").fg(Color::Blue),
            Cell::new("Value").fg(Color::Blue),
        ]);
        let _ = table.add_row(vec!["Subgroup Visibility", vis.as_str()]);
        let _ = table.add_row(vec![
            "CAN_CREATE_CONTEXT",
            if caps & MemberCapabilities::CAN_CREATE_CONTEXT.bits() != 0 {
                "true"
            } else {
                "false"
            },
        ]);
        let _ = table.add_row(vec![
            "CAN_INVITE_MEMBERS",
            if caps & MemberCapabilities::CAN_INVITE_MEMBERS.bits() != 0 {
                "true"
            } else {
                "false"
            },
        ]);
        let _ = table.add_row(vec![
            "CAN_JOIN_OPEN_SUBGROUPS",
            if caps & MemberCapabilities::CAN_JOIN_OPEN_SUBGROUPS.bits() != 0 {
                "true"
            } else {
                "false"
            },
        ]);
        println!("{table}");

        Ok(())
    }
}

/// Set the mask a group seeds every NON-ADMIN member's capability row from at
/// admission.
///
/// The flags mirror `group members set-capabilities` one for one, and that is
/// the point rather than tidiness: three of them were missing here, so the mask
/// that decides what a new member gets could not express capabilities the
/// per-member command could. `--can-author-on-behalf` was the one that mattered
/// — it is the grant delegated execution runs on, and with no flag for it the
/// only way to open a namespace to a fleet of relays was a per-node op for each
/// one, published at the moment the node is assigned and the admin is not
/// watching.
#[derive(Clone, Debug, Parser)]
#[command(
    about = "Set default capabilities for new group members (admin-only)",
    long_about = "Set the capabilities a group seeds every new NON-ADMIN member's row \
                  with at admission (admin-only).\n\n\
                  REPLACES the mask, it does not add to it: any capability whose flag \
                  is absent from this invocation is cleared. Read the current mask with \
                  `meroctl group settings get-info <GROUP_ID>` and pass every flag you \
                  intend to keep -- a group created by this node starts with \
                  --can-join-open-subgroups set, and dropping it stops later members \
                  inheriting into Open subgroups.\n\n\
                  NOT retroactive: the mask is copied into a member's row when that \
                  member is admitted, so members already in the group keep what they \
                  have. Use `meroctl group members set-capabilities` for those.\n\n\
                  Admins never receive it -- core seeds the default for non-admin roles \
                  only."
)]
pub struct SetDefaultCapabilitiesCommand {
    #[clap(
        name = "GROUP_ID",
        value_parser = crate::cli::validation::group_id,
        help = "The hex-encoded group ID"
    )]
    pub group_id: String,

    #[clap(long, help = "Allow new members to create contexts by default")]
    pub can_create_context: bool,

    #[clap(long, help = "Allow new members to invite others by default")]
    pub can_invite_members: bool,

    #[clap(long, help = "Allow new members to join open subgroups by default")]
    pub can_join_open_subgroups: bool,

    #[clap(long, help = "Allow new members to create subgroups by default")]
    pub can_create_subgroup: bool,

    #[clap(long, help = "Allow new members to delete subgroups by default")]
    pub can_delete_subgroup: bool,

    #[clap(
        long,
        help = "Allow new members to manage subgroup visibility by default"
    )]
    pub can_manage_visibility: bool,

    #[clap(long, help = "Allow new members to manage group metadata by default")]
    pub can_manage_metadata: bool,

    #[clap(
        long,
        help = "Allow new members to publish writes attributed to another member, under \
                a warrant that member signed -- the grant delegated execution runs on for \
                self-hosted relays. It does NOT make a TEE a relay: a TEE replica \
                (ReadOnlyTee) never relays whatever it holds, and a TEE relay (RelayTee, \
                `set-tee-admission-policy --mode relay`) relays without it. It reaches \
                every non-admin member admitted afterwards"
    )]
    pub can_author_on_behalf: bool,
}

/// Encode the default-capability flags into the bitmask sent to the node.
///
/// Reads each field by name for the same reason `members::encode_capabilities`
/// does: a positional list of `bool`s of one type lets a transposition compile,
/// pass, and set the wrong bit, and a `u32` afterwards cannot say which was
/// meant.
fn encode_default_capabilities(cmd: &SetDefaultCapabilitiesCommand) -> u32 {
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
    if cmd.can_author_on_behalf {
        capabilities |= MemberCapabilities::CAN_AUTHOR_ON_BEHALF.bits();
    }
    capabilities
}

impl SetDefaultCapabilitiesCommand {
    pub async fn run(self, environment: &mut Environment) -> Result<()> {
        let capabilities = encode_default_capabilities(&self);

        let request = SetDefaultCapabilitiesApiRequest {
            default_capabilities: capabilities,
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
#[command(
    about = "Set this subgroup's visibility (admin-only). When Open, parent-group \
             members holding CAN_JOIN_OPEN_SUBGROUPS are inherited as members of \
             this subgroup. When Restricted, membership requires explicit \
             add_group_members"
)]
pub struct SetSubgroupVisibilityCommand {
    #[clap(
        name = "GROUP_ID",
        value_parser = crate::cli::validation::group_id,
        help = "The hex-encoded group ID"
    )]
    pub group_id: String,

    #[clap(long, value_enum, help = "Subgroup visibility: open or restricted")]
    pub mode: VisibilityModeArg,
}

impl SetSubgroupVisibilityCommand {
    pub async fn run(self, environment: &mut Environment) -> Result<()> {
        let mode_str = match self.mode {
            VisibilityModeArg::Open => "open",
            VisibilityModeArg::Restricted => "restricted",
        };

        let request = SetSubgroupVisibilityApiRequest {
            subgroup_visibility: mode_str.to_owned(),
        };

        let client = environment.client()?;
        let response = client
            .set_subgroup_visibility(&self.group_id, request)
            .await?;

        environment.output.write(&response);

        Ok(())
    }
}

#[derive(Clone, Debug, Parser)]
#[command(about = "Show a namespace's TEE admission policy")]
pub struct GetTeeAdmissionPolicyCommand {
    #[clap(
        name = "GROUP_ID",
        value_parser = crate::cli::validation::group_id,
        help = "The hex-encoded namespace (root group) ID"
    )]
    pub group_id: String,
}

impl GetTeeAdmissionPolicyCommand {
    pub async fn run(self, environment: &mut Environment) -> Result<()> {
        let client = environment.client()?;
        let response = client.get_tee_admission_policy(&self.group_id).await?;
        environment.output.write(&response);
        Ok(())
    }
}

/// Set which attested TEE nodes a namespace admits, and in which role.
///
/// Two forms, as the API has: measurement lists (`--mrtd`, `--rtmr1..3`), or
/// signed releases (`--profile`), which take their measurements from the
/// mero-tee release the node runs and must leave the lists empty.
#[derive(Clone, Debug, Parser)]
#[command(
    about = "Set a namespace's TEE admission policy (admin-only)",
    long_about = "Set which attested TEE nodes the namespace admits (admin-only), by \
                  measurement lists or by signed mero-tee release (--profile).\n\n\
                  --mode decides the role they are admitted with: `replica` (ReadOnlyTee, \
                  the default) replicates and anchors sync but never relays members' \
                  writes; `relay` (RelayTee) may also author members' writes under their \
                  signed warrants, with no CAN_AUTHOR_ON_BEHALF grant. Changing the mode \
                  also converts the TEEs already admitted."
)]
pub struct SetTeeAdmissionPolicyCommand {
    #[clap(
        name = "GROUP_ID",
        value_parser = crate::cli::validation::group_id,
        help = "The hex-encoded namespace (root group) ID"
    )]
    pub group_id: String,

    #[clap(
        long = "mrtd",
        value_name = "HEX",
        help = "An allowed MRTD (repeatable)"
    )]
    pub allowed_mrtd: Vec<String>,

    #[clap(
        long = "rtmr0",
        value_name = "HEX",
        help = "An allowed RTMR0 (repeatable)"
    )]
    pub allowed_rtmr0: Vec<String>,

    #[clap(
        long = "rtmr1",
        value_name = "HEX",
        help = "An allowed RTMR1 (repeatable)"
    )]
    pub allowed_rtmr1: Vec<String>,

    #[clap(
        long = "rtmr2",
        value_name = "HEX",
        help = "An allowed RTMR2 (repeatable)"
    )]
    pub allowed_rtmr2: Vec<String>,

    #[clap(
        long = "rtmr3",
        value_name = "HEX",
        help = "An allowed RTMR3 (repeatable)"
    )]
    pub allowed_rtmr3: Vec<String>,

    #[clap(
        long = "tcb-status",
        value_name = "STATUS",
        help = "An allowed TCB status (repeatable); none admits only UpToDate"
    )]
    pub allowed_tcb_statuses: Vec<String>,

    #[clap(long, help = "Admit mock quotes (mock-attestation builds only)")]
    pub accept_mock: bool,

    #[clap(
        long = "profile",
        value_name = "PROFILE",
        help = "Admit by signed release: an image profile to accept, e.g. locked-read-only \
                (repeatable). The measurement lists must then be empty"
    )]
    pub allowed_profiles: Vec<String>,

    #[clap(
        long,
        value_name = "VERSION",
        requires = "allowed_profiles",
        help = "With --profile: the oldest mero-tee release admitted"
    )]
    pub min_release_version: Option<String>,

    #[clap(
        long,
        value_enum,
        default_value_t,
        help = "Role admitted TEEs receive: replica (ReadOnlyTee) or relay (RelayTee)"
    )]
    pub mode: TeeAdmissionModeArg,
}

impl SetTeeAdmissionPolicyCommand {
    fn request(self) -> SetTeeAdmissionPolicyApiRequest {
        let signed_release =
            (!self.allowed_profiles.is_empty()).then_some(SignedReleaseTeePolicy {
                allowed_profiles: self.allowed_profiles,
                min_release_version: self.min_release_version,
            });
        SetTeeAdmissionPolicyApiRequest {
            allowed_mrtd: self.allowed_mrtd,
            allowed_rtmr0: self.allowed_rtmr0,
            allowed_rtmr1: self.allowed_rtmr1,
            allowed_rtmr2: self.allowed_rtmr2,
            allowed_rtmr3: self.allowed_rtmr3,
            allowed_tcb_statuses: self.allowed_tcb_statuses,
            accept_mock: self.accept_mock,
            signed_release,
            mode: match self.mode {
                TeeAdmissionModeArg::Replica => TeeAdmissionMode::Replica,
                TeeAdmissionModeArg::Relay => TeeAdmissionMode::Relay,
            },
            // The node signs the policy's root proof itself when it holds the
            // admin's account root, which is the case for a `merod init` node.
            root_proof: None,
        }
    }

    pub async fn run(self, environment: &mut Environment) -> Result<()> {
        let group_id = self.group_id.clone();
        let request = self.request();
        let client = environment.client()?;
        let response = client.set_tee_admission_policy(&group_id, request).await?;
        environment.output.write(&response);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use calimero_context_config::MemberCapabilities;
    use calimero_server_primitives::admin::TeeAdmissionMode;
    use clap::Parser;

    use super::{
        encode_default_capabilities, SetDefaultCapabilitiesCommand, SetTeeAdmissionPolicyCommand,
    };

    const GROUP: &str = "1111111111111111111111111111111111111111111111111111111111111111";

    /// `--mode relay` reaches the request, and leaving it out asks for a
    /// replica policy — the same default the API applies to an absent field.
    #[test]
    fn the_tee_policy_mode_flag_reaches_the_request() {
        let relay = SetTeeAdmissionPolicyCommand::try_parse_from([
            "set-tee-admission-policy",
            GROUP,
            "--profile",
            "locked-read-only",
            "--mode",
            "relay",
        ])
        .expect("parse");
        let req = relay.request();
        assert_eq!(req.mode, TeeAdmissionMode::Relay);
        assert_eq!(
            req.signed_release.map(|s| s.allowed_profiles),
            Some(vec!["locked-read-only".to_owned()])
        );

        let default = SetTeeAdmissionPolicyCommand::try_parse_from([
            "set-tee-admission-policy",
            GROUP,
            "--mrtd",
            "aa",
        ])
        .expect("parse");
        let req = default.request();
        assert_eq!(req.mode, TeeAdmissionMode::Replica);
        assert!(req.signed_release.is_none());
    }

    /// A command with every flag off, to be turned on by name.
    ///
    /// Built field-by-field rather than positionally, for the reason
    /// `encode_default_capabilities` documents: a positional constructor makes
    /// the same transposition the function guards against, so it cannot catch
    /// one.
    fn cmd() -> SetDefaultCapabilitiesCommand {
        SetDefaultCapabilitiesCommand {
            group_id: String::new(),
            can_create_context: false,
            can_invite_members: false,
            can_join_open_subgroups: false,
            can_create_subgroup: false,
            can_delete_subgroup: false,
            can_manage_visibility: false,
            can_manage_metadata: false,
            can_author_on_behalf: false,
        }
    }

    struct FlagCase {
        set: fn(&mut SetDefaultCapabilitiesCommand),
        expected: MemberCapabilities,
        name: &'static str,
    }

    #[test]
    fn each_flag_sets_exactly_its_own_bit() {
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
            FlagCase {
                set: |c| c.can_author_on_behalf = true,
                expected: MemberCapabilities::CAN_AUTHOR_ON_BEHALF,
                name: "can_author_on_behalf",
            },
        ];

        for case in cases {
            let mut c = cmd();
            (case.set)(&mut c);
            assert_eq!(
                encode_default_capabilities(&c),
                case.expected.bits(),
                "{} must set exactly its own bit and no other",
                case.name
            );
        }
    }

    /// The flag this command existed without, and the reason it was added.
    ///
    /// A namespace opened to a relay fleet needs `CAN_AUTHOR_ON_BEHALF` in its
    /// DEFAULT mask, and it has to survive alongside the
    /// `CAN_JOIN_OPEN_SUBGROUPS` a group is created with — the grant is resolved
    /// through the membership anchor, so a root grant only reaches the subgroups
    /// a member actually inherits into.
    #[test]
    fn the_fleet_mask_carries_both_bits() {
        let mut c = cmd();
        c.can_join_open_subgroups = true;
        c.can_author_on_behalf = true;

        let mask = encode_default_capabilities(&c);

        assert_eq!(
            mask,
            (MemberCapabilities::CAN_JOIN_OPEN_SUBGROUPS
                | MemberCapabilities::CAN_AUTHOR_ON_BEHALF)
                .bits()
        );
    }

    #[test]
    fn no_flags_is_an_empty_mask() {
        // The command REPLACES the mask, so this is how a group is closed again
        // rather than a no-op — worth pinning so it cannot quietly become one.
        assert_eq!(encode_default_capabilities(&cmd()), 0);
    }
}
