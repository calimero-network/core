use clap::Parser;
use eyre::Result;
use serde::{Deserialize, Serialize};

use crate::cli::Environment;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct CreateGroupInNamespaceBody {
    group_alias: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateGroupInNamespaceResponse {
    group_id: String,
}

#[derive(Clone, Debug, Parser)]
#[command(about = "Create a child group in a namespace")]
pub struct CreateGroupCommand {
    #[clap(name = "NAMESPACE_ID", help = "The hex-encoded namespace ID")]
    pub namespace_id: String,

    #[clap(long, help = "Optional alias for the newly created group")]
    pub alias: Option<String>,
}

impl CreateGroupCommand {
    pub async fn run(self, environment: &mut Environment) -> Result<()> {
        let response = environment
            .client()?
            .create_group_in_namespace(&self.namespace_id, self.alias)
            .await?;
        let response: CreateGroupInNamespaceResponse = serde_json::from_value(response)
            .map_err(|err| eyre::eyre!("invalid response: {err}"))?;

        println!("{}", serde_json::to_string_pretty(&response)?);

        Ok(())
    }
}
