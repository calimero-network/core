//! `meroctl file`: a context's sealed files.

use std::path::{Path, PathBuf};

use clap::Subcommand;
use eyre::WrapErr;
use serde_json::{json, Value};

use crate::env::Env;

#[derive(Debug, Subcommand)]
pub enum FileCmd {
    /// Upload a sealed file (one request up to 16 MiB, else resumable parts); prints its id, size and SHA-256.
    Upload {
        file: PathBuf,
        #[arg(long, short)]
        context: String,
    },
    /// Download a sealed file's plaintext to `--out`: the node sends ciphertext and the file's key
    /// wrapped to a key made here, and every byte is checked before anything is written.
    Download {
        /// The context (alias or id).
        app: String,
        id: String,
        #[arg(long, short)]
        out: PathBuf,
    },
}

/// Exports file `id` of `app` and writes its plaintext to `out`; prints `{ id, size, path }`.
pub async fn download(env: &Env, app: &str, id: &str, out: &Path) -> eyre::Result<Value> {
    let bytes = env.client.export_file(app, id).await?;
    tokio::fs::write(out, &bytes)
        .await
        .wrap_err_with(|| out.display().to_string())?;
    Ok(json!({ "id": id, "size": bytes.len(), "path": out.display().to_string() }))
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
        FileCmd::Download { app, id, out } => {
            download(env, &env.app(&app).await?, &id, &out).await?
        }
    })
}
