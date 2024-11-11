use calimero_primitives::application::ApplicationId;
use calimero_primitives::context::ContextId;
use calimero_primitives::identity::PublicKey;
use calimero_server_primitives::admin::{
    InstallApplicationResponse, InstallDevApplicationRequest, UpdateContextApplicationRequest,
    UpdateContextApplicationResponse,
};
use camino::Utf8PathBuf;
use clap::Parser;
use eyre::Result as EyreResult;
use libp2p::identity::Keypair;
use libp2p::Multiaddr;
use reqwest::Client;

use crate::cli::Environment;
use crate::common::{do_request, fetch_multiaddr, load_config, multiaddr_to_url, RequestType};

#[derive(Debug, Parser)]
#[command(about = "Update app in context")]
pub struct UpdateCommand {
    #[clap(
        long,
        short = 'a',
        help = "The application ID to update in the context"
    )]
    application_id: Option<ApplicationId>,

    #[clap(long, short = 'c', help = "ContextId where to install the application")]
    context_id: ContextId,

    #[clap(
        long,
        short = 'p',
        help = "PublicKey needed for the application installation"
    )]
    member_public_key: PublicKey,

    #[clap(
        long,
        help = "Path to the application file to watch and install locally"
    )]
    path: Utf8PathBuf,

    #[clap(long, help = "Metadata needed for the application installation")]
    metadata: Option<String>,
}

impl UpdateCommand {
    pub async fn run(self, environment: &Environment) -> EyreResult<()> {
        let config = load_config(&environment.args.home, &environment.args.node_name)?;
        let multiaddr = fetch_multiaddr(&config)?;
        let client = Client::new();

        let metadata = self.metadata.map(String::into_bytes);

        install_app_and_update_context(
            environment,
            &client,
            multiaddr,
            self.path,
            self.context_id,
            metadata,
            &config.identity,
            self.member_public_key,
        )
        .await?;

        Ok(())
    }
}

async fn install_app_and_update_context(
    environment: &Environment,
    client: &Client,
    base_multiaddr: &Multiaddr,
    path: Utf8PathBuf,
    context_id: ContextId,
    metadata: Option<Vec<u8>>,
    keypair: &Keypair,
    member_public_key: PublicKey,
) -> EyreResult<()> {
    let application_id = install_app(
        environment,
        client,
        base_multiaddr,
        path.clone(),
        metadata.clone(),
        keypair,
    )
    .await?;

    update_context_application(
        environment,
        client,
        base_multiaddr,
        context_id,
        application_id,
        keypair,
        member_public_key,
    )
    .await?;

    Ok(())
}

async fn install_app(
    environment: &Environment,
    client: &Client,
    base_multiaddr: &Multiaddr,
    path: Utf8PathBuf,
    metadata: Option<Vec<u8>>,
    keypair: &Keypair,
) -> EyreResult<ApplicationId> {
    let url = multiaddr_to_url(base_multiaddr, "admin-api/dev/install-dev-application")?;

    let request = InstallDevApplicationRequest::new(path, metadata.unwrap_or_default());

    let response: InstallApplicationResponse =
        do_request(client, url, Some(request), keypair, RequestType::Post).await?;

    environment.output.write(&response);

    Ok(response.data.application_id)
}

async fn update_context_application(
    environment: &Environment,
    client: &Client,
    base_multiaddr: &Multiaddr,
    context_id: ContextId,
    application_id: ApplicationId,
    keypair: &Keypair,
    member_public_key: PublicKey,
) -> EyreResult<()> {
    let url = multiaddr_to_url(
        base_multiaddr,
        &format!("admin-api/dev/contexts/{context_id}/application"),
    )?;

    let request = UpdateContextApplicationRequest::new(application_id, member_public_key);

    let response: UpdateContextApplicationResponse =
        do_request(client, url, Some(request), keypair, RequestType::Post).await?;

    environment.output.write(&response);

    Ok(())
}
