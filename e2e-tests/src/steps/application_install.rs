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
    // CalimeroRegistry(String),
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
        }
    }
}
