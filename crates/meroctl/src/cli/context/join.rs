use calimero_primitives::context::ContextInvitationPayload;
use calimero_primitives::identity::PrivateKey;
use calimero_server_primitives::admin::JoinContextRequest;
use clap::Parser;
use eyre::{bail, Result as EyreResult};
use reqwest::Client;
use tracing::info;

use crate::cli::RootArgs;
use crate::common::RequestType::POST;
use crate::common::{get_response, multiaddr_to_url};
use crate::config_file::ConfigFile;

#[derive(Debug, Parser)]
pub struct JoinCommand {
    #[clap(value_name = "PRIVATE_KEY")]
    private_key: PrivateKey,
    #[clap(value_name = "INVITE")]
    invitation_payload: ContextInvitationPayload,
}

impl JoinCommand {
    pub async fn run(self, root_args: RootArgs) -> EyreResult<()> {
        let path = root_args.home.join(&root_args.node_name);
        if !ConfigFile::exists(&path) {
            bail!("Config file does not exist")
        }
        let Ok(config) = ConfigFile::load(&path) else {
            bail!("Failed to load config file");
        };
        let Some(multiaddr) = config.network.server.listen.first() else {
            bail!("No address.")
        };

        let url = multiaddr_to_url(multiaddr, "admin-api/dev/contexts/join")?;
        let client = Client::new();
        let response = get_response(
            &client,
            url,
            Some(JoinContextRequest {
                private_key: self.private_key,
                invitation_payload: self.invitation_payload,
            }),
            &config.identity,
            POST,
        )
        .await?;

        if !response.status().is_success() {
            bail!("Request failed with status: {}", response.status())
        }

        info!("Context {} sucesfully joined", self.context_id);

        Ok(())
    }
}
