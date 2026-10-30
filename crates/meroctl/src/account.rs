//! `meroctl account`: this account. A person is their account and each
//! passkey is one of their devices (v1's device identities); services use
//! API keys.

use std::path::PathBuf;

use calimero_identity::testing::SoftAuthenticator;
use clap::Subcommand;
use serde_json::{json, Value};

use crate::env::{load_dev_passkey, save_passkey, write_private, Env};

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
    Label { credential: String, label: String },
    /// Remove a device; its sessions end. Approved with a passkey.
    Revoke {
        credential: String,
        #[arg(long)]
        dev_passkey: Option<PathBuf>,
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
        /// Approve with this passkey file (development clusters).
        #[arg(long)]
        dev_passkey: Option<PathBuf>,
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
            let id = client.add_passkey(&mut pk, &mut approver).await;
            save_passkey(&approve_with, &approver)?;
            let id = id?;
            write_private(&dev_passkey, &pk.to_json())?;
            json!({ "credential": id, "passkey": dev_passkey })
        }
        AccountCmd::Device(DeviceCmd::Label { credential, label }) => {
            client.label_passkey(&credential, &label).await?
        }
        AccountCmd::Device(DeviceCmd::Revoke {
            credential,
            dev_passkey,
        }) => {
            env.delete_approved(
                &format!("auth/passkeys/{credential}"),
                dev_passkey.as_deref(),
            )
            .await?
        }
        AccountCmd::ApiKey(ApiKeyCmd::Create {
            label,
            contexts,
            read_only,
            ttl_secs,
            dev_passkey,
        }) => {
            let mut apps = Vec::with_capacity(contexts.len());
            for c in &contexts {
                apps.push(env.app(c).await?);
            }
            let apps = (!apps.is_empty()).then_some(apps);
            let v = env
                .post_approved(
                    "auth/apikeys",
                    json!({
                        "label": label, "apps": apps, "readOnly": read_only, "ttlSecs": ttl_secs,
                    }),
                    dev_passkey.as_deref(),
                )
                .await?;
            json!({ "keyId": v["keyId"], "key": v["key"] })
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
