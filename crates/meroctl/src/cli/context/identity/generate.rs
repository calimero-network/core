use calimero_server_primitives::admin::GenerateContextIdentityResponse;
use clap::Parser;
use comfy_table::{Cell, Color, Table};
use eyre::Result as EyreResult;

use crate::cli::Environment;
use crate::output::Report;

#[derive(Copy, Clone, Debug, Parser)]
#[command(about = "Generate public/private key pair used for context identity")]
pub struct GenerateCommand;

impl Report for GenerateContextIdentityResponse {
    fn report(&self) {
        let mut table = Table::new();
        let _ = table.set_header(vec![Cell::new("Generated Identity").fg(Color::Blue)]);
        let _ = table.add_row(vec![format!("Public Key: {}", self.data.public_key)]);
        println!("{table}");
    }
}

impl GenerateCommand {
    pub async fn run(self, environment: &Environment) -> EyreResult<()> {
        let connection = environment.connection()?;

        let response: GenerateContextIdentityResponse = connection
            .post("admin-api/dev/identity/context", None::<()>)
            .await?;

        environment.output.write(&response);
        Ok(())
    }
}
