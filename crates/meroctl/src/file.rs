//! `meroctl file`: a context's sealed files.

use std::path::PathBuf;

use clap::Subcommand;
use eyre::WrapErr;
use serde_json::Value;

use crate::env::Env;

#[derive(Debug, Subcommand)]
pub enum FileCmd {
    /// Upload a sealed file (one request up to 16 MiB, else resumable parts); prints its id, size and SHA-256.
    Upload {
        file: PathBuf,
        #[arg(long, short)]
        context: String,
    },
}

pub async fn run(cmd: FileCmd, env: &Env) -> eyre::Result<Value> {
    Ok(match cmd {
        FileCmd::Upload { file, context } => {
            let at = || file.display().to_string();
            let reader = tokio::fs::File::open(&file).await.wrap_err_with(at)?;
            let size = reader.metadata().await.wrap_err_with(at)?.len();
            env.client
                .upload_file(&env.app(&context).await?, reader, size)
                .await?
        }
    })
}
