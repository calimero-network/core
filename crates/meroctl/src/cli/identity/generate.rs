use calimero_primitives::identity::PrivateKey;
use calimero_server_primitives::admin::GenerateContextIdentityResponse;
use clap::Parser;
use eyre::Result as EyreResult;

use crate::cli::Environment;
use crate::identity::{create_identity, Identity};
use crate::output::Report;

#[derive(Debug, Parser)]
#[command(about = "Generate public/private key pair used for context identity")]
pub struct GenerateCommand {
    #[clap(
        short = 'i',
        long,
        help = "The name of the identity you are going to generate"
    )]
    pub identity_name: Option<String>,
}

impl Report for GenerateContextIdentityResponse {
    fn report(&self) {
        println!("public_key: {}", self.data.public_key);
        println!("private_key: {:?}", self.data.private_key);
    }
}

impl GenerateCommand {
    pub async fn run(self, environment: &Environment) -> EyreResult<()> {
        let private_key = PrivateKey::random(&mut rand::thread_rng());

        let identity_response =
            GenerateContextIdentityResponse::new(private_key.public_key(), private_key);

        let identity = Identity::new(private_key.public_key());

        if let Some(identity_name) = self.identity_name {
            create_identity(identity, &environment, identity_name)?;
        }

        environment.output.write(&identity_response);

        Ok(())
    }
}
