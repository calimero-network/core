//! `calimero-cloud`: the command line for a release-v2 cluster.
//!
//! Every request goes through the same attested, sealed client the SDKs use.
//! Services authenticate with an API key (`--api-key` / `CALIMERO_API_KEY`),
//! made in the console. On a development cluster (`--allow-mock`) `login`
//! signs in with a passkey kept in a file, and the session is saved per URL so
//! later commands reuse and refresh it. The Ops API takes `--ops-token`.

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use calimero_cloud_client::{Client, Consistency, Measurements, Policy, Session};
use calimero_identity::testing::SoftAuthenticator;
use clap::{Args, Parser, Subcommand};
use eyre::{bail, WrapErr};
use futures_util::StreamExt;
use serde_json::{json, Value};

#[derive(Debug, Parser)]
#[command(
    name = "calimero-cloud",
    about = "Deploy and run apps on a Calimero cloud cluster"
)]
struct Cli {
    /// Any node of the cluster.
    #[arg(long, env = "CALIMERO_URL", default_value = "http://127.0.0.1:2501/")]
    url: String,
    /// Trust mock quotes (development clusters such as `merod-v2 local`).
    #[arg(long, env = "CALIMERO_ALLOW_MOCK")]
    allow_mock: bool,
    /// Image profiles a real node's signed mero-tee release must list it under.
    #[arg(long, env = "CALIMERO_TEE_PROFILES", value_delimiter = ',')]
    tee_profiles: Vec<String>,
    /// The oldest mero-tee release to trust.
    #[arg(long, env = "CALIMERO_MIN_RELEASE")]
    min_release: Option<String>,
    /// Authenticate with this API key instead of a saved session.
    #[arg(long, env = "CALIMERO_API_KEY", hide_env_values = true)]
    api_key: Option<String>,
    /// Operator token, for `ops` commands.
    #[arg(long, env = "CALIMERO_OPS_TOKEN", hide_env_values = true)]
    ops_token: Option<String>,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Debug, Subcommand)]
enum Cmd {
    /// Sign in with a file passkey (development clusters only); signs up when the file is new.
    Login {
        #[arg(long)]
        dev_passkey: PathBuf,
        /// Account name, for a sign-up.
        #[arg(long, default_value = "developer")]
        name: String,
        /// The origin the passkey claims (must be one the cluster allows).
        #[arg(long, default_value = "http://localhost:3000")]
        origin: String,
    },
    /// Forget the saved session for this URL.
    Logout,
    /// Who the request runs as.
    Whoami,
    /// The node's attestation (verified by this policy before anything else is sent).
    Attest,
    /// Upload a WASM bundle and create an app from it.
    Deploy {
        wasm: PathBuf,
        #[arg(long)]
        name: String,
        #[command(flatten)]
        opts: AppOpts,
    },
    #[command(subcommand)]
    Bundle(BundleCmd),
    #[command(subcommand)]
    App(AppCmd),
    /// Call a method (a write). ARGS is JSON.
    Call {
        app: String,
        method: String,
        #[arg(default_value = "{}")]
        args: String,
        #[arg(long)]
        idempotency_key: Option<String>,
    },
    /// Run a read-only method. ARGS is JSON.
    View {
        app: String,
        method: String,
        #[arg(default_value = "{}")]
        args: String,
        /// Read any replica at least this fresh instead of the leader.
        #[arg(long)]
        min_index: Option<u64>,
    },
    /// Print committed events, then live ones, as JSON lines.
    Events {
        app: String,
        #[arg(long, default_value_t = 0)]
        from: u64,
        /// Stop after this many.
        #[arg(long)]
        limit: Option<usize>,
    },
    #[command(subcommand)]
    Apikey(ApiKeyCmd),
    #[command(subcommand)]
    Org(OrgCmd),
    #[command(subcommand)]
    Ops(OpsCmd),
    /// Any API route: METHOD is get, post or delete; BODY is JSON.
    Raw {
        method: String,
        path: String,
        body: Option<String>,
    },
}

#[derive(Debug, Args)]
struct AppOpts {
    /// Arguments for the app's `init`, JSON.
    #[arg(long)]
    init_args: Option<String>,
    /// Create it in this organization.
    #[arg(long)]
    org: Option<String>,
    /// Methods that need a passkey confirmation per call.
    #[arg(long = "confirm")]
    confirm_methods: Vec<String>,
}

#[derive(Debug, Subcommand)]
enum BundleCmd {
    /// Upload a WASM bundle; prints its hash.
    Upload { wasm: PathBuf },
}

#[derive(Debug, Subcommand)]
enum AppCmd {
    Create {
        name: String,
        #[arg(long)]
        bundle: String,
        #[command(flatten)]
        opts: AppOpts,
    },
    List,
    Get {
        app: String,
    },
    /// Move an app to another bundle, optionally running its migration.
    Upgrade {
        app: String,
        #[arg(long)]
        bundle: String,
        #[arg(long)]
        migrate: Option<String>,
    },
}

#[derive(Debug, Subcommand)]
enum ApiKeyCmd {
    /// Create a key (needs a passkey session); the key is printed once.
    Create {
        #[arg(long, default_value = "cli")]
        label: String,
        #[arg(long = "app")]
        apps: Vec<String>,
        #[arg(long)]
        read_only: bool,
        #[arg(long)]
        ttl_secs: Option<u64>,
    },
    List,
    Revoke {
        key_id: String,
    },
}

#[derive(Debug, Subcommand)]
enum OrgCmd {
    Create {
        name: String,
    },
    Get {
        org: String,
    },
    /// Make an invite token (role: guest, member or admin).
    Invite {
        org: String,
        #[arg(long, default_value = "member")]
        role: String,
        #[arg(long = "team")]
        teams: Vec<String>,
    },
    Redeem {
        token: String,
    },
}

#[derive(Debug, Subcommand)]
enum OpsCmd {
    Status,
    Quotas,
    /// Replace the quotas with this JSON.
    SetQuotas {
        quotas: String,
    },
    /// Write encrypted snapshots of the apps this node leads.
    Backup,
    /// Move an app to these replica node ids.
    Move {
        app: String,
        #[arg(long, value_delimiter = ',')]
        replicas: Vec<u64>,
    },
}

fn json_arg(s: &str) -> eyre::Result<Value> {
    serde_json::from_str(s).wrap_err_with(|| format!("not JSON: {s}"))
}

fn print(v: &impl serde::Serialize) -> eyre::Result<()> {
    println!("{}", serde_json::to_string_pretty(v)?);
    Ok(())
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Saved sessions, one per cluster URL.
struct Sessions {
    path: PathBuf,
}

impl Sessions {
    fn new() -> Self {
        let base = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
            .unwrap_or_else(|| PathBuf::from("."));
        Self {
            path: base.join("calimero-cloud").join("sessions.json"),
        }
    }

    fn all(&self) -> serde_json::Map<String, Value> {
        std::fs::read_to_string(&self.path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    fn get(&self, url: &str) -> Option<Session> {
        serde_json::from_value(self.all().get(url)?.clone()).ok()
    }

    fn put(&self, url: &str, s: Option<&Session>) -> eyre::Result<()> {
        let mut all = self.all();
        match s {
            Some(s) => {
                let _prev = all.insert(url.to_owned(), serde_json::to_value(s)?);
            }
            None => {
                let _prev = all.remove(url);
            }
        }
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        write_private(&self.path, &serde_json::to_string_pretty(&all)?)
    }
}

/// Write a file only its owner can read (sessions and passkeys are credentials).
fn write_private(path: &std::path::Path, contents: &str) -> eyre::Result<()> {
    let mut options = std::fs::OpenOptions::new();
    let _ = options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        let _ = options.mode(0o600);
    }
    std::io::Write::write_all(&mut options.open(path)?, contents.as_bytes())?;
    Ok(())
}

async fn authenticate(cli: &Cli, client: &Client, sessions: &Sessions) -> eyre::Result<()> {
    if let Some(key) = &cli.api_key {
        client.use_api_key(key).await;
        return Ok(());
    }
    if let Some(s) = sessions.get(&cli.url) {
        client.use_session(s.clone()).await;
        if s.expires_at <= now() + 30 {
            let fresh = client.refresh().await?;
            sessions.put(&cli.url, Some(&fresh))?;
        }
    }
    Ok(())
}

fn app_body(opts: &AppOpts) -> eyre::Result<Value> {
    let mut o = json!({ "confirmMethods": opts.confirm_methods });
    if let Some(a) = &opts.init_args {
        o["initArgs"] = json_arg(a)?;
    }
    if let Some(org) = &opts.org {
        o["org"] = json!(org);
    }
    Ok(o)
}

#[tokio::main]
async fn main() -> eyre::Result<()> {
    let cli = Cli::parse();
    let policy = client_policy(&cli);
    let client = Client::connect(&cli.url, policy)?;
    let sessions = Sessions::new();

    if let Cmd::Ops(cmd) = &cli.cmd {
        let token = cli
            .ops_token
            .as_deref()
            .ok_or_else(|| eyre::eyre!("ops commands need --ops-token"))?;
        client.use_api_key(token).await;
        let out = match cmd {
            OpsCmd::Status => client.get("ops/status").await?,
            OpsCmd::Quotas => client.get("ops/quotas").await?,
            OpsCmd::SetQuotas { quotas } => client.post("ops/quotas", json_arg(quotas)?).await?,
            OpsCmd::Backup => client.post("ops/backup", json!({})).await?,
            OpsCmd::Move { app, replicas } => {
                let path = format!("ops/apps/{app}/replicas");
                let body = json!({ "replicas": replicas });
                match client.post(&path, body.clone()).await {
                    Err(e) if e.code() == Some("NOT_LEADER") => {
                        let leader = e
                            .details()
                            .and_then(|d| d["leaderUrl"].as_str())
                            .ok_or_else(|| eyre::eyre!("{e}"))?
                            .to_owned();
                        let at = Client::connect(&leader, client_policy(&cli))?;
                        at.use_api_key(token).await;
                        at.post(&path, body).await?
                    }
                    other => other?,
                }
            }
        };
        return print(&out);
    }

    if let Cmd::Login {
        dev_passkey,
        name,
        origin,
    } = &cli.cmd
    {
        if !cli.allow_mock {
            bail!("a file passkey is for development clusters (--allow-mock); on a real cluster use an API key from the console");
        }
        let session = if dev_passkey.exists() {
            let mut pk = SoftAuthenticator::from_json(&std::fs::read_to_string(dev_passkey)?)
                .map_err(|e| eyre::eyre!("{}: {e}", dev_passkey.display()))?;
            let s = client.sign_in(&mut pk).await?;
            write_private(dev_passkey, &pk.to_json())?;
            s
        } else {
            let mut pk = SoftAuthenticator::new("localhost", origin);
            let s = client.sign_up(&mut pk, name).await?;
            write_private(dev_passkey, &pk.to_json())?;
            s
        };
        sessions.put(&cli.url, Some(&session))?;
        return print(&json!({ "accountId": session.account_id, "passkey": dev_passkey }));
    }
    if let Cmd::Logout = &cli.cmd {
        sessions.put(&cli.url, None)?;
        return Ok(());
    }

    authenticate(&cli, &client, &sessions).await?;
    let out: Value = match &cli.cmd {
        Cmd::Whoami => client.me().await?,
        Cmd::Attest => serde_json::to_value(client.attestation().await?)?,
        Cmd::Deploy { wasm, name, opts } => {
            let bytes = std::fs::read(wasm).wrap_err_with(|| format!("{}", wasm.display()))?;
            let bundle = client.upload_bundle(bytes).await?;
            let app = client.create_app(name, &bundle, app_body(opts)?).await?;
            json!({ "appId": app, "bundle": bundle })
        }
        Cmd::Bundle(BundleCmd::Upload { wasm }) => {
            let bytes = std::fs::read(wasm).wrap_err_with(|| format!("{}", wasm.display()))?;
            json!({ "hash": client.upload_bundle(bytes).await? })
        }
        Cmd::App(AppCmd::Create { name, bundle, opts }) => {
            json!({ "appId": client.create_app(name, bundle, app_body(opts)?).await? })
        }
        Cmd::App(AppCmd::List) => client.get("apps").await?,
        Cmd::App(AppCmd::Get { app }) => client.get(&format!("apps/{app}")).await?,
        Cmd::App(AppCmd::Upgrade {
            app,
            bundle,
            migrate,
        }) => {
            client
                .post(
                    &format!("apps/{app}/upgrade"),
                    json!({ "bundle": bundle, "migrate": migrate }),
                )
                .await?
        }
        Cmd::Call {
            app,
            method,
            args,
            idempotency_key,
        } => serde_json::to_value(
            client
                .call(app, method, json_arg(args)?, idempotency_key.as_deref())
                .await?,
        )?,
        Cmd::View {
            app,
            method,
            args,
            min_index,
        } => {
            let consistency = match min_index {
                Some(min_index) => Consistency::Stale {
                    min_index: *min_index,
                },
                None => Consistency::Strong,
            };
            serde_json::to_value(
                client
                    .view(app, method, json_arg(args)?, consistency)
                    .await?,
            )?
        }
        Cmd::Events { app, from, limit } => {
            let stream = client.events(app, *from).await?;
            let mut stream = Box::pin(stream.take(limit.unwrap_or(usize::MAX)));
            while let Some((id, kind, data)) = stream.next().await {
                println!("{}", json!({ "id": id, "kind": kind, "data": data }));
            }
            return Ok(());
        }
        Cmd::Apikey(ApiKeyCmd::Create {
            label,
            apps,
            read_only,
            ttl_secs,
        }) => {
            let apps = (!apps.is_empty()).then_some(apps);
            let (key_id, key) = client
                .create_api_key(json!({
                    "label": label, "apps": apps, "readOnly": read_only, "ttlSecs": ttl_secs,
                }))
                .await?;
            json!({ "keyId": key_id, "key": key })
        }
        Cmd::Apikey(ApiKeyCmd::List) => client.get("auth/apikeys").await?,
        Cmd::Apikey(ApiKeyCmd::Revoke { key_id }) => {
            client.delete(&format!("auth/apikeys/{key_id}")).await?
        }
        Cmd::Org(OrgCmd::Create { name }) => client.post("orgs", json!({ "name": name })).await?,
        Cmd::Org(OrgCmd::Get { org }) => client.get(&format!("orgs/{org}")).await?,
        Cmd::Org(OrgCmd::Invite { org, role, teams }) => {
            client
                .post(
                    &format!("orgs/{org}/invites"),
                    json!({ "role": role, "teams": teams }),
                )
                .await?
        }
        Cmd::Org(OrgCmd::Redeem { token }) => {
            client
                .post(&format!("invites/{token}/redeem"), json!({}))
                .await?
        }
        Cmd::Raw { method, path, body } => {
            let path = path.trim_start_matches('/');
            match method.to_ascii_lowercase().as_str() {
                "get" => client.get(path).await?,
                "delete" => client.delete(path).await?,
                "post" => {
                    let b = body.as_deref().map(json_arg).transpose()?;
                    client.post(path, b.unwrap_or_else(|| json!({}))).await?
                }
                other => bail!("unknown method {other}: use get, post or delete"),
            }
        }
        Cmd::Login { .. } | Cmd::Logout | Cmd::Ops(_) => unreachable!("handled above"),
    };
    print(&out)
}

fn client_policy(cli: &Cli) -> Policy {
    Policy {
        allow_mock: cli.allow_mock,
        app_hash: None,
        measurements: if cli.tee_profiles.is_empty() {
            Measurements::None
        } else {
            Measurements::Release {
                allowed_profiles: cli.tee_profiles.clone(),
                min_version: cli.min_release.clone(),
            }
        },
        allowed_tcb_statuses: Vec::new(),
    }
}
