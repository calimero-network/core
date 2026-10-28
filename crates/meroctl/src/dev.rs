//! `meroctl dev`: v1's developer workflow. `start` installs a build, creates
//! a context from it and (with `--watch`) keeps pushing it on every save.

use std::path::PathBuf;

use clap::Subcommand;
use serde_json::{json, Value};

use crate::env::{json_arg, Env};

#[derive(Debug, Subcommand)]
pub enum DevCmd {
    /// Install a build (`.mpk`, or a development `.wasm`), create a context
    /// from it, and with `--watch` push every save to it.
    Start {
        #[arg(long)]
        path: PathBuf,
        /// The context's name.
        #[arg(long, default_value = "dev")]
        name: String,
        /// Arguments for the app's `init`, JSON.
        #[arg(long, short)]
        params: Option<String>,
        #[arg(long, short)]
        watch: bool,
        /// The owner's passkey file that approves each update.
        #[arg(long)]
        dev_passkey: PathBuf,
    },
    /// Push a build to its contexts on every save (as `meroctl app watch`).
    Watch {
        #[arg(long)]
        path: PathBuf,
        #[arg(long = "context", short)]
        contexts: Vec<String>,
        #[arg(long)]
        dev_passkey: PathBuf,
        #[arg(long)]
        once: bool,
        #[arg(long, default_value_t = 500)]
        interval_ms: u64,
    },
}

pub async fn run(cmd: DevCmd, env: &Env) -> eyre::Result<()> {
    match cmd {
        DevCmd::Start {
            path,
            name,
            params,
            watch,
            dev_passkey,
        } => {
            let installed = crate::app::install_file(env, &path).await?;
            let mut body = json!({ "name": name });
            match installed["bundle"].as_str() {
                Some(b) => body["bundle"] = json!(b),
                None => {
                    body["package"] = installed["package"].clone();
                    body["version"] = installed["version"].clone();
                }
            }
            if let Some(p) = params {
                body["initArgs"] = json_arg(&p)?;
            }
            let created: Value = env.client.post("apps", body).await?;
            println!("{}", serde_json::to_string(&created)?);
            if watch {
                let app = created["appId"].as_str().unwrap_or_default().to_owned();
                // The build is installed already: the first push is the next save.
                crate::app::watch(env, &path, &[app], &dev_passkey, false, 500, false).await?;
            }
            Ok(())
        }
        DevCmd::Watch {
            path,
            contexts,
            dev_passkey,
            once,
            interval_ms,
        } => crate::app::watch(env, &path, &contexts, &dev_passkey, once, interval_ms, true).await,
    }
}
