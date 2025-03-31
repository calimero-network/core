use eyre::{bail, Result as EyreResult};
use serde::{Deserialize, Serialize};
use tokio::fs::{self, File};
use async_compression::tokio::bufread::GzipDecoder;
use tokio::io::{self, AsyncWriteExt, BufReader};
use futures_util::{TryStreamExt, StreamExt};
use tokio_util::io::StreamReader;
use tokio::sync::mpsc;
use tokio_util::bytes::Bytes;

use crate::driver::{Test, TestContext};
use crate::meroctl::Meroctl;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApplicationInstallStep {
    pub application: ApplicationSource,
    pub target: ApplicationInstallTarget,
}

/// Source location for an application that can be installed.
/// Supports both local files and remote URLs with optional gzip compression.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ApplicationSource {
    /// Local file path to a WASM application
    LocalFile(String),
    /// Remote URL pointing to a WASM application, optionally gzip compressed (.gz extension)
    Url(String),
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ApplicationInstallTarget {
    Inviter,
    AllMembers,
}

impl Test for ApplicationInstallStep {
    fn display_name(&self) -> String {
        format!("app install ({:?})", self.target)
    }

    async fn run_assert(&self, ctx: &mut TestContext<'_>) -> EyreResult<()> {
        if matches!(self.target, ApplicationInstallTarget::AllMembers) {
            for invitee in &ctx.invitees {
                let application_id = self.application.install(ctx.meroctl, invitee).await?;
                if let Some(existing_app_id) = &ctx.application_id {
                    if existing_app_id != &application_id {
                        bail!(
                            "Application ID mismatch: existing ID is {}, but got {}",
                            existing_app_id,
                            application_id
                        );
                    }
                }

                if ctx
                    .meroctl
                    .application_get(invitee, &application_id)
                    .await?
                    .is_null()
                {
                    bail!("Failed to lookup installed application on '{}'", invitee);
                }

                ctx.application_id = Some(application_id);

                ctx.output_writer.write_str(&format!(
                    "Report: Installed application on '{invitee}' node"
                ));
            }
        }

        let application_id = self.application.install(ctx.meroctl, &ctx.inviter).await?;
        if let Some(existing_app_id) = &ctx.application_id {
            if existing_app_id != &application_id {
                bail!(
                    "Application ID mismatch: existing ID is {}, but got {}",
                    existing_app_id,
                    application_id
                );
            }
        }

        if ctx
            .meroctl
            .application_get(&ctx.inviter, &application_id)
            .await?
            .is_null()
        {
            bail!(
                "Failed to lookup installed application on '{}'",
                &ctx.inviter
            );
        }

        ctx.application_id = Some(application_id);

        ctx.output_writer.write_str(&format!(
            "Report: Installed application on '{}' node",
            &ctx.inviter
        ));

        Ok(())
    }
}

impl ApplicationSource {
    /// Installs the application from this source onto the specified node.
    ///
    /// For local files, directly installs the WASM file using meroctl.
    /// For remote URLs, downloads the file (decompressing if it's gzipped),
    /// temporarily stores it locally, installs it, and then cleans up.
    ///
    /// # Arguments
    /// * `meroctl` - The meroctl instance to use for installation
    /// * `node_name` - Name of the node to install the application on
    ///
    /// # Returns
    /// * `String` - Installation result message
    ///
    /// # Errors
    /// * If URL download fails
    /// * If decompression fails for gzipped files
    /// * If temporary file operations fail
    /// * If the meroctl installation fails
    async fn install(&self, meroctl: &Meroctl, node_name: &str) -> EyreResult<String> {
        match self {
            Self::LocalFile(path) => meroctl.application_install(node_name, path).await,
            Self::Url(url) => {
                let response = reqwest::get(url).await?;
                let temp_path = "/tmp/app.wasm";
                let file = File::create(&temp_path).await?;

                if url.ends_with(".gz") {
                    let stream = response.bytes_stream();
                    let (tx, rx) = mpsc::channel::<Result<Bytes, std::io::Error>>(1024);

                    let write_to_file = tokio::spawn(async move {
                        let stream = tokio_stream::wrappers::ReceiverStream::new(rx);
                        let reader = StreamReader::new(stream);
                        let mut decoder = GzipDecoder::new(BufReader::new(reader));
                        let mut file = file;
                        io::copy(&mut decoder, &mut file).await?;
                        file.flush().await?;
                        Ok::<_, io::Error>(())
                    });

                    let process_input = tokio::spawn(async move {
                        let mut stream = stream;
                        while let Some(chunk) = stream.next().await {
                            let chunk = chunk.map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
                            tx.send(Ok(chunk)).await.map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
                        }
                        Ok::<_, io::Error>(())
                    });

                    let (write_result, process_result) = tokio::join!(write_to_file, process_input);
                    write_result??;
                    process_result??;
                } else {
                    let mut file = file;
                    let stream = response.bytes_stream()
                        .map_err(|e| io::Error::new(io::ErrorKind::Other, e));
                    let mut reader = StreamReader::new(stream);
                    io::copy(&mut reader, &mut file).await?;
                    file.flush().await?;
                }

                let result = meroctl.application_install(node_name, &temp_path).await;
                fs::remove_file(&temp_path).await?;
                result
            }
        }
    }
}
