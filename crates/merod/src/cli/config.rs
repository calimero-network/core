#![allow(unused_results, reason = "Occurs in macro")]

use std::str::FromStr;

use calimero_config::{write_atomic, ConfigFile, CONFIG_FILE};
use camino::Utf8Path;
use clap::Parser;
use eyre::{bail, eyre, Result as EyreResult};
use tokio::fs::read_to_string;
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
        let home = root_args.home.join(&root_args.node_name);

        if !ConfigFile::exists(&home) {
            bail!("Node is not initialized in {:?}", home);
        }

        let config_path = home.join(CONFIG_FILE);

        // Load the existing TOML file
        let toml_str = read_to_string(&config_path)
            .await
            .map_err(|_| eyre!("Node is not initialized in {:?}", config_path))?;

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
                // `entry` inserts an empty table only when the segment is
                // absent, and returns the existing value otherwise. A pre-
                // existing non-table value is preserved (not overwritten) and
                // surfaces as an error on the next `as_table_like_mut` call.
                current = table
                    .entry(key)
                    .or_insert_with(|| Item::Table(toml_edit::Table::new()));
            }

            let table = current.as_table_like_mut().ok_or_else(|| {
                eyre!("cannot set '{}': parent of '{last}' is not a table", kv.key)
            })?;
            table.insert(last, Item::Value(kv.value.clone()));
        }

        self.validate_toml(&doc, &home).await?;

        // Live node's config holds its private key; write atomically.
        write_atomic(&config_path, doc.to_string()).await?;

        info!("Node configuration has been updated");

        Ok(())
    }

    /// Validate the candidate config the same way `merod run` does, so a
    /// `merod config` write cannot persist values the node would reject at
    /// startup. Previously this only round-tripped the deserialize (catching
    /// type errors) but let semantically-invalid values through — e.g. a
    /// zeroed sync deadline, a port conflict, or an out-of-range limit —
    /// which then failed at the next `run`. We load the candidate from a temp
    /// file and run the full [`validate_config`], passing the real node home
    /// so path-accessibility checks are meaningful.
    pub async fn validate_toml(
        &self,
        doc: &toml_edit::DocumentMut,
        home: &camino::Utf8Path,
    ) -> EyreResult<()> {
        // Candidate holds the private key: stage in a private 0700 dir, not
        // a shared, predictably-named file under the system temp dir.
        let tmp_dir = tempfile::tempdir()?;
        let tmp_dir_utf8 = Utf8Path::from_path(tmp_dir.path())
            .ok_or_else(|| eyre!("temp dir path is not valid UTF-8"))?;

        write_atomic(&tmp_dir_utf8.join(CONFIG_FILE), doc.to_string()).await?;

        let config = ConfigFile::load(tmp_dir_utf8).await?;
        crate::cli::validation::validate_config(&config, home)?;

        Ok(())
    }
}
