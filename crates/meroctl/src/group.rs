//! `meroctl group`: an organization's teams, v1's groups (nested, open or
//! restricted), and v1's group upgrades (`group upgrade`), which move an
//! organization's (or one team's) contexts to a release.

use std::path::PathBuf;

use clap::{Subcommand, ValueEnum};
use serde_json::{json, Value};

use crate::env::{json_arg, Env};
use crate::namespace::{org, Join};

#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum TeamRole {
    Lead,
    Member,
    ReadOnly,
}

impl TeamRole {
    fn wire(self) -> &'static str {
        match self {
            Self::Lead => "lead",
            Self::Member => "member",
            Self::ReadOnly => "readOnly",
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
    /// Delete a team with the teams below it and their contexts.
    Delete {
        namespace: String,
        group: String,
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
    /// v1's group upgrades: move the organization's contexts (or one team's,
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
    /// Run the failed contexts of a run again, as a new run.
    Retry {
        namespace: String,
        run: String,
        #[arg(long)]
        dev_passkey: Option<PathBuf>,
    },
}

pub async fn run(cmd: GroupCmd, env: &Env) -> eyre::Result<Value> {
    let client = &env.client;
    Ok(match cmd {
        GroupCmd::Get { namespace, group } => {
            client
                .get(&format!("orgs/{namespace}/teams/{group}"))
                .await?
        }
        GroupCmd::Update {
            namespace,
            group,
            name,
            join,
        } => {
            let mut body = json!({});
            if let Some(n) = name {
                body["name"] = json!(n);
            }
            if let Some(j) = join {
                body["join"] = json!(j.wire());
            }
            client
                .post(&format!("orgs/{namespace}/teams/{group}"), body)
                .await?
        }
        GroupCmd::Delete { namespace, group } => {
            client
                .delete(&format!("orgs/{namespace}/teams/{group}"))
                .await?
        }
        GroupCmd::Reparent {
            namespace,
            group,
            parent,
            top: _,
        } => {
            // `parent: null` moves it to the top.
            client
                .post(
                    &format!("orgs/{namespace}/teams/{group}"),
                    json!({ "parent": parent }),
                )
                .await?
        }
        GroupCmd::Subgroups { namespace, group } => {
            let teams = org(env, &namespace).await?["teams"].clone();
            let below: Vec<Value> = teams
                .as_object()
                .map(|t| {
                    t.values()
                        .filter(|t| t["parent"].as_str() == Some(group.as_str()))
                        .cloned()
                        .collect()
                })
                .unwrap_or_default();
            json!({ "subgroups": below })
        }
        GroupCmd::Members(MembersCmd::List { namespace, group }) => {
            let team = client
                .get(&format!("orgs/{namespace}/teams/{group}"))
                .await?;
            let team = if team.get("team").is_some() {
                team["team"].clone()
            } else {
                team
            };
            json!({ "members": team["members"], "readOnly": team["readOnly"] })
        }
        GroupCmd::Members(MembersCmd::Add {
            namespace,
            group,
            account,
            role,
            capabilities,
            dev_passkey,
        }) => {
            env.post_approved(
                &format!("orgs/{namespace}/teams/{group}/members"),
                json!({ "account": account, "role": role.wire(), "capabilities": capabilities }),
                dev_passkey.as_deref(),
            )
            .await?
        }
        GroupCmd::Members(MembersCmd::Remove {
            namespace,
            group,
            account,
            dev_passkey,
        }) => {
            env.delete_approved(
                &format!("orgs/{namespace}/teams/{group}/members/{account}"),
                dev_passkey.as_deref(),
            )
            .await?
        }
        GroupCmd::Metadata {
            namespace,
            group,
            metadata,
        } => {
            client
                .post(
                    &format!("orgs/{namespace}/teams/{group}"),
                    json!({ "metadata": json_arg(&metadata)? }),
                )
                .await?
        }
        GroupCmd::Settings {
            namespace,
            group,
            capabilities,
            dev_passkey,
        } => {
            env.post_approved(
                &format!("orgs/{namespace}/capabilities"),
                json!({ "team": group, "capabilities": capabilities }),
                dev_passkey.as_deref(),
            )
            .await?
        }
        GroupCmd::Leave {
            namespace,
            group,
            dev_passkey,
        } => {
            let me = client.me().await?;
            let account = me["accountId"].as_str().unwrap_or_default();
            env.delete_approved(
                &format!("orgs/{namespace}/teams/{group}/members/{account}"),
                dev_passkey.as_deref(),
            )
            .await?
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
            env.post_approved(
                &format!("orgs/{namespace}/upgrade"),
                json!({
                    "package": package, "version": version, "team": group,
                    "cascade": cascade, "migrate": migrate,
                }),
                dev_passkey.as_deref(),
            )
            .await?
        }
        GroupCmd::Upgrade(UpgradeCmd::Status { namespace, run }) => {
            client.upgrade_run(&namespace, &run).await?
        }
        GroupCmd::Upgrade(UpgradeCmd::Retry {
            namespace,
            run,
            dev_passkey,
        }) => {
            env.post_approved(
                &format!("orgs/{namespace}/upgrades/{run}/retry"),
                json!({}),
                dev_passkey.as_deref(),
            )
            .await?
        }
    })
}
