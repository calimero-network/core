#![allow(unused_results, reason = "Occurs in macro")]

use std::env::temp_dir;
use std::fs::{read_to_string, write};
use std::str::FromStr;

use calimero_config::{ConfigFile, CONFIG_FILE};
use camino::Utf8PathBuf;
use clap::{value_parser, Parser};
use eyre::{bail, eyre, Result as EyreResult};
use toml_edit::{Item, Value};
use tracing::info;

use crate::cli;

/// Configure the node
#[derive(Debug, Parser)]
pub struct ConfigCommand {
    /// Key-value pairs to be added or updated in the TOML file
    #[clap(short, long, value_parser = value_parser!(KeyValuePair))]
    arg: Vec<KeyValuePair>,
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
    pub fn run(self, root_args: &cli::RootArgs) -> EyreResult<()> {
        let path = root_args.home.join(&root_args.node_name);

        if !ConfigFile::exists(&path) {
            bail!("Node is not initialized in {:?}", path);
        }

        let path = path.join(CONFIG_FILE);

        // Load the existing TOML file
        let toml_str =
            read_to_string(&path).map_err(|_| eyre!("Node is not initialized in {:?}", path))?;
        let mut doc = toml_str.parse::<toml_edit::DocumentMut>()?;

        // Update the TOML document
        for kv in self.arg.iter() {
            let key_parts: Vec<&str> = kv.key.split('.').collect();

            let mut current = doc.as_item_mut();

            for key in &key_parts[..key_parts.len() - 1] {
                current = &mut current[key];
            }

            current[key_parts[key_parts.len() - 1]] = Item::Value(kv.value.clone());
        }

        self.validate_toml(&doc)?;

        // Save the updated TOML back to the file
        write(&path, doc.to_string())?;

        info!("Node configuration has been updated");

        Ok(())
    }

    pub fn validate_toml(self, doc: &toml_edit::DocumentMut) -> EyreResult<()> {
        let tmp_dir = temp_dir();
        let tmp_path = tmp_dir.join(CONFIG_FILE);

        write(&tmp_path, doc.to_string())?;

        let tmp_path_utf8 = Utf8PathBuf::try_from(tmp_dir)?;

        drop(ConfigFile::load(&tmp_path_utf8)?);

        Ok(())
    }
}
