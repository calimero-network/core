//! `meroctl instance`: app instances, v1's contexts. A context runs one
//! application release (or, on a development cluster, a raw bundle), on the
//! replicas the cluster places it on.

use std::path::PathBuf;

use calimero_client::types::{AppCreateRequest, AppUpgradeRequest, Grant};
use clap::Subcommand;
use futures_util::StreamExt;
use serde_json::{json, Value};

use crate::env::{json_arg, to_json, DevPasskey, Env};

#[derive(Debug, Subcommand)]
pub enum ContextCmd {
    /// Create an instance from an installed application (`--package`), a
    /// bundle hash, or a local file (`--path`: installed first).
    Create {
        /// The instance's name.
        #[arg(long)]
        name: String,
        #[arg(long, conflicts_with_all = ["bundle", "path"])]
        package: Option<String>,
        /// The release; default the newest installed.
        #[arg(long, requires = "package")]
        version: Option<String>,
        /// The service, for a multi-service application.
        #[arg(long)]
        service: Option<String>,
        #[arg(long, conflicts_with = "path")]
        bundle: Option<String>,
        /// A `.mpk` or (development) `.wasm` to install first.
        #[arg(long)]
        path: Option<PathBuf>,
        /// Arguments for the app's `init`, JSON.
        #[arg(long)]
        params: Option<String>,
        /// Create it in this organization.
        #[arg(long = "org", value_name = "ORG")]
        namespace: Option<String>,
        /// Reached by this team's members (a team of `--org`).
        #[arg(long = "team", value_name = "TEAM", requires = "namespace")]
        group: Option<String>,
        /// Also name it with this alias.
        #[arg(long)]
        alias: Option<String>,
    },
    /// Instances this account reaches, optionally only one application's.
    #[command(alias = "ls")]
    List {
        #[arg(long)]
        package: Option<String>,
    },
    Get {
        #[arg(value_name = "INSTANCE")]
        context: String,
    },
    /// Delete an instance (its owner or an org admin).
    #[command(alias = "del")]
    Delete {
        #[arg(value_name = "INSTANCE")]
        context: String,
    },
    /// Move an instance to another release (`--version`, `--package`) or a
    /// bundle, migrating its state where the ABI says how. The owner
    /// approves (`--dev-passkey` on a development cluster).
    Update {
        #[arg(value_name = "INSTANCE")]
        context: String,
        #[arg(long)]
        version: Option<String>,
        #[arg(long)]
        package: Option<String>,
        #[arg(long)]
        service: Option<String>,
        #[arg(long, conflicts_with_all = ["version", "package"])]
        bundle: Option<String>,
        #[arg(long)]
        migrate: Option<String>,
        #[arg(long)]
        dev_passkey: Option<PathBuf>,
    },
    /// The update policy the instance follows (its own, its organization's or the
    /// defaults); with a flag, set its own (an owner or admin of its organization).
    UpgradePolicy {
        #[arg(value_name = "INSTANCE")]
        context: String,
        #[command(flatten)]
        policy: crate::namespace::PolicyArgs,
    },
    /// The first of two approvals of a red update, where the instance's policy asks for a
    /// second approver; another owner or admin then applies it with `instance update`.
    ApproveUpdate {
        #[arg(value_name = "INSTANCE")]
        context: String,
        #[arg(long)]
        version: String,
        /// Approve the upgrade of the instance and the apps it created (`upgrade-unit`).
        #[arg(long)]
        unit: bool,
        #[arg(long)]
        dev_passkey: Option<PathBuf>,
    },
    /// Undo the instance's last upgrade within a week of it (an owner or admin of its
    /// organization, with a passkey): back to the release it ran and to the state of its
    /// restore point, taken just before the upgrade; whatever it wrote since is lost.
    /// Without `--yes`, shows what would be undone and lost.
    Undo {
        #[arg(value_name = "INSTANCE")]
        context: String,
        /// Undo it; without it, only show the plan.
        #[arg(long)]
        yes: bool,
        #[arg(long)]
        dev_passkey: Option<PathBuf>,
    },
    /// Hand the instance to another account (its owner approves with a passkey).
    Transfer {
        #[arg(value_name = "INSTANCE")]
        context: String,
        /// The new owner's account id.
        to: String,
        #[arg(long)]
        dev_passkey: Option<PathBuf>,
    },
    /// Move the instance to another organization, its data where
    /// it is: whoever manages it now, as an admin of the target, approves
    /// with a passkey.
    Move {
        #[arg(value_name = "INSTANCE")]
        context: String,
        #[arg(value_name = "ORG")]
        namespace: String,
        /// Bring the members of its organization who reach it along.
        #[arg(long)]
        bring_members: bool,
        #[arg(long)]
        dev_passkey: Option<PathBuf>,
    },
    /// Print the instance's committed events, then live ones, as JSON lines.
    #[command(alias = "ws")]
    Watch {
        #[arg(value_name = "INSTANCE")]
        context: String,
        #[arg(long, default_value_t = 0)]
        from: u64,
        /// Stop after this many.
        #[arg(long)]
        limit: Option<usize>,
    },
    /// This account's names for instances.
    #[command(subcommand)]
    Alias(AliasCmd),
    /// Make an instance the default (the alias `default`).
    Use {
        #[arg(value_name = "INSTANCE")]
        context: String,
    },
    /// Who reaches the instance: its grants, which replace the ones it has (none: only its
    /// owner).
    Access {
        #[arg(value_name = "INSTANCE")]
        context: String,
        /// Each a way in: `org`, `team:<TEAM>`, `account:<ACCOUNT>` (a member or a guest),
        /// `account:<ACCOUNT>=<SCOPE JSON>` (with the app's scope), `anyone`.
        #[arg(long = "grant", value_name = "GRANT", value_parser = parse_grant)]
        grants: Vec<Grant>,
    },
    /// Set the instance's metadata (JSON; `null` clears it).
    Metadata {
        #[arg(value_name = "INSTANCE")]
        context: String,
        metadata: String,
    },
    /// Keys, bytes and blobs the instance holds.
    Storage {
        #[arg(value_name = "INSTANCE")]
        context: String,
    },
    /// The instance's recent outbound HTTPS requests, newest first: host, path (no
    /// query), status or failure, secret names, sizes; never a value or body. Its
    /// owner and its organization's admins only.
    OutboundLog {
        #[arg(value_name = "INSTANCE")]
        context: String,
    },
    /// The ABI of the bundle the instance runs.
    Abi {
        #[arg(value_name = "INSTANCE")]
        context: String,
    },
    /// Who is present in the instance now (ephemeral, never stored).
    #[command(subcommand)]
    Presence(PresenceCmd),
}

/// A `--grant`: `org`, `anyone`, `team:<TEAM>`, `account:<ACCOUNT>[=<SCOPE JSON>]`.
fn parse_grant(spec: &str) -> Result<Grant, String> {
    match spec {
        "org" => return Ok(Grant::Org),
        "anyone" => return Ok(Grant::Anyone),
        _ => {}
    }
    if let Some(team) = spec.strip_prefix("team:") {
        return Ok(Grant::Team {
            team: team.to_owned(),
        });
    }
    let Some(rest) = spec.strip_prefix("account:") else {
        return Err("a grant is org, anyone, team:<TEAM> or account:<ACCOUNT>[=<SCOPE>]".into());
    };
    let (account, scope) = match rest.split_once('=') {
        Some((account, scope)) => (
            account,
            Some(serde_json::from_str(scope).map_err(|e| format!("scope is JSON: {e}"))?),
        ),
        None => (rest, None),
    };
    Ok(Grant::Account {
        account: account.to_owned(),
        scope,
    })
}

#[derive(Debug, Subcommand)]
pub enum AliasCmd {
    #[command(aliases = ["new", "create"])]
    Add {
        alias: String,
        #[arg(value_name = "INSTANCE")]
        context: String,
    },
    #[command(aliases = ["rm", "del", "delete"])]
    Remove { alias: String },
    Get {
        #[arg(default_value = "default")]
        alias: String,
    },
    #[command(alias = "ls")]
    List,
}

#[derive(Debug, Subcommand)]
pub enum PresenceCmd {
    #[command(alias = "ls")]
    List {
        #[arg(value_name = "INSTANCE")]
        context: String,
    },
    /// Say something about this device: STATE is hex (none retracts).
    Publish {
        #[arg(value_name = "INSTANCE")]
        context: String,
        #[arg(long)]
        seq: u64,
        #[arg(long)]
        state: Option<String>,
    },
    /// The snapshot, then each change, as JSON lines.
    Stream {
        #[arg(value_name = "INSTANCE")]
        context: String,
        #[arg(long)]
        limit: Option<usize>,
    },
}

pub async fn run(cmd: ContextCmd, env: &Env) -> eyre::Result<Value> {
    let client = &env.client;
    let apps = client.apps();
    Ok(match cmd {
        ContextCmd::Create {
            name,
            package,
            version,
            service,
            bundle,
            path,
            params,
            namespace,
            group,
            alias,
        } => {
            let (package, version, bundle) = match path {
                Some(path) => {
                    let installed =
                        crate::app::install_file(env, &path, namespace.as_deref()).await?;
                    match installed["bundle"].as_str() {
                        Some(b) => (None, None, Some(b.to_owned())),
                        None => (
                            installed["package"].as_str().map(str::to_owned),
                            installed["version"].as_str().map(str::to_owned),
                            None,
                        ),
                    }
                }
                None => (package, version, bundle),
            };
            if package.is_none() && bundle.is_none() {
                eyre::bail!("from what? --package, --bundle or --path");
            }
            let body = AppCreateRequest {
                name: Some(name),
                package,
                version,
                service,
                bundle,
                org: namespace,
                init_args: params.as_deref().map(json_arg).transpose()?,
                grants: group.map(|team| vec![Grant::Team { team }]),
                ..AppCreateRequest::default()
            };
            let created = apps.create(body).await?;
            if let Some(alias) = alias {
                let _ = client.auth().alias_set(&alias, &created.app_id).await?;
            }
            to_json(&created)?
        }
        ContextCmd::List { package } => to_json(&apps.list(package.as_deref(), None).await?)?,
        ContextCmd::Get { context } => to_json(&apps.get(&env.app(&context).await?).await?)?,
        ContextCmd::Delete { context } => to_json(&apps.delete(&env.app(&context).await?).await?)?,
        ContextCmd::Update {
            context,
            version,
            package,
            service,
            bundle,
            migrate,
            dev_passkey,
        } => {
            let app = env.app(&context).await?;
            let body = AppUpgradeRequest {
                bundle,
                package,
                version,
                service,
                migrate,
            };
            let mut pk = DevPasskey::load(dev_passkey.as_deref())?;
            let out = apps.upgrade(&app, body, pk.get()).await;
            pk.save()?;
            to_json(&out?)?
        }
        ContextCmd::UpgradePolicy { context, policy } => {
            let app = env.app(&context).await?;
            let view = to_json(&apps.policy(&app).await?)?;
            match policy.change(&view)? {
                None => view,
                Some(change) => {
                    let mut pk = policy.passkey()?;
                    let out = apps.policy_set(&app, Some(change), pk.get()).await;
                    pk.save()?;
                    to_json(&out?)?
                }
            }
        }
        ContextCmd::ApproveUpdate {
            context,
            version,
            unit,
            dev_passkey,
        } => {
            let app = env.app(&context).await?;
            let mut pk = DevPasskey::load(dev_passkey.as_deref())?;
            let out = apps
                .approve_upgrade(&app, &version, Some(unit), pk.get())
                .await;
            pk.save()?;
            to_json(&out?)?
        }
        ContextCmd::Undo {
            context,
            yes,
            dev_passkey,
        } => {
            let app = env.app(&context).await?;
            if yes {
                let mut pk = DevPasskey::load(dev_passkey.as_deref())?;
                let out = apps.undo(&app, pk.get()).await;
                pk.save()?;
                to_json(&out?)?
            } else {
                to_json(&apps.undo_view(&app).await?)?
            }
        }
        ContextCmd::Transfer {
            context,
            to,
            dev_passkey,
        } => {
            let app = env.app(&context).await?;
            let mut pk = DevPasskey::load(dev_passkey.as_deref())?;
            let out = apps.transfer(&app, &to, pk.get()).await;
            pk.save()?;
            to_json(&out?)?
        }
        ContextCmd::Move {
            context,
            namespace,
            bring_members,
            dev_passkey,
        } => {
            let app = env.app(&context).await?;
            let mut pk = DevPasskey::load(dev_passkey.as_deref())?;
            let out = apps
                .app_move(&app, &namespace, Some(bring_members), pk.get())
                .await;
            pk.save()?;
            to_json(&out?)?
        }
        ContextCmd::Watch {
            context,
            from,
            limit,
        } => {
            let stream = client.events(&env.app(&context).await?, from).await?;
            let mut stream = Box::pin(stream.take(limit.unwrap_or(usize::MAX)));
            while let Some((id, kind, data)) = stream.next().await {
                println!("{}", json!({ "id": id, "kind": kind, "data": data }));
            }
            return Ok(Value::Null);
        }
        ContextCmd::Alias(AliasCmd::Add { alias, context }) => to_json(
            &client
                .auth()
                .alias_set(&alias, &env.app(&context).await?)
                .await?,
        )?,
        ContextCmd::Alias(AliasCmd::Remove { alias }) => {
            to_json(&client.auth().alias_delete(&alias).await?)?
        }
        ContextCmd::Alias(AliasCmd::Get { alias }) => {
            to_json(&client.auth().alias_get(&alias).await?)?
        }
        ContextCmd::Alias(AliasCmd::List) => to_json(&client.auth().alias_list().await?)?,
        ContextCmd::Use { context } => to_json(
            &client
                .auth()
                .alias_set("default", &env.app(&context).await?)
                .await?,
        )?,
        ContextCmd::Access { context, grants } => {
            to_json(&apps.access(&env.app(&context).await?, grants).await?)?
        }
        ContextCmd::Metadata { context, metadata } => to_json(
            &apps
                .metadata(&env.app(&context).await?, json_arg(&metadata)?)
                .await?,
        )?,
        ContextCmd::Storage { context } => {
            to_json(&apps.storage(&env.app(&context).await?).await?)?
        }
        ContextCmd::OutboundLog { context } => {
            to_json(&apps.outbound_log(&env.app(&context).await?).await?)?
        }
        ContextCmd::Abi { context } => to_json(&apps.abi(&env.app(&context).await?).await?)?,
        ContextCmd::Presence(PresenceCmd::List { context }) => {
            to_json(&apps.presence_get(&env.app(&context).await?).await?)?
        }
        ContextCmd::Presence(PresenceCmd::Publish {
            context,
            seq,
            state,
        }) => {
            // Hex, checked and written lower-case; none sends `null`, which retracts.
            let state = state.map(|s| hex::decode(s).map(hex::encode)).transpose()?;
            to_json(
                &apps
                    .presence_publish(&env.app(&context).await?, seq, Some(state.as_deref()))
                    .await?,
            )?
        }
        ContextCmd::Presence(PresenceCmd::Stream { context, limit }) => {
            let stream = client.presence_stream(&env.app(&context).await?).await?;
            let mut stream = Box::pin(stream.take(limit.unwrap_or(usize::MAX)));
            while let Some((_, kind, data)) = stream.next().await {
                println!("{}", json!({ "kind": kind, "data": data }));
            }
            return Ok(Value::Null);
        }
    })
}
