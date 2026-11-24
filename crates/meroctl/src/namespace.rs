//! `meroctl org`: organizations, v1's namespaces. An organization holds
//! members (with roles), teams (`meroctl team`), invites and instances.

use std::path::PathBuf;

use calimero_client::types::{
    InviteCreateRequest, Org, OrgMembersDetail, OrgRole, OutboundSelection,
    OutboundSelectionManifest, TeamJoin,
};
use clap::{Subcommand, ValueEnum};
use serde_json::{json, Value};

use crate::env::{json_arg, to_json, DevPasskey, Env};

#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum Role {
    Guest,
    Member,
    Admin,
    /// Moves ownership to the member (the current owner's).
    Owner,
}

impl Role {
    pub fn wire(self) -> OrgRole {
        match self {
            Self::Guest => OrgRole::Guest,
            Self::Member => OrgRole::Member,
            Self::Admin => OrgRole::Admin,
            Self::Owner => OrgRole::Owner,
        }
    }
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum Join {
    /// Whoever reaches the parent is a member (v1's open subgroup).
    Open,
    /// Only the members added (v1's restricted subgroup).
    Restricted,
}

impl Join {
    pub fn wire(self) -> TeamJoin {
        match self {
            Self::Open => TeamJoin::Everyone,
            Self::Restricted => TeamJoin::Invite,
        }
    }
}

#[derive(Debug, Subcommand)]
pub enum NamespaceCmd {
    Create {
        name: String,
    },
    Get {
        #[arg(value_name = "ORG")]
        namespace: String,
    },
    /// The organizations this account belongs to, with its role.
    #[command(alias = "ls")]
    List,
    /// Delete the organization with its teams, invites and instances (the owner's).
    Delete {
        #[arg(value_name = "ORG")]
        namespace: String,
        #[arg(long)]
        dev_passkey: Option<PathBuf>,
    },
    /// Make an invitation token.
    Invite {
        #[arg(value_name = "ORG")]
        namespace: String,
        #[arg(long, value_enum, default_value = "member")]
        role: Role,
        /// Teams the invitee joins too.
        #[arg(long = "team", value_name = "TEAM")]
        groups: Vec<String>,
        #[arg(long)]
        ttl_secs: Option<u64>,
        /// How many times it can be redeemed.
        #[arg(long)]
        uses: Option<u32>,
        /// Approve with this passkey file (development clusters).
        #[arg(long)]
        dev_passkey: Option<PathBuf>,
    },
    /// Join with an invitation token.
    Join {
        token: String,
    },
    /// Leave the organization.
    Leave {
        #[arg(value_name = "ORG")]
        namespace: String,
        /// Approve with this passkey file (development clusters).
        #[arg(long)]
        dev_passkey: Option<PathBuf>,
    },
    /// The organization's teams.
    #[command(name = "teams")]
    Groups {
        #[arg(value_name = "ORG")]
        namespace: String,
    },
    /// Create a team.
    #[command(name = "create-team")]
    CreateGroup {
        #[arg(value_name = "ORG")]
        namespace: String,
        name: String,
        /// Nest it under this team.
        #[arg(long)]
        parent: Option<String>,
        #[arg(long, value_enum, default_value = "restricted")]
        join: Join,
    },
    #[command(subcommand)]
    Members(MembersCmd),
    /// The organization's metadata record.
    #[command(subcommand)]
    Metadata(MetadataCmd),
    /// Set a member's metadata (theirs, or an admin's); `null` clears it.
    MemberMetadata {
        #[arg(value_name = "ORG")]
        namespace: String,
        account: String,
        metadata: String,
    },
    /// Set the capabilities (v1's `MemberCapabilities` bits) of a member, of
    /// a team's members (`--team`), or the organization's default.
    Capabilities {
        #[arg(value_name = "ORG")]
        namespace: String,
        capabilities: u32,
        #[arg(long, conflicts_with = "group")]
        account: Option<String>,
        #[arg(long = "team", value_name = "TEAM")]
        group: Option<String>,
        /// Approve with this passkey file (development clusters).
        #[arg(long)]
        dev_passkey: Option<PathBuf>,
    },
    /// Approve URL prefixes an application's publisher may call out to from
    /// the organization's instances (the owner's, where the owner holds outbound approvals).
    Outbound {
        #[arg(value_name = "ORG")]
        namespace: String,
        #[arg(long)]
        package: String,
        /// The prefixes (default: every prefix its latest release declares).
        #[arg(long = "prefix")]
        prefixes: Vec<String>,
        /// Set exactly these prefixes (how one is revoked).
        #[arg(long)]
        replace: bool,
        #[arg(long)]
        dev_passkey: Option<PathBuf>,
    },
    /// The organization's update policy (the one its instances without their own
    /// follow); with a flag, set it (its owner or an admin, with a passkey).
    UpgradePolicy {
        #[arg(value_name = "ORG")]
        namespace: String,
        #[command(flatten)]
        policy: PolicyArgs,
    },
    /// The organization's instances with an update available or a first approval
    /// waiting (its owner and admins).
    Updates {
        #[arg(value_name = "ORG")]
        namespace: String,
        /// The page after this instance id.
        #[arg(long)]
        after: Option<String>,
    },
    /// Approve a mero-tee release to hold the organization's instances
    /// (where the owner holds release approvals).
    ApproveRelease {
        #[arg(value_name = "ORG")]
        namespace: String,
        version: String,
        #[arg(long)]
        dev_passkey: Option<PathBuf>,
    },
}

/// `--auto-apply`: which updates apply on their own after the hold.
#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum AutoApply {
    Never,
    NoDestinations,
}

/// An update policy's changes (`org upgrade-policy`, `instance upgrade-policy`): none
/// reads it; any sets it, starting from the one set at that scope (else the one in force).
#[derive(Debug, clap::Args)]
pub struct PolicyArgs {
    /// A red update needs two distinct owners or admins.
    #[arg(long)]
    second_approver: Option<bool>,
    /// Which updates apply on their own after the hold: `never`, or `no-destinations` (a
    /// green update that removes no method and moves none of the apps it created, of an app
    /// whose unit sends data nowhere outside Calimero, before or after).
    #[arg(long, value_enum)]
    auto_apply: Option<AutoApply>,
    /// How long a green update waits before applying on its own: 24 to 168 hours.
    #[arg(long)]
    hold_hours: Option<u32>,
    /// Drop the policy set at this scope.
    #[arg(long, conflicts_with_all = ["second_approver", "auto_apply", "hold_hours"])]
    clear: bool,
    #[arg(long)]
    dev_passkey: Option<PathBuf>,
}

impl PolicyArgs {
    /// The policy to send from `view` (`GET .../upgrade-policy`), `None` for a read;
    /// `Some(None)` drops it.
    pub fn change<T: serde::de::DeserializeOwned>(
        &self,
        view: &Value,
    ) -> eyre::Result<Option<Option<T>>> {
        if self.clear {
            return Ok(Some(None));
        }
        if self.second_approver.is_none() && self.auto_apply.is_none() && self.hold_hours.is_none()
        {
            return Ok(None);
        }
        let mut policy = if view["own"].is_null() {
            view["policy"].clone()
        } else {
            view["own"].clone()
        };
        if let Some(on) = self.second_approver {
            policy["secondApprover"] = json!(on);
        }
        if let Some(auto) = self.auto_apply {
            policy["autoApply"] = json!(match auto {
                AutoApply::Never => "never",
                AutoApply::NoDestinations => "noDestinations",
            });
        }
        if let Some(hours) = self.hold_hours {
            policy["holdHours"] = json!(hours);
        }
        Ok(Some(Some(serde_json::from_value(policy)?)))
    }

    pub fn passkey(&self) -> eyre::Result<DevPasskey> {
        DevPasskey::load(self.dev_passkey.as_deref())
    }
}

#[derive(Debug, Subcommand)]
pub enum MembersCmd {
    #[command(alias = "ls")]
    List {
        #[arg(value_name = "ORG")]
        namespace: String,
        /// Each member's lockout risk instead (owners and admins): passkeys
        /// held (to "3+"), recovery code set, last active day, live sessions.
        #[arg(long)]
        security: bool,
    },
    /// Add a member, or change their role.
    #[command(alias = "set-role")]
    Add {
        #[arg(value_name = "ORG")]
        namespace: String,
        account: String,
        #[arg(long, value_enum, default_value = "member")]
        role: Role,
        /// Approve with this passkey file (development clusters).
        #[arg(long)]
        dev_passkey: Option<PathBuf>,
    },
    #[command(alias = "rm")]
    Remove {
        #[arg(value_name = "ORG")]
        namespace: String,
        account: String,
        /// Approve with this passkey file (development clusters).
        #[arg(long)]
        dev_passkey: Option<PathBuf>,
    },
}

#[derive(Debug, Subcommand)]
pub enum MetadataCmd {
    Get {
        #[arg(value_name = "ORG")]
        namespace: String,
    },
    /// Replace the metadata (JSON).
    Set {
        #[arg(value_name = "ORG")]
        namespace: String,
        metadata: String,
    },
}

/// An organization's record (`GET /orgs/{org}`).
pub async fn org(env: &Env, namespace: &str) -> eyre::Result<Org> {
    Ok(env.client.orgs().get(namespace).await?)
}

pub async fn run(cmd: NamespaceCmd, env: &Env) -> eyre::Result<Value> {
    let orgs = env.client.orgs();
    Ok(match cmd {
        NamespaceCmd::Create { name } => to_json(&orgs.create(&name, None, None).await?)?,
        NamespaceCmd::Get { namespace } => to_json(&org(env, &namespace).await?)?,
        NamespaceCmd::List => json!({ "namespaces": env.client.auth().me().await?.orgs }),
        NamespaceCmd::Delete {
            namespace,
            dev_passkey,
        } => {
            let mut pk = DevPasskey::load(dev_passkey.as_deref())?;
            let out = orgs.delete(&namespace, pk.get()).await;
            pk.save()?;
            to_json(&out?)?
        }
        NamespaceCmd::Invite {
            namespace,
            role,
            groups,
            ttl_secs,
            uses,
            dev_passkey,
        } => {
            let body = InviteCreateRequest {
                role: role.wire(),
                teams: Some(groups),
                ttl_secs,
                uses: uses.map(u64::from),
                all_apps: None,
                capabilities: None,
            };
            let mut pk = DevPasskey::load(dev_passkey.as_deref())?;
            let out = orgs.invite_create(&namespace, body, pk.get()).await;
            pk.save()?;
            to_json(&out?)?
        }
        NamespaceCmd::Join { token } => to_json(&orgs.invite_redeem(&token).await?)?,
        NamespaceCmd::Leave {
            namespace,
            dev_passkey,
        } => {
            let account = env.client.auth().me().await?.account_id;
            let mut pk = DevPasskey::load(dev_passkey.as_deref())?;
            let out = orgs.remove_member(&namespace, &account, pk.get()).await;
            pk.save()?;
            to_json(&out?)?
        }
        NamespaceCmd::Groups { namespace } => {
            json!({ "groups": org(env, &namespace).await?.teams })
        }
        NamespaceCmd::CreateGroup {
            namespace,
            name,
            parent,
            join,
        } => to_json(
            &orgs
                .team_create(&namespace, &name, join.wire(), parent.as_deref())
                .await?,
        )?,
        NamespaceCmd::Members(MembersCmd::List {
            namespace,
            security: false,
        }) => {
            json!({ "members": org(env, &namespace).await?.members })
        }
        NamespaceCmd::Members(MembersCmd::List {
            namespace,
            security: true,
        }) => to_json(
            &orgs
                .members(&namespace, Some(OrgMembersDetail::Security))
                .await?,
        )?,
        NamespaceCmd::Members(MembersCmd::Add {
            namespace,
            account,
            role,
            dev_passkey,
        }) => {
            let mut pk = DevPasskey::load(dev_passkey.as_deref())?;
            let out = orgs
                .set_member(&namespace, &account, role.wire(), pk.get())
                .await;
            pk.save()?;
            to_json(&out?)?
        }
        NamespaceCmd::Members(MembersCmd::Remove {
            namespace,
            account,
            dev_passkey,
        }) => {
            let mut pk = DevPasskey::load(dev_passkey.as_deref())?;
            let out = orgs.remove_member(&namespace, &account, pk.get()).await;
            pk.save()?;
            to_json(&out?)?
        }
        NamespaceCmd::Metadata(MetadataCmd::Get { namespace }) => {
            json!({ "metadata": org(env, &namespace).await?.metadata })
        }
        NamespaceCmd::Metadata(MetadataCmd::Set {
            namespace,
            metadata,
        }) => to_json(&orgs.metadata(&namespace, json_arg(&metadata)?).await?)?,
        NamespaceCmd::MemberMetadata {
            namespace,
            account,
            metadata,
        } => to_json(
            &orgs
                .member_metadata(&namespace, &account, json_arg(&metadata)?)
                .await?,
        )?,
        NamespaceCmd::Capabilities {
            namespace,
            capabilities,
            account,
            group,
            dev_passkey,
        } => {
            let mut pk = DevPasskey::load(dev_passkey.as_deref())?;
            let out = orgs
                .capabilities(
                    &namespace,
                    u64::from(capabilities),
                    account.as_deref(),
                    group.as_deref(),
                    pk.get(),
                )
                .await;
            pk.save()?;
            to_json(&out?)?
        }
        NamespaceCmd::Outbound {
            namespace,
            package,
            prefixes,
            replace,
            dev_passkey,
        } => {
            outbound(
                env,
                Some(&namespace),
                &package,
                &prefixes,
                replace,
                dev_passkey,
            )
            .await?
        }
        NamespaceCmd::ApproveRelease {
            namespace,
            version,
            dev_passkey,
        } => approve_release(env, Some(&namespace), &version, dev_passkey).await?,
        NamespaceCmd::UpgradePolicy { namespace, policy } => {
            let view = to_json(&orgs.policy(&namespace).await?)?;
            match policy.change(&view)? {
                None => view,
                Some(change) => {
                    let mut pk = policy.passkey()?;
                    let out = orgs.policy_set(&namespace, Some(change), pk.get()).await;
                    pk.save()?;
                    to_json(&out?)?
                }
            }
        }
        NamespaceCmd::Updates { namespace, after } => {
            to_json(&orgs.updates(&namespace, after.as_deref()).await?)?
        }
    })
}

/// The prefixes of an outbound approval: those named, else every prefix the
/// package's latest release declares (`"manifest"`).
pub fn prefixes(named: &[String]) -> OutboundSelection {
    if named.is_empty() {
        OutboundSelection::Manifest(OutboundSelectionManifest::Manifest)
    } else {
        OutboundSelection::List(named.to_vec())
    }
}

/// The owner approves outbound prefixes (an org's, or the account's own: its
/// only organization's).
pub async fn outbound(
    env: &Env,
    namespace: Option<&str>,
    package: &str,
    named: &[String],
    replace: bool,
    dev_passkey: Option<PathBuf>,
) -> eyre::Result<Value> {
    let selection = Some(prefixes(named));
    let mut pk = DevPasskey::load(dev_passkey.as_deref())?;
    // The account's own apps are its only organization's.
    let ns = match namespace {
        Some(ns) => ns.to_owned(),
        None => env.client.sole_org().await?,
    };
    let out = env
        .client
        .orgs()
        .outbound_approval(&ns, package, selection, Some(replace), pk.get())
        .await;
    pk.save()?;
    to_json(&out?)
}

/// The owner approves a mero-tee release (an org's apps, or the account's own:
/// its only organization's).
pub async fn approve_release(
    env: &Env,
    namespace: Option<&str>,
    version: &str,
    dev_passkey: Option<PathBuf>,
) -> eyre::Result<Value> {
    let mut pk = DevPasskey::load(dev_passkey.as_deref())?;
    // The account's own apps are its only organization's.
    let ns = match namespace {
        Some(ns) => ns.to_owned(),
        None => env.client.sole_org().await?,
    };
    let out = env
        .client
        .orgs()
        .release_approval(&ns, version, None, pk.get())
        .await;
    pk.save()?;
    to_json(&out?)
}
