//! What every command runs with: the node (saved with `meroctl node add`, or
//! `--api`), the attested client, the saved session, and helpers.
//!
//! State lives under `--home` (v1's `CALIMERO_HOME`, `~/.calimero`):
//! `meroctl/nodes.json` (aliases, the default) and `meroctl/sessions.json`
//! (one session per node URL). Both are written readable by their owner only.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use calimero_client::{Client, Measurements, Policy, Session};
use calimero_identity::testing::SoftAuthenticator;
use eyre::WrapErr;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// How to judge the node's quote.
#[derive(Clone, Debug, Default)]
pub struct Trust {
    pub allow_mock: bool,
    pub allow_debug_profiles: bool,
    pub tee_profiles: Vec<String>,
    pub min_release: Option<String>,
}

impl Trust {
    pub fn policy(&self) -> Policy {
        Policy {
            allow_mock: self.allow_mock,
            app_hash: None,
            measurements: if self.tee_profiles.is_empty() {
                Measurements::None
            } else {
                Measurements::Release {
                    allowed_profiles: self.tee_profiles.clone(),
                    min_version: self.min_release.clone(),
                }
            },
            allowed_tcb_statuses: Vec::new(),
            allow_debug_profiles: self.allow_debug_profiles,
        }
    }
}

/// A saved node.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SavedNode {
    pub url: String,
    /// A development cluster: its mock quote is trusted.
    #[serde(default)]
    pub allow_mock: bool,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Nodes {
    #[serde(default)]
    pub default: Option<String>,
    #[serde(default)]
    pub nodes: std::collections::BTreeMap<String, SavedNode>,
}

pub struct Home {
    pub dir: PathBuf,
}

impl Home {
    fn file(&self, name: &str) -> PathBuf {
        self.dir.join("meroctl").join(name)
    }

    pub fn nodes(&self) -> Nodes {
        read_json(&self.file("nodes.json")).unwrap_or_default()
    }

    pub fn save_nodes(&self, nodes: &Nodes) -> eyre::Result<()> {
        write_private(
            &self.file("nodes.json"),
            &serde_json::to_string_pretty(nodes)?,
        )
    }

    fn sessions(&self) -> serde_json::Map<String, Value> {
        read_json(&self.file("sessions.json")).unwrap_or_default()
    }

    pub fn session(&self, url: &str) -> Option<Session> {
        serde_json::from_value(self.sessions().get(url)?.clone()).ok()
    }

    pub fn save_session(&self, url: &str, s: Option<&Session>) -> eyre::Result<()> {
        let mut all = self.sessions();
        match s {
            Some(s) => {
                let _prev = all.insert(url.to_owned(), serde_json::to_value(s)?);
            }
            None => {
                let _prev = all.remove(url);
            }
        }
        write_private(
            &self.file("sessions.json"),
            &serde_json::to_string_pretty(&all)?,
        )
    }
}

fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Option<T> {
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

/// Write a file only its owner can read (sessions and passkeys are credentials).
pub fn write_private(path: &Path, contents: &str) -> eyre::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
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

pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

pub fn json_arg(s: &str) -> eyre::Result<Value> {
    serde_json::from_str(s).wrap_err_with(|| format!("not JSON: {s}"))
}

pub fn read_file(path: &Path) -> eyre::Result<Vec<u8>> {
    std::fs::read(path).wrap_err_with(|| format!("{}", path.display()))
}

/// The passkey kept in a development passkey file.
pub fn load_dev_passkey(path: &Path) -> eyre::Result<SoftAuthenticator> {
    SoftAuthenticator::from_json(&std::fs::read_to_string(path)?)
        .map_err(|e| eyre::eyre!("{}: {e}", path.display()))
}

/// The client for the chosen node, signed in as the saved session or API key.
pub struct Env {
    pub home: Home,
    pub url: String,
    pub trust: Trust,
    pub client: Client,
    pub api_key: Option<String>,
    pub ops_token: Option<String>,
}

impl Env {
    pub fn connect(
        home: Home,
        url: String,
        trust: Trust,
        api_key: Option<String>,
        ops_token: Option<String>,
    ) -> eyre::Result<Self> {
        let client = Client::connect(&url, trust.policy())?;
        Ok(Self {
            home,
            url,
            trust,
            client,
            api_key,
            ops_token,
        })
    }

    /// Sign the client in: the API key if given, else the saved session
    /// (refreshed when it is about to expire). Without either, requests go
    /// out unauthenticated and the node says what they need.
    pub async fn authenticate(&self) -> eyre::Result<()> {
        if let Some(key) = &self.api_key {
            self.client.use_api_key(key).await;
            return Ok(());
        }
        if let Some(s) = self.home.session(&self.url) {
            self.client.use_session(s.clone()).await;
            if s.expires_at <= now() + 30 {
                let fresh = self.client.refresh().await?;
                self.home.save_session(&self.url, Some(&fresh))?;
            }
        }
        Ok(())
    }

    /// A client for another node of the same cluster, with the same trust.
    pub fn at(&self, url: &str) -> eyre::Result<Client> {
        Client::connect(url, self.trust.policy())
    }

    /// The app (context) an argument names: an alias of this account, or
    /// the id itself.
    pub async fn app(&self, name: &str) -> eyre::Result<String> {
        if self.api_key.is_none() && self.home.session(&self.url).is_none() {
            return Ok(name.to_owned());
        }
        Ok(match self.client.alias(name).await {
            Ok(v) => v["appId"].as_str().unwrap_or(name).to_owned(),
            Err(_) => name.to_owned(),
        })
    }

    /// Delete an organization, which its owner approves (as [`Self::post_approved`]).
    pub async fn delete_approved(
        &self,
        org: &str,
        dev_passkey: Option<&Path>,
    ) -> eyre::Result<Value> {
        match dev_passkey {
            Some(file) => {
                let mut pk = load_dev_passkey(file)?;
                let out = self.client.delete_org(org, &mut pk).await;
                save_passkey(file, &pk)?;
                Ok(out?)
            }
            None => Ok(self.client.delete(&format!("orgs/{org}")).await?),
        }
    }

    /// POST that an owner approves: with `--dev-passkey`, the challenge is
    /// signed with that file's passkey; without it, a real cluster answers
    /// `412 CONFIRM_REQUIRED` and the owner approves in the console.
    pub async fn post_approved(
        &self,
        path: &str,
        body: Value,
        dev_passkey: Option<&Path>,
    ) -> eyre::Result<Value> {
        match dev_passkey {
            Some(file) => {
                let mut pk = load_dev_passkey(file)?;
                let out = self.client.post_confirmed(path, body, &mut pk).await;
                save_passkey(file, &pk)?;
                Ok(out?)
            }
            None => Ok(self.client.post(path, body).await?),
        }
    }
}

/// Keep a development passkey after use: its counter moved.
pub fn save_passkey(file: &Path, pk: &SoftAuthenticator) -> eyre::Result<()> {
    write_private(file, &pk.to_json())
}
