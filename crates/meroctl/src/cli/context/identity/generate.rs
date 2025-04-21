use calimero_primitives::identity::PrivateKey;
use calimero_server_primitives::admin::GenerateContextIdentityResponse;
use clap::Parser;
use color_eyre::owo_colors::OwoColorize;
use eyre::Result as EyreResult;

use crate::cli::Environment;
use crate::output::Report;

#[derive(Debug, Parser)]
#[command(about = "Generate public/private key pair used for context identity")]
pub struct GenerateCommand;

impl Report for GenerateContextIdentityResponse {
    fn report(&self) {
        println!("{} {}", "✓".green(), "Key pair generated".bold());
        println!("  Public Key: {}", self.data.public_key.to_string().cyan());
        println!(
            "  Private Key: {}",
            self.data.private_key.to_string().cyan()
        );
    }
}

impl GenerateCommand {
    pub async fn run(self, environment: &Environment) -> EyreResult<()> {
        let private_key = PrivateKey::random(&mut rand::thread_rng());
        let response = GenerateContextIdentityResponse::new(private_key.public_key(), private_key);
        environment.output.write(&response);

        Ok(())
    }
}
