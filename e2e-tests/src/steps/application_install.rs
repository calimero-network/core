use eyre::{bail, Result as EyreResult};
use serde::{Deserialize, Serialize};

use crate::driver::{Test, TestContext};
use crate::meroctl::Meroctl;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApplicationInstallStep {
    pub application: ApplicationSource,
    pub target: ApplicationInstallTarget,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ApplicationSource {
    LocalFile(String),
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
    async fn install(&self, meroctl: &Meroctl, node_name: &str) -> EyreResult<String> {
        match self {
            Self::LocalFile(path) => meroctl.application_install(node_name, path).await,
            Self::Url(url) => {
                // Download the file
                let response = reqwest::get(url).await?;
                let bytes = response.bytes().await?;
                
                let decoded_bytes = if url.ends_with(".gz") {
                    use std::io::Read;
                    let mut decoder = flate2::read::GzDecoder::new(&bytes[..]);
                    let mut decompressed = Vec::new();
                    decoder.read_to_end(&mut decompressed)?;
                    decompressed
                } else {
                    bytes.to_vec()
                };
                
                // Use a simple temp file name
                let temp_path = "/tmp/app.wasm";
                
                // Save to temporary file
                tokio::fs::write(&temp_path, decoded_bytes).await?;

                // Install using the downloaded file
                let result = meroctl.application_install(node_name, &temp_path).await;
                
                // Clean up
                tokio::fs::remove_file(&temp_path).await?;
                
                result
            }
        }
    }
}
