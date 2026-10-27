//! `meroctl namespace`: organizations, v1's namespaces. An organization holds
//! members (with roles), teams (`meroctl group`), invites and contexts.

use std::path::PathBuf;

use clap::{Subcommand, ValueEnum};
use serde_json::{json, Value};

use crate::env::{json_arg, load_dev_passkey, need_passkey, save_passkey, Env};

#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum Role {
    Guest,
    Member,
    Admin,
    /// Moves ownership to the member (the current owner's).
    Owner,
}

impl Role {
    pub fn wire(self) -> &'static str {
        match self {
            Self::Guest => "guest",
            Self::Member => "member",
            Self::Admin => "admin",
            Self::Owner => "owner",
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
    pub fn wire(self) -> &'static str {
        match self {
            Self::Open => "everyone",
            Self::Restricted => "invite",
        }
    }
}

#[derive(Debug, Subcommand)]
pub enum NamespaceCmd {
    Create {
        name: String,
    },
    Get {
        namespace: String,
    },
    /// The organizations this account belongs to, with its role.
    #[command(alias = "ls")]
    List,
    /// Delete the organization with its teams, invites and contexts (the owner's).
    Delete {
        namespace: String,
        #[arg(long)]
        dev_passkey: Option<PathBuf>,
    },
    /// Make an invitation token.
    Invite {
        namespace: String,
        #[arg(long, value_enum, default_value = "member")]
        role: Role,
        /// Teams the invitee joins too.
        #[arg(long = "group")]
        groups: Vec<String>,
        #[arg(long)]
        ttl_secs: Option<u64>,
        /// How many times it can be redeemed.
        #[arg(long)]
        uses: Option<u32>,
    },
    /// Join with an invitation token.
    Join {
        token: String,
    },
    /// Leave the organization.
    Leave {
        namespace: String,
    },
    /// The organization's teams.
    Groups {
        namespace: String,
    },
    /// Create a team.
    CreateGroup {
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
        namespace: String,
        account: String,
        metadata: String,
    },
    /// Set the capabilities (v1's `MemberCapabilities` bits) of a member, of
    /// a team's members (`--group`), or the organization's default.
    Capabilities {
        namespace: String,
        capabilities: u32,
        #[arg(long, conflicts_with = "group")]
        account: Option<String>,
        #[arg(long)]
        group: Option<String>,
    },
    /// Approve URL prefixes an application's publisher may call out to from
    /// the organization's contexts (the owner's, where the owner holds outbound approvals).
    Outbound {
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
    /// Approve a mero-tee release to hold the organization's contexts
    /// (where the owner holds release approvals).
    ApproveRelease {
        namespace: String,
        version: String,
        #[arg(long)]
        dev_passkey: Option<PathBuf>,
    },
}

#[derive(Debug, Subcommand)]
pub enum MembersCmd {
    #[command(alias = "ls")]
    List { namespace: String },
    /// Add a member, or change their role.
    #[command(alias = "set-role")]
    Add {
        namespace: String,
        account: String,
        #[arg(long, value_enum, default_value = "member")]
        role: Role,
    },
    #[command(alias = "rm")]
    Remove { namespace: String, account: String },
}

#[derive(Debug, Subcommand)]
pub enum MetadataCmd {
    Get {
        namespace: String,
    },
    /// Replace the metadata (JSON).
    Set {
        namespace: String,
        metadata: String,
    },
}

/// An organization's record (`GET /orgs/{org}`).
pub async fn org(env: &Env, namespace: &str) -> eyre::Result<Value> {
    Ok(env.client.get(&format!("orgs/{namespace}")).await?)
}

pub async fn run(cmd: NamespaceCmd, env: &Env) -> eyre::Result<Value> {
    let client = &env.client;
    Ok(match cmd {
        NamespaceCmd::Create { name } => client.post("orgs", json!({ "name": name })).await?,
        NamespaceCmd::Get { namespace } => client.get(&format!("orgs/{namespace}")).await?,
        NamespaceCmd::List => json!({ "namespaces": client.me().await?["orgs"] }),
        NamespaceCmd::Delete {
            namespace,
            dev_passkey,
        } => {
            let file = need_passkey(dev_passkey.as_ref(), "deleting an organization")?;
            let mut pk = load_dev_passkey(file)?;
            let out = client.delete_org(&namespace, &mut pk).await;
            save_passkey(file, &pk)?;
            out?
        }
        NamespaceCmd::Invite {
            namespace,
            role,
            groups,
            ttl_secs,
            uses,
        } => client
            .post(
                &format!("orgs/{namespace}/invites"),
                json!({ "role": role.wire(), "teams": groups, "ttlSecs": ttl_secs, "uses": uses }),
            )
            .await?,
        NamespaceCmd::Join { token } => {
            client
                .post(&format!("invites/{token}/redeem"), json!({}))
                .await?
        }
        NamespaceCmd::Leave { namespace } => {
            let me = client.me().await?;
            let account = me["accountId"].as_str().unwrap_or_default();
            client
                .delete(&format!("orgs/{namespace}/members/{account}"))
                .await?
        }
        NamespaceCmd::Groups { namespace } => {
            json!({ "groups": org(env, &namespace).await?["teams"] })
        }
        NamespaceCmd::CreateGroup {
            namespace,
            name,
            parent,
            join,
        } => {
            client
                .post(
                    &format!("orgs/{namespace}/teams"),
                    json!({ "name": name, "parent": parent, "join": join.wire() }),
                )
                .await?
        }
        NamespaceCmd::Members(MembersCmd::List { namespace }) => {
            json!({ "members": org(env, &namespace).await?["members"] })
        }
        NamespaceCmd::Members(MembersCmd::Add {
            namespace,
            account,
            role,
        }) => {
            client
                .post(
                    &format!("orgs/{namespace}/members"),
                    json!({ "account": account, "role": role.wire() }),
                )
                .await?
        }
        NamespaceCmd::Members(MembersCmd::Remove { namespace, account }) => {
            client
                .delete(&format!("orgs/{namespace}/members/{account}"))
                .await?
        }
        NamespaceCmd::Metadata(MetadataCmd::Get { namespace }) => {
            json!({ "metadata": org(env, &namespace).await?["metadata"] })
        }
        NamespaceCmd::Metadata(MetadataCmd::Set {
            namespace,
            metadata,
        }) => {
            client
                .set_org_metadata(&namespace, json_arg(&metadata)?)
                .await?
        }
        NamespaceCmd::MemberMetadata {
            namespace,
            account,
            metadata,
        } => {
            client
                .set_member_metadata(&namespace, &account, json_arg(&metadata)?)
                .await?
        }
        NamespaceCmd::Capabilities {
            namespace,
            capabilities,
            account,
            group,
        } => {
            client
                .post(
                    &format!("orgs/{namespace}/capabilities"),
                    json!({ "account": account, "team": group, "capabilities": capabilities }),
                )
                .await?
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
    })
}

/// The owner approves outbound prefixes (an org's, or the account's own).
pub async fn outbound(
    env: &Env,
    namespace: Option<&str>,
    package: &str,
    prefixes: &[String],
    replace: bool,
    dev_passkey: Option<PathBuf>,
) -> eyre::Result<Value> {
    let file = need_passkey(dev_passkey.as_ref(), "an outbound approval")?;
    let mut pk = load_dev_passkey(file)?;
    let prefixes = (!prefixes.is_empty()).then_some(prefixes);
    let out = env
        .client
        .approve_outbound(namespace, package, prefixes, replace, &mut pk)
        .await;
    save_passkey(file, &pk)?;
    Ok(out?)
}

/// The owner approves a mero-tee release (an org's apps, or the account's own).
pub async fn approve_release(
    env: &Env,
    namespace: Option<&str>,
    version: &str,
    dev_passkey: Option<PathBuf>,
) -> eyre::Result<Value> {
    let file = need_passkey(dev_passkey.as_ref(), "a release approval")?;
    let mut pk = load_dev_passkey(file)?;
    let out = env
        .client
        .approve_release(namespace, version, &mut pk)
        .await;
    save_passkey(file, &pk)?;
    Ok(out?)
}
