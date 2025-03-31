use async_compression::tokio::bufread::GzipDecoder;
use eyre::{bail, Result as EyreResult};
use futures_util::TryStreamExt;
use serde::{Deserialize, Serialize};
use tokio::fs::{self, File};
use tokio::io::{self, AsyncWriteExt};
use tokio_util::io::StreamReader;

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
                let mut file = File::create(&temp_path).await?;

                if url.ends_with(".gz") {
                    let stream = response
                        .bytes_stream()
                        .map_err(|e| io::Error::new(io::ErrorKind::Other, e));

                    let reader = StreamReader::new(stream);

                    let mut decoder = GzipDecoder::new(reader);
                    io::copy(&mut decoder, &mut file).await?;
                    file.flush().await?;
                } else {
                    let mut file = file;
                    let stream = response
                        .bytes_stream()
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
