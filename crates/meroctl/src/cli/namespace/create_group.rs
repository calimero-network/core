use clap::Parser;
use eyre::Result;
use serde::{Deserialize, Serialize};

use crate::cli::group::settings::VisibilityModeArg;
use crate::cli::Environment;
use crate::output::Report;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct CreateGroupInNamespaceResponseData {
    group_id: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct CreateGroupInNamespaceResponse {
    data: CreateGroupInNamespaceResponseData,
}

impl Report for CreateGroupInNamespaceResponse {
    fn report(&self) {
        println!("Created group: {}", self.data.group_id);
    }
}

#[derive(Clone, Debug, Parser)]
#[command(about = "Create a child group in a namespace")]
pub struct CreateGroupCommand {
    #[clap(name = "NAMESPACE_ID", help = "The hex-encoded namespace ID")]
    pub namespace_id: String,

    #[clap(long, help = "Optional alias for the newly created group")]
    pub alias: Option<String>,

    #[clap(
        long,
        value_enum,
        help = "Visibility at creation (default: open). Pass `restricted` for a \
                private group"
    )]
    pub visibility: Option<VisibilityModeArg>,
}

impl CreateGroupCommand {
    pub async fn run(self, environment: &mut Environment) -> Result<()> {
        let visibility = self.visibility.map(|mode| {
            match mode {
                VisibilityModeArg::Open => "open",
                VisibilityModeArg::Restricted => "restricted",
            }
            .to_owned()
        });
        let response = environment
            .client()?
            .create_group_in_namespace(&self.namespace_id, self.alias, visibility)
            .await?;
        let response: CreateGroupInNamespaceResponse = serde_json::from_value(response)
            .map_err(|err| eyre::eyre!("invalid response: {err}"))?;

        environment.output.write(&response);

        Ok(())
    }
}
