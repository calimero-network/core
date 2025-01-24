use std::fs::File;
use std::io::BufWriter;

use calimero_primitives::identity::PrivateKey;
use calimero_server_primitives::admin::GenerateContextIdentityResponse;
use clap::Parser;
use eyre::Result as EyreResult;

use crate::cli::Environment;
use crate::output::Report;

#[derive(Debug, Parser)]
#[command(about = "Generate public/private key pair used for context identity")]
pub struct GenerateCommand {
    #[clap(
        short,
        long,
        help = "The name of the identity you are going to generate"
    )]
    pub name: Option<String>,
}

impl Report for GenerateContextIdentityResponse {
    fn report(&self) {
        println!("public_key: {}", self.data.public_key);
        println!("private_key: {}", self.data.private_key);
    }
}

impl GenerateCommand {
    pub async fn run(self, environment: &Environment) -> EyreResult<()> {
        let private_key = PrivateKey::random(&mut rand::thread_rng());

        let response = GenerateContextIdentityResponse::new(private_key.public_key(), private_key);

        if let Some(identity_name) = self.name {
            let path = &environment
                .args
                .home
                .join(&environment.args.node_name)
                .join(format!("{}.identity", identity_name));

            let file_writer = BufWriter::new(File::create(path)?);

            serde_json::to_writer(file_writer, &response)?;
        }

        environment.output.write(&response);

        Ok(())
    }
}
