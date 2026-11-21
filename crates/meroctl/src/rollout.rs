//! `meroctl rollout`: how a deployment's offered releases reach its instances that
//! apply updates on their own (waves, paused on failures), for its managers.

use std::path::PathBuf;

use clap::Subcommand;
use serde_json::Value;

use crate::env::{to_json, DevPasskey, Env};

#[derive(Debug, Subcommand)]
pub enum RolloutCmd {
    /// Each offered app's rollout: its wave, the next one's time, whether it is
    /// paused, and how its instances fare with the release.
    Status {
        #[arg(value_name = "DEPLOYMENT")]
        deployment: String,
    },
    /// Stop the offered release of APP from reaching more instances on their own.
    Pause {
        #[arg(value_name = "DEPLOYMENT")]
        deployment: String,
        #[arg(value_name = "APP")]
        app: String,
        /// Hours between waves from here on (1 to 168).
        #[arg(long)]
        wave_hours: Option<u64>,
        #[arg(long)]
        dev_passkey: Option<PathBuf>,
    },
    /// Go on from the wave open at the pause; the next opens a full pause later.
    Resume {
        #[arg(value_name = "DEPLOYMENT")]
        deployment: String,
        #[arg(value_name = "APP")]
        app: String,
        /// Hours between waves from here on (1 to 168).
        #[arg(long)]
        wave_hours: Option<u64>,
        #[arg(long)]
        dev_passkey: Option<PathBuf>,
    },
}

pub async fn run(cmd: RolloutCmd, env: &Env) -> eyre::Result<Value> {
    let deployments = env.client.deployments();
    Ok(match cmd {
        RolloutCmd::Status { deployment } => to_json(&deployments.rollout(&deployment).await?)?,
        RolloutCmd::Pause {
            deployment,
            app,
            wave_hours,
            dev_passkey,
        } => {
            let mut pk = DevPasskey::load(dev_passkey.as_deref())?;
            let out = deployments
                .rollout_pause(&deployment, &app, wave_hours, pk.get())
                .await;
            pk.save()?;
            to_json(&out?)?
        }
        RolloutCmd::Resume {
            deployment,
            app,
            wave_hours,
            dev_passkey,
        } => {
            let mut pk = DevPasskey::load(dev_passkey.as_deref())?;
            let out = deployments
                .rollout_resume(&deployment, &app, wave_hours, pk.get())
                .await;
            pk.save()?;
            to_json(&out?)?
        }
    })
}
