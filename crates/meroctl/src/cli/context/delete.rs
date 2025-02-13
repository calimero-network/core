use calimero_primitives::alias::Kind;
use calimero_primitives::context::ContextId;
use calimero_server_primitives::admin::DeleteContextResponse;
use clap::Parser;
use eyre::Result as EyreResult;
use reqwest::Client;

use crate::cli::Environment;
use crate::common::{
    do_request, fetch_multiaddr, load_config, multiaddr_to_url, resolve_identifier, RequestType,
};
use crate::output::Report;

#[derive(Debug, Parser)]
#[command(about = "Delete an context")]
pub struct DeleteCommand {
    #[clap(
        name = "CONTEXT_ID",
        help = "ContextId or alias of the context to delete"
    )]
    pub context_id: String,
}

impl Report for DeleteContextResponse {
    fn report(&self) {
        println!("is_deleted: {}", self.data.is_deleted);
    }
}

impl DeleteCommand {
    pub async fn run(self, environment: &Environment) -> EyreResult<()> {
        let config = load_config(&environment.args.home, &environment.args.node_name)?;

        let context_id: ContextId =
            resolve_identifier(&config, &self.context_id, Kind::Context, None)
                .await?
                .into();

        let url = multiaddr_to_url(
            fetch_multiaddr(&config)?,
            &format!("admin-api/dev/contexts/{}", context_id),
        )?;

        let response: DeleteContextResponse = do_request(
            &Client::new(),
            url,
            None::<()>,
            &config.identity,
            RequestType::Delete,
        )
        .await?;

        environment.output.write(&response);

        Ok(())
    }
}
