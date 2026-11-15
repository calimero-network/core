//! `meroctl group`: an organization's teams, v1's groups (nested, open or
//! restricted), and v1's group upgrades (`group upgrade`), which move an
//! organization's (or one team's) contexts to a release.

use std::path::PathBuf;

use calimero_client::types::{OrgUpgradeRequest, Team, TeamSetMemberRequest};
use clap::{Subcommand, ValueEnum};
use serde_json::{json, Value};

use crate::env::{json_arg, to_json, DevPasskey, Env};
use crate::namespace::{org, Join};

#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum TeamRole {
    Lead,
    Member,
    ReadOnly,
}

impl TeamRole {
    fn wire(self) -> calimero_client::types::TeamRole {
        match self {
            Self::Lead => calimero_client::types::TeamRole::Lead,
            Self::Member => calimero_client::types::TeamRole::Member,
            Self::ReadOnly => calimero_client::types::TeamRole::ReadOnly,
        }
    }
}

#[derive(Debug, Subcommand)]
pub enum GroupCmd {
    Get {
        namespace: String,
        group: String,
    },
    /// Rename a team or change who may join it.
    Update {
        namespace: String,
        group: String,
        #[arg(long)]
        name: Option<String>,
        #[arg(long, value_enum)]
        join: Option<Join>,
    },
    /// Delete a team with the teams below it and their instances (with
    /// instances there, approved with a passkey).
    Delete {
        namespace: String,
        group: String,
        /// Approve with this passkey file (development clusters).
        #[arg(long)]
        dev_passkey: Option<PathBuf>,
    },
    /// Move a team under another (`--top`: to the top of the organization).
    Reparent {
        namespace: String,
        group: String,
        #[arg(required_unless_present = "top")]
        parent: Option<String>,
        #[arg(long, conflicts_with = "parent")]
        top: bool,
    },
    /// The teams directly below one.
    Subgroups {
        namespace: String,
        group: String,
    },
    #[command(subcommand)]
    Members(MembersCmd),
    /// Set the team's metadata (JSON).
    Metadata {
        namespace: String,
        group: String,
        metadata: String,
    },
    /// Set the capabilities every member of the team gets.
    Settings {
        namespace: String,
        group: String,
        #[arg(long)]
        capabilities: u32,
        /// Approve with this passkey file (development clusters).
        #[arg(long)]
        dev_passkey: Option<PathBuf>,
    },
    /// Leave the team.
    Leave {
        namespace: String,
        group: String,
        /// Approve with this passkey file (development clusters).
        #[arg(long)]
        dev_passkey: Option<PathBuf>,
    },
    /// v1's group upgrades: move the organization's instances (or one team's,
    /// `--group`, with `--cascade` its subtree) to a release, then follow and retry.
    #[command(subcommand)]
    Upgrade(UpgradeCmd),
}

#[derive(Debug, Subcommand)]
pub enum MembersCmd {
    #[command(alias = "ls")]
    List { namespace: String, group: String },
    /// Add a member, or change their role or capabilities.
    #[command(alias = "set-role")]
    Add {
        namespace: String,
        group: String,
        account: String,
        #[arg(long, value_enum, default_value = "member")]
        role: TeamRole,
        #[arg(long)]
        capabilities: Option<u32>,
        /// Approve with this passkey file (development clusters).
        #[arg(long)]
        dev_passkey: Option<PathBuf>,
    },
    #[command(alias = "rm")]
    Remove {
        namespace: String,
        group: String,
        account: String,
        /// Approve with this passkey file (development clusters).
        #[arg(long)]
        dev_passkey: Option<PathBuf>,
    },
}

#[derive(Debug, Subcommand)]
pub enum UpgradeCmd {
    Trigger {
        namespace: String,
        #[arg(long)]
        package: String,
        #[arg(long)]
        version: String,
        #[arg(long)]
        group: Option<String>,
        #[arg(long, requires = "group")]
        cascade: bool,
        #[arg(long)]
        migrate: Option<String>,
        /// The organization owner's passkey, approving the whole run.
        #[arg(long)]
        dev_passkey: Option<PathBuf>,
    },
    Status {
        namespace: String,
        run: String,
    },
    /// Run the failed instances of a run again, as a new run.
    Retry {
        namespace: String,
        run: String,
        #[arg(long)]
        dev_passkey: Option<PathBuf>,
    },
}

pub async fn run(cmd: GroupCmd, env: &Env) -> eyre::Result<Value> {
    let orgs = env.client.orgs();
    Ok(match cmd {
        GroupCmd::Get { namespace, group } => to_json(&orgs.team_get(&namespace, &group).await?)?,
        GroupCmd::Update {
            namespace,
            group,
            name,
            join,
        } => to_json(
            &orgs
                .team_update(
                    &namespace,
                    &group,
                    name.as_deref(),
                    join.map(Join::wire),
                    None,
                    None,
                )
                .await?,
        )?,
        GroupCmd::Delete {
            namespace,
            group,
            dev_passkey,
        } => {
            let mut pk = DevPasskey::load(dev_passkey.as_deref())?;
            let out = orgs.team_delete(&namespace, &group, pk.get()).await;
            pk.save()?;
            to_json(&out?)?
        }
        GroupCmd::Reparent {
            namespace,
            group,
            parent,
            top: _,
        } => {
            // `parent: null` moves it to the top.
            to_json(
                &orgs
                    .team_update(
                        &namespace,
                        &group,
                        None,
                        None,
                        Some(parent.as_deref()),
                        None,
                    )
                    .await?,
            )?
        }
        GroupCmd::Subgroups { namespace, group } => {
            let below: Vec<Team> = org(env, &namespace)
                .await?
                .teams
                .into_values()
                .filter(|t| t.parent.as_deref() == Some(group.as_str()))
                .collect();
            json!({ "subgroups": below })
        }
        GroupCmd::Members(MembersCmd::List { namespace, group }) => {
            let team = orgs.team_get(&namespace, &group).await?;
            json!({ "members": team.members, "readOnly": team.extra.get("readOnly") })
        }
        GroupCmd::Members(MembersCmd::Add {
            namespace,
            group,
            account,
            role,
            capabilities,
            dev_passkey,
        }) => {
            let body = TeamSetMemberRequest {
                account,
                role: Some(role.wire()),
                capabilities: capabilities.map(u64::from),
                ..TeamSetMemberRequest::default()
            };
            let mut pk = DevPasskey::load(dev_passkey.as_deref())?;
            let out = orgs
                .team_set_member(&namespace, &group, body, pk.get())
                .await;
            pk.save()?;
            to_json(&out?)?
        }
        GroupCmd::Members(MembersCmd::Remove {
            namespace,
            group,
            account,
            dev_passkey,
        }) => {
            let mut pk = DevPasskey::load(dev_passkey.as_deref())?;
            let out = orgs
                .team_remove_member(&namespace, &group, &account, pk.get())
                .await;
            pk.save()?;
            to_json(&out?)?
        }
        GroupCmd::Metadata {
            namespace,
            group,
            metadata,
        } => to_json(
            &orgs
                .team_update(
                    &namespace,
                    &group,
                    None,
                    None,
                    None,
                    Some(json_arg(&metadata)?),
                )
                .await?,
        )?,
        GroupCmd::Settings {
            namespace,
            group,
            capabilities,
            dev_passkey,
        } => {
            let mut pk = DevPasskey::load(dev_passkey.as_deref())?;
            let out = orgs
                .capabilities(
                    &namespace,
                    u64::from(capabilities),
                    None,
                    Some(&group),
                    pk.get(),
                )
                .await;
            pk.save()?;
            to_json(&out?)?
        }
        GroupCmd::Leave {
            namespace,
            group,
            dev_passkey,
        } => {
            let account = env.client.auth().me().await?.account_id;
            let mut pk = DevPasskey::load(dev_passkey.as_deref())?;
            let out = orgs
                .team_remove_member(&namespace, &group, &account, pk.get())
                .await;
            pk.save()?;
            to_json(&out?)?
        }
        GroupCmd::Upgrade(UpgradeCmd::Trigger {
            namespace,
            package,
            version,
            group,
            cascade,
            migrate,
            dev_passkey,
        }) => {
            let body = OrgUpgradeRequest {
                package,
                version,
                team: group,
                cascade: Some(cascade),
                migrate,
                operator_proof: None,
            };
            let mut pk = DevPasskey::load(dev_passkey.as_deref())?;
            let out = orgs.upgrade(&namespace, body, pk.get()).await;
            pk.save()?;
            to_json(&out?)?
        }
        GroupCmd::Upgrade(UpgradeCmd::Status { namespace, run }) => {
            to_json(&orgs.upgrade_status(&namespace, &run).await?)?
        }
        GroupCmd::Upgrade(UpgradeCmd::Retry {
            namespace,
            run,
            dev_passkey,
        }) => {
            let mut pk = DevPasskey::load(dev_passkey.as_deref())?;
            let out = orgs.upgrade_retry(&namespace, &run, pk.get(), None).await;
            pk.save()?;
            to_json(&out?)?
        }
    })
}
