//! `meroctl namespace`: organizations, v1's namespaces. An organization holds
//! members (with roles), teams (`meroctl group`), invites and contexts.

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
        namespace: String,
        /// Approve with this passkey file (development clusters).
        #[arg(long)]
        dev_passkey: Option<PathBuf>,
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
        /// Approve with this passkey file (development clusters).
        #[arg(long)]
        dev_passkey: Option<PathBuf>,
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
    List {
        namespace: String,
        /// Each member's lockout risk instead (owners and admins): passkeys
        /// held (to "3+"), recovery code set, last active day, live sessions.
        #[arg(long)]
        security: bool,
    },
    /// Add a member, or change their role.
    #[command(alias = "set-role")]
    Add {
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
        namespace: String,
    },
    /// Replace the metadata (JSON).
    Set {
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

/// The owner approves outbound prefixes (an org's, or the account's own).
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
    let out = match namespace {
        Some(ns) => {
            env.client
                .orgs()
                .outbound_approval(ns, package, selection, Some(replace), pk.get())
                .await
        }
        None => {
            env.client
                .auth()
                .account_outbound_approval(package, selection, Some(replace), pk.get())
                .await
        }
    };
    pk.save()?;
    to_json(&out?)
}

/// The owner approves a mero-tee release (an org's apps, or the account's own).
pub async fn approve_release(
    env: &Env,
    namespace: Option<&str>,
    version: &str,
    dev_passkey: Option<PathBuf>,
) -> eyre::Result<Value> {
    let mut pk = DevPasskey::load(dev_passkey.as_deref())?;
    let out = match namespace {
        Some(ns) => {
            env.client
                .orgs()
                .release_approval(ns, version, None, pk.get())
                .await
        }
        None => {
            env.client
                .auth()
                .account_release_approval(version, None, pk.get())
                .await
        }
    };
    pk.save()?;
    to_json(&out?)
}
