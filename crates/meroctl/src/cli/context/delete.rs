use calimero_primitives::alias::Alias;
use calimero_primitives::context::ContextId;
use calimero_server_primitives::admin::DeleteContextResponse;
use clap::Parser;
use eyre::{OptionExt, Result as EyreResult};
use reqwest::Client;

use crate::cli::Environment;
use crate::common::{
    do_request, fetch_multiaddr, load_config, multiaddr_to_url, resolve_alias, RequestType,
};
use crate::output::Report;

#[derive(Debug, Parser)]
#[command(about = "Delete a context")]
pub struct DeleteCommand {
    #[clap(name = "CONTEXT", help = "The context to delete")]
    pub context: Alias<ContextId>,
}

impl Report for DeleteContextResponse {
    fn report(&self) {
        println!("is_deleted: {}", self.data.is_deleted);
    }
}

impl DeleteCommand {
    pub async fn run(self, environment: &Environment) -> EyreResult<()> {
        let config = load_config(&environment.args.home, &environment.args.node_name)?;

        let multiaddr = fetch_multiaddr(&config)?;

        let context_id = resolve_alias(multiaddr, &config.identity, self.context, None)
            .await?
            .value()
            .cloned()
            .ok_or_eyre("unable to resolve")?;

        let url = multiaddr_to_url(multiaddr, &format!("admin-api/dev/contexts/{}", context_id))?;

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
