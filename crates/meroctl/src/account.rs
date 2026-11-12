//! `meroctl account`: this account. A person is their account and each
//! passkey is one of their devices (v1's device identities); services use
//! API keys.

use std::path::PathBuf;

use calimero_client::types::{ApikeyCreateRequest, ApikeyCreateRequestUpgrades};
use calimero_identity::testing::SoftAuthenticator;
use clap::Subcommand;
use serde_json::{json, Value};

use crate::env::{load_dev_passkey, save_passkey, to_json, write_private, DevPasskey, Env};

#[derive(Debug, Subcommand)]
pub enum AccountCmd {
    /// The account, its devices (passkeys), organizations and approvals.
    Show,
    /// What the account uses against its quotas.
    Usage,
    /// The account's devices (passkeys).
    #[command(subcommand)]
    Device(DeviceCmd),
    /// The account's sessions (needs a passkey session).
    #[command(subcommand)]
    Session(SessionCmd),
    /// What the account's passkeys did, newest first: the apps each signed
    /// in to and the changes each approved (needs a passkey session).
    Activity {
        /// Only what happened after this time (Unix seconds).
        #[arg(long)]
        after: Option<u64>,
    },
    /// API keys for services (need a passkey session to create).
    #[command(subcommand)]
    ApiKey(ApiKeyCmd),
    /// Approve a mero-tee release to hold the account's own contexts.
    ApproveRelease {
        version: String,
        #[arg(long)]
        dev_passkey: Option<PathBuf>,
    },
    /// Approve URL prefixes an application's publisher may call out to from
    /// the account's own contexts.
    Outbound {
        #[arg(long)]
        package: String,
        #[arg(long = "prefix")]
        prefixes: Vec<String>,
        #[arg(long)]
        replace: bool,
        #[arg(long)]
        dev_passkey: Option<PathBuf>,
    },
}

#[derive(Debug, Subcommand)]
pub enum DeviceCmd {
    #[command(alias = "ls")]
    List,
    /// Add a device: a new file passkey (development clusters), approved
    /// with a device the account already has.
    Add {
        #[arg(long)]
        dev_passkey: PathBuf,
        /// The passkey file of a device the account already has: it
        /// approves the new one.
        #[arg(long)]
        approve_with: PathBuf,
        #[arg(long, default_value = "http://localhost:3000")]
        origin: String,
    },
    /// Name a device ("" clears the name).
    Label {
        /// The passkey's credential id (base64url: it may start with `-`).
        #[arg(allow_hyphen_values = true)]
        credential: String,
        label: String,
    },
    /// Remove a device; its sessions end. Approved with a passkey.
    Revoke {
        /// The passkey's credential id (base64url: it may start with `-`).
        #[arg(allow_hyphen_values = true)]
        credential: String,
        #[arg(long)]
        dev_passkey: Option<PathBuf>,
    },
}

#[derive(Debug, Subcommand)]
pub enum SessionCmd {
    /// Its live sessions: the device (passkey) each was opened with, the
    /// application and origin it is bound to, when it opened and was last
    /// active (to within about an hour), and the current one.
    #[command(alias = "ls")]
    List,
    /// End one session (sign it out).
    Revoke {
        /// The session id, as `list` shows it (base64url: it may start
        /// with `-`).
        #[arg(allow_hyphen_values = true)]
        session: String,
    },
}

#[derive(Debug, Subcommand)]
pub enum ApiKeyCmd {
    /// Create a key; it is printed once.
    Create {
        #[arg(long, default_value = "meroctl")]
        label: String,
        /// Only these contexts.
        #[arg(long = "context")]
        contexts: Vec<String>,
        #[arg(long)]
        read_only: bool,
        #[arg(long)]
        ttl_secs: Option<u64>,
        /// An upgrade key, for CI: it upgrades this context to signed
        /// releases of its application (`context update --version`) and does
        /// nothing else. Takes no other scope.
        #[arg(long, conflicts_with_all = ["contexts", "read_only"])]
        upgrades: Option<String>,
        /// With `--upgrades`: the application whose releases it may move the
        /// context to (default: the one it runs).
        #[arg(long, requires = "upgrades")]
        package: Option<String>,
        /// Approve with this passkey file (development clusters).
        #[arg(long)]
        dev_passkey: Option<PathBuf>,
    },
    #[command(alias = "ls")]
    List,
    /// Revoke a key. Approved with a passkey.
    Revoke {
        key_id: String,
        #[arg(long)]
        dev_passkey: Option<PathBuf>,
    },
}

pub async fn run(cmd: AccountCmd, env: &Env) -> eyre::Result<Value> {
    let auth = env.client.auth();
    Ok(match cmd {
        AccountCmd::Show => to_json(&auth.me().await?)?,
        AccountCmd::Usage => to_json(&auth.usage().await?)?,
        AccountCmd::Device(DeviceCmd::List) => {
            let me = auth.me().await?;
            json!({ "devices": me.passkeys, "current": me.passkey_id })
        }
        AccountCmd::Device(DeviceCmd::Add {
            dev_passkey,
            approve_with,
            origin,
        }) => {
            if dev_passkey.exists() {
                eyre::bail!(
                    "{} exists: a new device needs a new file",
                    dev_passkey.display()
                );
            }
            let mut pk = SoftAuthenticator::new("localhost", &origin);
            let mut approver = load_dev_passkey(&approve_with)?;
            let id = env.client.add_passkey(&mut pk, &mut approver).await;
            save_passkey(&approve_with, &approver)?;
            let id = id?;
            write_private(&dev_passkey, &pk.to_json())?;
            json!({ "credential": id, "passkey": dev_passkey })
        }
        AccountCmd::Device(DeviceCmd::Label { credential, label }) => {
            to_json(&auth.passkey_label(&credential, &label).await?)?
        }
        AccountCmd::Device(DeviceCmd::Revoke {
            credential,
            dev_passkey,
        }) => {
            let mut pk = DevPasskey::load(dev_passkey.as_deref())?;
            let out = auth.passkey_remove(&credential, pk.get()).await;
            pk.save()?;
            to_json(&out?)?
        }
        AccountCmd::Session(SessionCmd::List) => to_json(&auth.sessions_list().await?)?,
        AccountCmd::Session(SessionCmd::Revoke { session }) => {
            to_json(&auth.session_revoke(&session).await?)?
        }
        AccountCmd::Activity { after } => to_json(&auth.activity_list(after).await?)?,
        AccountCmd::ApiKey(ApiKeyCmd::Create {
            label,
            contexts,
            read_only,
            ttl_secs,
            upgrades,
            package,
            dev_passkey,
        }) => {
            let mut apps = Vec::with_capacity(contexts.len());
            for c in &contexts {
                apps.push(env.app(c).await?);
            }
            let upgrades = match upgrades {
                Some(context) => Some(ApikeyCreateRequestUpgrades {
                    app: env.app(&context).await?,
                    package,
                }),
                None => None,
            };
            let body = ApikeyCreateRequest {
                label: Some(label),
                apps: (!apps.is_empty()).then_some(apps),
                read_only: Some(read_only),
                ttl_secs,
                upgrades,
                ..ApikeyCreateRequest::default()
            };
            let mut pk = DevPasskey::load(dev_passkey.as_deref())?;
            let out = auth.apikey_create(body, pk.get()).await;
            pk.save()?;
            let v = out?;
            json!({ "keyId": v.key_id, "key": v.key })
        }
        AccountCmd::ApiKey(ApiKeyCmd::List) => to_json(&auth.apikey_list().await?)?,
        AccountCmd::ApiKey(ApiKeyCmd::Revoke {
            key_id,
            dev_passkey,
        }) => {
            let mut pk = DevPasskey::load(dev_passkey.as_deref())?;
            let out = auth.apikey_revoke(&key_id, pk.get()).await;
            pk.save()?;
            to_json(&out?)?
        }
        AccountCmd::ApproveRelease {
            version,
            dev_passkey,
        } => crate::namespace::approve_release(env, None, &version, dev_passkey).await?,
        AccountCmd::Outbound {
            package,
            prefixes,
            replace,
            dev_passkey,
        } => {
            crate::namespace::outbound(env, None, &package, &prefixes, replace, dev_passkey).await?
        }
    })
}
