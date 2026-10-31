//! `meroctl account`: this account. A person is their account and each
//! passkey is one of their devices (v1's device identities); services use
//! API keys.

use std::path::PathBuf;

use calimero_identity::testing::SoftAuthenticator;
use clap::Subcommand;
use serde_json::{json, Value};

use crate::env::{write_private, Env};

#[derive(Debug, Subcommand)]
pub enum AccountCmd {
    /// The account, its devices (passkeys), organizations and approvals.
    Show,
    /// What the account uses against its quotas.
    Usage,
    /// The account's devices (passkeys).
    #[command(subcommand)]
    Device(DeviceCmd),
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
    /// Add a device: a new file passkey (development clusters).
    Add {
        #[arg(long)]
        dev_passkey: PathBuf,
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
    /// Remove a device; its sessions end.
    Revoke {
        /// The passkey's credential id (base64url: it may start with `-`).
        #[arg(allow_hyphen_values = true)]
        credential: String,
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
    },
    #[command(alias = "ls")]
    List,
    Revoke {
        key_id: String,
    },
}

pub async fn run(cmd: AccountCmd, env: &Env) -> eyre::Result<Value> {
    let client = &env.client;
    Ok(match cmd {
        AccountCmd::Show => client.me().await?,
        AccountCmd::Usage => client.usage().await?,
        AccountCmd::Device(DeviceCmd::List) => {
            let me = client.me().await?;
            json!({ "devices": me["passkeys"], "current": me["passkeyId"] })
        }
        AccountCmd::Device(DeviceCmd::Add {
            dev_passkey,
            origin,
        }) => {
            if dev_passkey.exists() {
                eyre::bail!(
                    "{} exists: a new device needs a new file",
                    dev_passkey.display()
                );
            }
            let mut pk = SoftAuthenticator::new("localhost", &origin);
            let id = client.add_passkey(&mut pk).await?;
            write_private(&dev_passkey, &pk.to_json())?;
            json!({ "credential": id, "passkey": dev_passkey })
        }
        AccountCmd::Device(DeviceCmd::Label { credential, label }) => {
            client.label_passkey(&credential, &label).await?
        }
        AccountCmd::Device(DeviceCmd::Revoke { credential }) => {
            client
                .delete(&format!("auth/passkeys/{credential}"))
                .await?
        }
        AccountCmd::ApiKey(ApiKeyCmd::Create {
            label,
            contexts,
            read_only,
            ttl_secs,
        }) => {
            let mut apps = Vec::with_capacity(contexts.len());
            for c in &contexts {
                apps.push(env.app(c).await?);
            }
            let apps = (!apps.is_empty()).then_some(apps);
            let (key_id, key) = client
                .create_api_key(json!({
                    "label": label, "apps": apps, "readOnly": read_only, "ttlSecs": ttl_secs,
                }))
                .await?;
            json!({ "keyId": key_id, "key": key })
        }
        AccountCmd::ApiKey(ApiKeyCmd::List) => client.get("auth/apikeys").await?,
        AccountCmd::ApiKey(ApiKeyCmd::Revoke { key_id }) => {
            client.delete(&format!("auth/apikeys/{key_id}")).await?
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
