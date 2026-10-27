//! `meroctl blob`: a context's blobs (v1's, scoped to the context that holds them).

use std::path::PathBuf;

use clap::Subcommand;
use serde_json::{json, Value};

use crate::env::{read_file, Env};

#[derive(Debug, Subcommand)]
pub enum BlobCmd {
    #[command(alias = "ls")]
    List {
        #[arg(long, short)]
        context: String,
    },
    /// Upload a file; prints its id.
    Upload {
        file: PathBuf,
        #[arg(long, short)]
        context: String,
    },
    /// Write a blob to `--output` (default: standard output).
    Download {
        id: String,
        #[arg(long, short)]
        context: String,
        #[arg(long, short)]
        output: Option<PathBuf>,
    },
    Info {
        id: String,
        #[arg(long, short)]
        context: String,
    },
    /// Forget a blob (its owner or an org admin); its bytes go once no other context holds them.
    #[command(alias = "rm")]
    Delete {
        id: String,
        #[arg(long, short)]
        context: String,
    },
}

pub async fn run(cmd: BlobCmd, env: &Env) -> eyre::Result<Value> {
    let client = &env.client;
    Ok(match cmd {
        BlobCmd::List { context } => client.list_blobs(&env.app(&context).await?).await?,
        BlobCmd::Upload { file, context } => {
            client
                .upload_blob(&env.app(&context).await?, read_file(&file)?)
                .await?
        }
        BlobCmd::Download {
            id,
            context,
            output,
        } => {
            let bytes = client.download_blob(&env.app(&context).await?, &id).await?;
            match output {
                Some(path) => {
                    std::fs::write(&path, &bytes)?;
                    json!({ "id": id, "bytes": bytes.len(), "output": path })
                }
                None => {
                    std::io::Write::write_all(&mut std::io::stdout(), &bytes)?;
                    return Ok(Value::Null);
                }
            }
        }
        BlobCmd::Info { id, context } => client.blob_info(&env.app(&context).await?, &id).await?,
        BlobCmd::Delete { id, context } => {
            client.delete_blob(&env.app(&context).await?, &id).await?
        }
    })
}
