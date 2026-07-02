#![allow(unused_results, reason = "Occurs in macro")]

use std::env::temp_dir;
use std::str::FromStr;

use calimero_config::{ConfigFile, CONFIG_FILE};
use camino::Utf8PathBuf;
use clap::Parser;
use eyre::{bail, eyre, Result as EyreResult};
use tokio::fs::{read_to_string, write};
use toml_edit::{Item, Value};
use tracing::info;

use crate::cli;

/// Configure the node
#[derive(Debug, Parser)]
pub struct ConfigCommand {
    /// Key-value pairs to be added or updated in the TOML file
    #[clap(value_name = "ARGS")]
    args: Vec<KeyValuePair>,
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

impl ConfigCommand {
    pub async fn run(self, root_args: &cli::RootArgs) -> EyreResult<()> {
        let path = root_args.home.join(&root_args.node_name);

        if !ConfigFile::exists(&path) {
            bail!("Node is not initialized in {:?}", path);
        }

        let path = path.join(CONFIG_FILE);

        // Load the existing TOML file
        let toml_str = read_to_string(&path)
            .await
            .map_err(|_| eyre!("Node is not initialized in {:?}", path))?;

        let mut doc = toml_str.parse::<toml_edit::DocumentMut>()?;

        // Update the TOML document. Navigate the dotted key path via table-like
        // lookups instead of `Index`/`IndexMut`, which panic when a path segment
        // resolves to a non-table (e.g. `foo.bar=1` where `foo` is a string).
        for kv in self.args.iter() {
            let key_parts: Vec<&str> = kv.key.split('.').collect();
            let (last, parents) = key_parts
                .split_last()
                .expect("split('.') always yields at least one segment");

            let mut current = doc.as_item_mut();
            for key in parents {
                let table = current
                    .as_table_like_mut()
                    .ok_or_else(|| eyre!("cannot set '{}': '{key}' is not a table", kv.key))?;
                if table.get(key).is_none() {
                    table.insert(key, Item::Table(toml_edit::Table::new()));
                }
                current = table
                    .get_mut(key)
                    .expect("entry inserted above must be present");
            }

            let table = current.as_table_like_mut().ok_or_else(|| {
                eyre!("cannot set '{}': parent of '{last}' is not a table", kv.key)
            })?;
            table.insert(last, Item::Value(kv.value.clone()));
        }

        self.validate_toml(&doc).await?;

        // Save the updated TOML back to the file
        write(&path, doc.to_string()).await?;

        info!("Node configuration has been updated");

        Ok(())
    }

    pub async fn validate_toml(self, doc: &toml_edit::DocumentMut) -> EyreResult<()> {
        let tmp_dir = temp_dir();
        let tmp_path = tmp_dir.join(CONFIG_FILE);

        write(&tmp_path, doc.to_string()).await?;

        let tmp_path_utf8 = Utf8PathBuf::try_from(tmp_dir)?;

        drop(ConfigFile::load(&tmp_path_utf8).await?);

        Ok(())
    }
}
