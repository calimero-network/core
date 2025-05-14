use std::env::var;

use clap::Parser;
use eyre::Result as EyreResult;
use tracing_subscriber::fmt::layer;
use tracing_subscriber::prelude::*;
use tracing_subscriber::{registry, EnvFilter};

use crate::cli::RootCommand;
use crate::config::{ConfigFile, OutputFormat}; // Import the config module

mod cli;
mod defaults;
mod config; // Make sure config.rs is included

#[tokio::main]
async fn main() -> EyreResult<()> {
    setup()?;

    // Parse the root command
    let command = RootCommand::parse();

    // Execute the parsed command (which includes `ConfigCommand`)
    command.run().await
}

fn setup() -> EyreResult<()> {
    registry()
        .with(EnvFilter::builder().parse(format!(
            "merod=info,calimero_=info,{}",
            var("RUST_LOG").unwrap_or_default()
        ))?)
        .with(layer())
        .init();

    color_eyre::install()?;

    Ok(())
}

// RootCommand to handle configuration commands and others
#[derive(Parser)]
#[command(name = "Merod CLI")]
#[command(author = "Kiran Rao <your-email>")]
#[command(version = "1.0")]
#[command(about = "Merod CLI Tool")]
pub struct RootCommand {
    #[command(subcommand)]
    pub config: Option<ConfigCommand>, // Subcommand for handling config-related commands
}

#[derive(Parser)]
pub enum ConfigCommand {
    /// Print the configuration in the specified format (JSON or Pretty)
    #[command(about = "Print the configuration")]
    Print {
        #[arg(short, long, value_enum, default_value_t = OutputFormat::Pretty)]
        format: OutputFormat, // Option to choose the output format (Pretty or JSON)
    },

    /// Save the configuration to the file
    #[command(about = "Save the configuration")]
    Save {},

    /// Get the value of a specific config key
    #[command(about = "Get the value of a specific config key")]
    Key {
        #[arg(value_name = "KEY")]
        key: String, // Key to retrieve
    },

    /// Show the hint for a specific config key
    #[command(about = "Show the hint for a specific config key")]
    Hint {
        #[arg(value_name = "KEY")]
        key: String, // Key to get a hint for
    },

    /// Show editable keys with example values
    #[command(about = "Show editable config keys")]
    EditableKeys {},
}

impl ConfigCommand {
    pub async fn run(self) -> EyreResult<()> {
        let config_dir = var("CONFIG_DIR").unwrap_or_else(|_| ".".to_string()); // Default to current directory if not set
        let config_path = std::path::PathBuf::from(config_dir);

        // Load the configuration file
        let config = ConfigFile::load(&config_path).unwrap_or_else(|_| {
            ConfigFile::new(
                libp2p_identity::Keypair::generate_ed25519(),
                Default::default(),
                Default::default(),
                Default::default(),
                Default::default(),
                calimero_context::config::ContextConfig::default(),
            )
        });

        match self {
            ConfigCommand::Print { format } => {
                config.print(format)?;
            }
            ConfigCommand::Save {} => {
                config.save(&config_path)?;
                println!("Configuration saved successfully.");
            }
            ConfigCommand::Key { key } => {
                if let Some(value) = config.get_value(&key) {
                    println!("Value for {}: {}", key, value);
                } else {
                    println!("Key '{}' not found in the config.", key);
                }
            }
            ConfigCommand::Hint { key } => {
                config.print_hint_for_key(&key);
            }
            ConfigCommand::EditableKeys {} => {
                config.print_hints(); // Print all editable keys with example values
            }
        }

        Ok(())
    }
}
