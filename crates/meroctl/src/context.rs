//! `meroctl context`: app instances, v1's contexts. A context runs one
//! application release (or, on a development cluster, a raw bundle), on the
//! replicas the cluster places it on.

use std::path::PathBuf;

use calimero_client::types::{Access, AppCreateRequest, AppUpgradeRequest};
use clap::{Subcommand, ValueEnum};
use futures_util::StreamExt;
use serde_json::{json, Value};

use crate::env::{json_arg, to_json, DevPasskey, Env};

#[derive(Debug, Subcommand)]
pub enum ContextCmd {
    /// Create a context from an installed application (`--package`), a
    /// bundle hash, or a local file (`--path`: installed first).
    Create {
        /// The context's name.
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
        /// Create it in this organization (namespace).
        #[arg(long)]
        namespace: Option<String>,
        /// Reached by this team's members (a group of `--namespace`).
        #[arg(long, requires = "namespace")]
        group: Option<String>,
        /// Also name it with this alias.
        #[arg(long)]
        alias: Option<String>,
    },
    /// Contexts this account reaches, optionally only one application's.
    #[command(alias = "ls")]
    List {
        #[arg(long)]
        package: Option<String>,
    },
    Get {
        context: String,
    },
    /// Delete a context (its owner or an org admin).
    #[command(alias = "del")]
    Delete {
        context: String,
    },
    /// Move a context to another release (`--version`, `--package`) or a
    /// bundle, migrating its state where the ABI says how. The owner
    /// approves (`--dev-passkey` on a development cluster).
    Update {
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
    /// Hand the context to another account (its owner approves with a passkey).
    Transfer {
        context: String,
        /// The new owner's account id.
        to: String,
        #[arg(long)]
        dev_passkey: Option<PathBuf>,
    },
    /// Move the context to another organization (namespace), its data where
    /// it is: whoever manages it now, as an admin of the target, approves
    /// with a passkey.
    Move {
        context: String,
        namespace: String,
        /// Bring the members of its organization who reach it along.
        #[arg(long)]
        bring_members: bool,
        #[arg(long)]
        dev_passkey: Option<PathBuf>,
    },
    /// Print the context's committed events, then live ones, as JSON lines.
    #[command(alias = "ws")]
    Watch {
        context: String,
        #[arg(long, default_value_t = 0)]
        from: u64,
        /// Stop after this many.
        #[arg(long)]
        limit: Option<usize>,
    },
    /// This account's names for contexts.
    #[command(subcommand)]
    Alias(AliasCmd),
    /// Make a context the default (the alias `default`).
    Use {
        context: String,
    },
    /// Who reaches the context.
    Access {
        context: String,
        #[arg(value_enum)]
        kind: AccessKind,
        /// For `team`: the team (a group).
        #[arg(long)]
        group: Option<String>,
        /// For `accounts`: who.
        #[arg(long = "account")]
        accounts: Vec<String>,
    },
    /// Set the context's metadata (JSON; `null` clears it).
    Metadata {
        context: String,
        metadata: String,
    },
    /// Keys, bytes and blobs the context holds.
    Storage {
        context: String,
    },
    /// The ABI of the bundle the context runs.
    Abi {
        context: String,
    },
    /// Who is present in the context now (ephemeral, never stored).
    #[command(subcommand)]
    Presence(PresenceCmd),
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum AccessKind {
    Owner,
    Org,
    Team,
    Accounts,
    Public,
}

#[derive(Debug, Subcommand)]
pub enum AliasCmd {
    #[command(aliases = ["new", "create"])]
    Add { alias: String, context: String },
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
    List { context: String },
    /// Say something about this device: STATE is hex (none retracts).
    Publish {
        context: String,
        #[arg(long)]
        seq: u64,
        #[arg(long)]
        state: Option<String>,
    },
    /// The snapshot, then each change, as JSON lines.
    Stream {
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
                    let installed = crate::app::install_file(env, &path).await?;
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
                access: group.map(|team| Access::Team { team }),
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
        ContextCmd::Access {
            context,
            kind,
            group,
            accounts,
        } => {
            let access = match kind {
                AccessKind::Owner => Access::Owner,
                AccessKind::Org => Access::Org,
                AccessKind::Public => Access::Public,
                AccessKind::Team => Access::Team {
                    team: group.ok_or_else(|| eyre::eyre!("which team? --group"))?,
                },
                AccessKind::Accounts => Access::Accounts { accounts },
            };
            to_json(&apps.access(&env.app(&context).await?, access).await?)?
        }
        ContextCmd::Metadata { context, metadata } => to_json(
            &apps
                .metadata(&env.app(&context).await?, json_arg(&metadata)?)
                .await?,
        )?,
        ContextCmd::Storage { context } => {
            to_json(&apps.storage(&env.app(&context).await?).await?)?
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
