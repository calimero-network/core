//! `meroctl context`: app instances, v1's contexts. A context runs one
//! application release (or, on a development cluster, a raw bundle), on the
//! replicas the cluster places it on.

use std::path::PathBuf;

use clap::{Subcommand, ValueEnum};
use futures_util::StreamExt;
use serde_json::{json, Value};

use crate::env::{json_arg, Env};

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
        /// Methods that need a passkey confirmation per call.
        #[arg(long = "confirm")]
        confirm_methods: Vec<String>,
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
            confirm_methods,
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
            let mut body = json!({
                "name": name,
                "package": package,
                "version": version,
                "service": service,
                "bundle": bundle,
                "org": namespace,
                "confirmMethods": confirm_methods,
            });
            if let Some(p) = params {
                body["initArgs"] = json_arg(&p)?;
            }
            if let Some(team) = group {
                body["access"] = json!({ "kind": "team", "team": team });
            }
            let created = client.post("apps", body).await?;
            if let (Some(alias), Some(app)) = (alias, created["appId"].as_str()) {
                let _ = client.set_alias(&alias, app).await?;
            }
            created
        }
        ContextCmd::List { package } => match package {
            Some(p) => client.get(&format!("apps?package={p}")).await?,
            None => client.get("apps").await?,
        },
        ContextCmd::Get { context } => {
            client
                .get(&format!("apps/{}", env.app(&context).await?))
                .await?
        }
        ContextCmd::Delete { context } => client.delete_app(&env.app(&context).await?).await?,
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
            let body = json!({
                "bundle": bundle, "package": package, "version": version,
                "service": service, "migrate": migrate,
            });
            env.post_approved(&format!("apps/{app}/upgrade"), body, dev_passkey.as_deref())
                .await?
        }
        ContextCmd::Transfer {
            context,
            to,
            dev_passkey,
        } => {
            let app = env.app(&context).await?;
            env.post_approved(
                &format!("apps/{app}/transfer"),
                json!({ "to": to }),
                dev_passkey.as_deref(),
            )
            .await?
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
        ContextCmd::Alias(AliasCmd::Add { alias, context }) => {
            client.set_alias(&alias, &env.app(&context).await?).await?
        }
        ContextCmd::Alias(AliasCmd::Remove { alias }) => client.delete_alias(&alias).await?,
        ContextCmd::Alias(AliasCmd::Get { alias }) => client.alias(&alias).await?,
        ContextCmd::Alias(AliasCmd::List) => client.aliases().await?,
        ContextCmd::Use { context } => {
            client
                .set_alias("default", &env.app(&context).await?)
                .await?
        }
        ContextCmd::Access {
            context,
            kind,
            group,
            accounts,
        } => {
            let access = match kind {
                AccessKind::Owner => json!({ "kind": "owner" }),
                AccessKind::Org => json!({ "kind": "org" }),
                AccessKind::Public => json!({ "kind": "public" }),
                AccessKind::Team => json!({
                    "kind": "team",
                    "team": group.ok_or_else(|| eyre::eyre!("which team? --group"))?,
                }),
                AccessKind::Accounts => json!({ "kind": "accounts", "accounts": accounts }),
            };
            client
                .post(
                    &format!("apps/{}/access", env.app(&context).await?),
                    json!({ "access": access }),
                )
                .await?
        }
        ContextCmd::Metadata { context, metadata } => {
            client
                .set_app_metadata(&env.app(&context).await?, json_arg(&metadata)?)
                .await?
        }
        ContextCmd::Storage { context } => client.app_storage(&env.app(&context).await?).await?,
        ContextCmd::Abi { context } => client.app_abi(&env.app(&context).await?).await?,
        ContextCmd::Presence(PresenceCmd::List { context }) => {
            client.presence(&env.app(&context).await?).await?
        }
        ContextCmd::Presence(PresenceCmd::Publish {
            context,
            seq,
            state,
        }) => {
            let state = state.map(hex::decode).transpose()?;
            client
                .publish_presence(&env.app(&context).await?, seq, state.as_deref())
                .await?
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
