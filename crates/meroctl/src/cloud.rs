//! `meroctl cloud`: Calimero Cloud (the console's manager API, mdma) and the
//! compute a cluster runs. `login` keeps a console session, `orgs` and
//! `clusters` list and create, `use` saves a cluster's node as the default
//! node; `compute` is a cluster's pipelines and compute runs (any cluster,
//! `local` included, through `--node`).
//!
//! The manager accepts only a console session for clusters and organizations
//! (Google sign-in): `login --token` takes the session token the console shows,
//! and the manager's `X-MDMA-Session-Refresh` header keeps it fresh. An
//! organization's API key (`ck_live_…`) publishes (`meroctl app publish --org`).

use std::path::PathBuf;

use clap::Subcommand;
use eyre::{bail, eyre, WrapErr};
use reqwest::Method;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::env::{write_private, Home, SavedNode};

pub const DEFAULT_CLOUD: &str = "https://cloud.calimero.network";
const REFRESH_HEADER: &str = "x-mdma-session-refresh";

#[derive(Debug, Subcommand)]
pub enum CloudCmd {
    /// Keep a console session for the commands below: a signed-in console's
    /// session token (the bearer it sends the manager), checked against the Cloud.
    Login {
        /// The session token (also `CALIMERO_CLOUD_TOKEN`).
        #[arg(long, env = "CALIMERO_CLOUD_TOKEN", hide_env_values = true)]
        token: String,
        /// The Cloud.
        #[arg(long, env = "CALIMERO_CLOUD_URL", default_value = DEFAULT_CLOUD)]
        url: String,
    },
    /// Forget the saved session.
    Logout,
    /// Your organizations, the personal one first.
    Orgs,
    /// Your dedicated clusters, and a new one.
    #[command(subcommand)]
    Clusters(ClustersCmd),
    /// Save a cluster's node as a meroctl node (default: named after the
    /// cluster) and make it the default, so every other command talks to it.
    Use {
        /// The cluster's id or name.
        cluster: String,
        /// The saved node's name.
        #[arg(long = "as")]
        name: Option<String>,
    },
    /// A cluster's compute: pipelines and the logs of compute runs. Talks to
    /// the node `--node` names (the default after `cloud use`; `local` works too).
    #[command(subcommand)]
    Compute(ComputeCmd),
}

#[derive(Debug, Subcommand)]
pub enum ClustersCmd {
    /// Every cluster you can see, with its nodes' URLs.
    #[command(alias = "ls")]
    List,
    /// A new dedicated cluster (an organization admin; quota and a payment method).
    Create {
        name: String,
        /// The organization (default: your personal one).
        #[arg(long)]
        org: Option<String>,
        /// The cluster's size, as the console offers it.
        #[arg(long)]
        size: String,
        #[arg(long)]
        region: Option<String>,
    },
}

#[derive(Debug, Subcommand)]
pub enum ComputeCmd {
    /// A context's pipelines: validate, estimate, run, follow, receipts, outputs.
    #[command(subcommand)]
    Pipeline(crate::pipeline::PipelineCmd),
    /// A compute run's module log (stdout, stderr and `log` lines).
    Logs {
        /// The context (alias or id).
        app: String,
        run: String,
        /// The attempt (default: the latest that kept a log).
        #[arg(long)]
        attempt: Option<u32>,
    },
}

impl CloudCmd {
    /// Talks to a node, not to the Cloud: `main` connects first.
    pub fn needs_node(&self) -> bool {
        matches!(self, Self::Compute(_))
    }
}

/// The saved console session, `{home}/meroctl/cloud.json` (owner-only).
#[derive(Debug, Serialize, Deserialize)]
struct Saved {
    url: String,
    token: String,
}

fn file(home: &Home) -> PathBuf {
    home.dir.join("meroctl").join("cloud.json")
}

fn load(home: &Home) -> eyre::Result<Saved> {
    let text = std::fs::read_to_string(file(home))
        .map_err(|_| eyre!("not signed in to the Cloud: meroctl cloud login --token <session token>"))?;
    Ok(serde_json::from_str(&text)?)
}

fn save(home: &Home, saved: &Saved) -> eyre::Result<()> {
    write_private(&file(home), &serde_json::to_string_pretty(saved)?)
}

/// One manager request with the saved session; a refreshed token is saved.
async fn call(home: &Home, method: Method, path: &str, body: Option<Value>) -> eyre::Result<Value> {
    let mut saved = load(home)?;
    let (answer, refreshed) = request(&saved.url, &saved.token, method, path, body).await?;
    if let Some(token) = refreshed {
        saved.token = token;
        save(home, &saved)?;
    }
    Ok(answer)
}

async fn request(
    url: &str,
    token: &str,
    method: Method,
    path: &str,
    body: Option<Value>,
) -> eyre::Result<(Value, Option<String>)> {
    let base = url.trim_end_matches('/');
    if !base.starts_with("https://") && !crate::make::is_loopback(base) {
        bail!("the Cloud URL must be https:// (a session is never sent in the clear)");
    }
    let mut req = reqwest::Client::new()
        .request(method, format!("{base}{path}"))
        .bearer_auth(token);
    if let Some(body) = body {
        req = req.json(&body);
    }
    let res = req.send().await.wrap_err_with(|| format!("reaching {base}"))?;
    let status = res.status();
    let refreshed = res
        .headers()
        .get(REFRESH_HEADER)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    let answer: Value = res.json().await.unwrap_or(Value::Null);
    if status == reqwest::StatusCode::UNAUTHORIZED {
        bail!("the Cloud refused the session (401): sign in again with meroctl cloud login");
    }
    if !status.is_success() {
        let detail = answer["detail"]
            .as_str()
            .or(answer["message"].as_str())
            .or(answer["error"].as_str())
            .map(str::to_owned)
            .unwrap_or_else(|| answer.to_string());
        bail!("the Cloud answered {status}: {detail}");
    }
    Ok((answer, refreshed))
}

/// The commands that talk to the Cloud itself.
pub async fn run(cmd: CloudCmd, home: &Home) -> eyre::Result<Value> {
    match cmd {
        CloudCmd::Login { token, url } => {
            let (orgs, refreshed) = request(&url, &token, Method::GET, "/api/v2/orgs", None).await?;
            save(
                home,
                &Saved {
                    url: url.clone(),
                    token: refreshed.unwrap_or(token),
                },
            )?;
            Ok(json!({ "cloud": url, "orgs": orgs }))
        }
        CloudCmd::Logout => {
            let _ = std::fs::remove_file(file(home));
            Ok(Value::Null)
        }
        CloudCmd::Orgs => call(home, Method::GET, "/api/v2/orgs", None).await,
        CloudCmd::Clusters(ClustersCmd::List) => call(home, Method::GET, "/api/v2/clusters", None).await,
        CloudCmd::Clusters(ClustersCmd::Create {
            name,
            org,
            size,
            region,
        }) => {
            let mut body = json!({ "name": name, "size": size });
            if let Some(org) = org {
                body["org"] = json!(org);
            }
            if let Some(region) = region {
                body["region"] = json!(region);
            }
            call(home, Method::POST, "/api/v2/clusters", Some(body)).await
        }
        CloudCmd::Use { cluster, name } => {
            let all = call(home, Method::GET, "/api/v2/clusters", None).await?;
            let list = all
                .as_array()
                .or_else(|| all["clusters"].as_array())
                .ok_or_else(|| eyre!("the Cloud answered no cluster list"))?;
            let found = list
                .iter()
                .find(|c| c["id"] == cluster.as_str() || c["name"] == cluster.as_str())
                .ok_or_else(|| eyre!("no cluster {cluster} (meroctl cloud clusters list)"))?;
            let url = found["nodes"]
                .as_array()
                .and_then(|n| n.iter().find_map(|n| n["public_url"].as_str()))
                .ok_or_else(|| eyre!("cluster {cluster} has no node with a public URL yet"))?;
            let name = name.unwrap_or_else(|| {
                found["name"]
                    .as_str()
                    .unwrap_or(&cluster)
                    .chars()
                    .map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '-' })
                    .collect()
            });
            let mut nodes = home.nodes();
            let _ = nodes.nodes.insert(
                name.clone(),
                SavedNode {
                    url: url.to_owned(),
                    allow_mock: false,
                },
            );
            nodes.default = Some(name.clone());
            home.save_nodes(&nodes)?;
            Ok(json!({ "node": name, "url": url, "default": true }))
        }
        CloudCmd::Compute(_) => unreachable!("connects first (main)"),
    }
}
