use crate::cli::Environment;
use calimero_primitives::alias::Alias;
use calimero_primitives::context::ContextId;
use clap::Parser;
use eyre::{Result as EyreResult, WrapErr};

use crate::common::{create_alias, fetch_multiaddr, load_config};

#[derive(Debug, Parser)]
#[command(name = "use", about = "Set the default context")]
pub struct UseCommand {
    /// The context id to set as default
    pub context_id: ContextId,
}

impl UseCommand {
    pub async fn run(self, environment: &Environment) -> EyreResult<()> {
        // Load config and get multiaddr
        let config = load_config(&environment.args.home, &environment.args.node_name)?;
        let multiaddr = fetch_multiaddr(&config)?;
        
        // Create "default" alias for the specified context ID
        let default_alias: Alias<ContextId> = "default".parse()
            .expect("'default' is a valid alias name");
        
        // Create or update the "default" alias using the common helper function
        let res = create_alias(multiaddr, &config.identity, default_alias, None, self.context_id)
            .await
            .wrap_err("Failed to set default context")?;
        
        // Output the result
        environment.output.write(&res);
        
        println!("Default context set to: {}", self.context_id);
        Ok(())
    }
}