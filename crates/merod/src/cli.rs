use camino::Utf8PathBuf;
use clap::{Parser, Subcommand, Args};
use const_format::concatcp;
use eyre::Result as EyreResult;
use serde::{Deserialize, Serialize};
use std::fs;

use crate::defaults;

#[derive(Debug, Serialize, Deserialize, Clone)]
struct ConfigSchema {
    datastore: DatastoreConfig,
    network: NetworkConfig,
    logging: LoggingConfig,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct DatastoreConfig {
    #[serde(default = "default_datastore_type")]
    r#type: String,
    path: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct NetworkConfig {
    #[serde(default = "default_network_transport")]
    transport: String,
    port: Option<u16>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct LoggingConfig {
    #[serde(default = "default_logging_level")]
    level: String,
    format: Option<String>,
}

fn default_datastore_type() -> String {
    "rocksdb".to_string()
}

fn default_network_transport() -> String {
    "tcp".to_string()
}

fn default_logging_level() -> String {
    "info".to_string()
}

#[derive(Debug, Parser)]
pub struct ConfigCommand {
    /// Edit configuration (section.key=value)
    #[arg(short, long, value_name = "EDIT")]
    edit: Vec<String>,

    /// Output format for configuration
    #[arg(long, value_parser = ["yaml", "json"], default_value = "yaml")]
    output_format: String,

    /// Show configuration hints
    #[arg(long)]
    show_hints: bool,

    #[command(flatten)]
    root_args: RootArgs,
}

impl ConfigCommand {
    pub fn run(&self, root_args: &RootArgs) -> EyreResult<()> {
        // Determine config path
        let config_path = root_args.home.join("config.yaml");

        // Show hints if requested
        if self.show_hints {
            print_config_hints();
            return Ok(());
        }

        // Load existing configuration
        let mut config = load_config(&config_path)?;

        // Handle configuration mutations
        if !self.edit.is_empty() {
            for edit in &self.edit {
                let parts: Vec<&str> = edit.split('=').collect();
                if parts.len() != 2 {
                    return Err(eyre::eyre!("Invalid edit format: {}", edit));
                }

                let (key, value) = (parts[0], parts[1]);
                let key_parts: Vec<&str> = key.split('.').collect();

                match (key_parts.as_slice(), value) {
                    (["datastore", "type"], v) => config.datastore.r#type = v.to_string(),
                    (["datastore", "path"], v) => config.datastore.path = Some(v.to_string()),
                    (["network", "transport"], v) => config.network.transport = v.to_string(),
                    (["network", "port"], v) => config.network.port = v.parse::<u16>().ok(),
                    (["logging", "level"], v) => config.logging.level = v.to_string(),
                    (["logging", "format"], v) => config.logging.format = Some(v.to_string()),
                    _ => return Err(eyre::eyre!("Unknown configuration key: {}", key)),
                }
            }

            // Save updated configuration
            save_config(&config, &config_path)?;
        }

        // Output configuration
        let output = match self.output_format.as_str() {
            "json" => serde_json::to_string_pretty(&config)?,
            _ => serde_yaml::to_string(&config)?,
        };

        println!("{}", output);

        Ok(())
    }
}

fn load_config(config_path: &Utf8PathBuf) -> EyreResult<ConfigSchema> {
    if !config_path.exists() {
        // Return default configuration if file doesn't exist
        return Ok(ConfigSchema {
            datastore: DatastoreConfig {
                r#type: default_datastore_type(),
                path: None,
            },
            network: NetworkConfig {
                transport: default_network_transport(),
                port: None,
            },
            logging: LoggingConfig {
                level: default_logging_level(),
                format: None,
            },
        });
    }

    let config_content = fs::read_to_string(config_path)?;
    let config: ConfigSchema = serde_yaml::from_str(&config_content)?;
    Ok(config)
}

fn save_config(config: &ConfigSchema, config_path: &Utf8PathBuf) -> EyreResult<()> {
    // Ensure directory exists
    if let Some(parent) = config_path.parent() {
        fs::create_dir_all(parent)?;
    }

    let yaml_content = serde_yaml::to_string(config)?;
    fs::write(config_path, yaml_content)?;
    Ok(())
}

fn print_config_hints() {
    println!("Configuration Hints:");
    println!("\nDatastore Configuration:");
    println!("  type: Supported types - rocksdb, memory, leveldb, s3");
    println!("  path: Optional storage path");

    println!("\nNetwork Configuration:");
    println!("  transport: Supported transports - tcp, udp, quic, websocket");
    println!("  port: Optional network port");

    println!("\nLogging Configuration:");
    println!("  level: Supported levels - debug, info, warn, error");
    println!("  format: Optional log format");

    println!("\nExample Mutations:");
    println!("  merod config -e datastore.type=rocksdb");
    println!("  merod config -e network.port=2428");
    println!("  merod config -e logging.level=debug");
}
