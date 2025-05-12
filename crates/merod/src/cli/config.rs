#![allow(unused_results, reason = "Occurs in macro")]

use std::env::temp_dir;
use std::fs::{read_to_string, write};
use std::str::FromStr;

use calimero_config::{ConfigFile, CONFIG_FILE};
use camino::Utf8PathBuf;
use clap::Parser;
use eyre::{bail, eyre, Result as EyreResult};
use toml_edit::{Item, Value};
use tracing::info;

/// Configure the node
#[derive(Debug, Parser)]
#[command(
    about = "Update or inspect node configuration",
    after_help = r#"Examples:
  # View the current config
  merod config

  # View config in JSON format
  merod config --output-format json

  # Show editable keys and their allowed values
  merod config --show-hints

  # Edit configuration
  merod config network.port=2428 logging.level=debug
"#
)]
pub struct ConfigCommand {
    /// Key-value pairs to add or update (e.g. network.port=2428)
    #[clap(value_name = "ARGS")]
    args: Vec<KeyValuePair>,

    /// Output format when printing config (yaml or json)
    #[clap(long, value_parser = ["yaml", "json"], default_value = "yaml")]
    output_format: String,

    /// Show editable keys and value hints
    #[clap(long)]
    show_hints: bool,
}

#[derive(Clone, Debug)]
struct KeyValuePair {
    key: String,
    value: Value,
}

impl FromStr for KeyValuePair {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let mut parts = s.splitn(2, '=');
        let key = parts.next().ok_or("Missing key")?.to_owned();
        let value = parts.next().ok_or("Missing value")?;
        let value = Value::from_str(value).map_err(|e| e.to_string())?;

        Ok(Self { key, value })
    }
}

#[warn(unused_results)]
impl ConfigCommand {
    pub fn run(self, root_args: &crate::cli::RootArgs) -> EyreResult<()> {
        let path = root_args.home.join(&root_args.node_name);

        if !ConfigFile::exists(&path) {
            bail!("Node is not initialized in {:?}", path);
        }

        let config_path = path.join(CONFIG_FILE);
        let toml_str = read_to_string(&config_path)
            .map_err(|_| eyre!("Failed to read config from {:?}", config_path))?;
        let mut doc = toml_str.parse::<toml_edit::DocumentMut>()?;

        // Show hints if requested
        if self.show_hints {
            print_hints();
            return Ok(());
        }

        // If no args, just print the config
        if self.args.is_empty() {
            return self.print_config(&doc);
        }

        // Apply mutations
        for kv in &self.args {
            let key_parts: Vec<&str> = kv.key.split('.').collect();
            let mut current = doc.as_item_mut();
            for part in &key_parts[..key_parts.len() - 1] {
                current = &mut current[part];
            }
            current[key_parts[key_parts.len() - 1]] = Item::Value(kv.value.clone());
        }

        self.validate_toml(&doc)?;
        write(&config_path, doc.to_string())?;
        info!("Node configuration has been updated");
        Ok(())
    }

    fn print_config(&self, doc: &toml_edit::DocumentMut) -> EyreResult<()> {
        match self.output_format.as_str() {
            "json" => {
                let value: toml::Value = doc.clone().into();
                let json = serde_json::to_string_pretty(&value)?;
                println!("{}", json);
            }
            "yaml" => {
                let value: toml::Value = doc.clone().into();
                let yaml = serde_yaml::to_string(&value)?;
                println!("{}", yaml);
            }
            _ => bail!("Unsupported output format"),
        }
        Ok(())
    }

    fn validate_toml(&self, doc: &toml_edit::DocumentMut) -> EyreResult<()> {
        let tmp_dir = temp_dir();
        let tmp_path = tmp_dir.join(CONFIG_FILE);
        write(&tmp_path, doc.to_string())?;
        let tmp_path_utf8 = Utf8PathBuf::try_from(tmp_dir)?;
        drop(ConfigFile::load(&tmp_path_utf8)?);
        Ok(())
    }
}

fn print_hints() {
    println!("Editable Configuration Keys and Hints:\n");

    println!("  datastore.type       = rocksdb | memory | leveldb | s3");
    println!("  datastore.path       = string (path to store DB)");
    println!("  network.transport    = tcp | udp | quic | websocket");
    println!("  network.port         = number (e.g. 2428)");
    println!("  logging.level        = debug | info | warn | error");
    println!("  logging.format       = json | text | pretty");
}
