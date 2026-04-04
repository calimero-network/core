use clap::Parser;
use serde::{Deserialize, Serialize};
use eyre::Result;

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
        let request = CreateGroupInNamespaceBody {
            group_alias: self.alias,
        };

        let client = environment.client()?;
        let url = client
            .api_url()
            .join(&format!("/admin-api/namespaces/{}/groups", self.namespace_id))
            .map_err(|err| eyre::eyre!("failed to build namespace create-group URL: {err}"))?;
        let raw = reqwest::Client::new()
            .post(url)
            .json(&request)
            .send()
            .await?;
        if !raw.status().is_success() {
            let status = raw.status();
            let body = raw.text().await.unwrap_or_default();
            return Err(eyre::eyre!("request failed with status {status}: {body}"));
        }
        let response: CreateGroupInNamespaceResponse = raw
            .json()
            .await
            .map_err(|err| eyre::eyre!("invalid response: {err}"))?;

        println!("{}", serde_json::to_string_pretty(&response)?);

        Ok(())
    }
}
