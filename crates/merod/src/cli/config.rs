#![allow(unused_results, reason = "Occurs in macro")]

use std::env::temp_dir;
use std::str::FromStr;

use calimero_config::{ConfigFile, CONFIG_FILE};
use camino::Utf8PathBuf;
use clap::Parser;
use colored::*;
use eyre::{bail, eyre, Result as EyreResult};
use schemars::schema_for;
use similar::{ChangeTag, TextDiff};
use tokio::fs::{read_to_string, write};
use toml_edit::{DocumentMut, Item, Value};
use tracing::info;

use crate::cli;

/// Configure the node
#[derive(Debug, Parser)]
pub struct ConfigCommand {
    /// Key-value pairs to be added or updated in the TOML file, or keys with ? for hints
    #[clap(value_name = "ARGS")]
    args: Vec<String>,

    /// Output format for printing
    #[clap(long, value_name = "FORMAT", default_value = "default")]
    #[clap(value_enum)]
    print: PrintFormat,

    /// Save modifications to config file
    #[clap(short, long)]
    save: bool,
}

#[derive(Copy, Clone, Debug, clap::ValueEnum)]
enum PrintFormat {
    Default,
    Toml,
    Json,
    Human,
}

#[derive(Clone, Debug)]
enum ConfigArg {
    Mutation { key: String, value: Value },
    Hint { key: String },
}

impl FromStr for ConfigArg {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.ends_with('?') {
            let key = s.trim_end_matches('?').to_owned();
            if key.is_empty() {
                return Err("Empty key for hint".to_owned());
            }
            return Ok(ConfigArg::Hint { key });
        }

        let mut parts = s.splitn(2, '=');
        let key = parts.next().ok_or("Missing key")?.to_owned();

        let value = parts.next().ok_or("Missing value")?;
        let value = Value::from_str(value).map_err(|e| e.to_string())?;

        Ok(ConfigArg::Mutation { key, value })
    }
}

impl ConfigCommand {
    pub async fn run(self, root_args: &cli::RootArgs) -> EyreResult<()> {
        let path = root_args.home.join(&root_args.node_name);

        if !ConfigFile::exists(&path) {
            bail!("Node is not initialized in {:?}", path);
        }

        let config_path = path.join(CONFIG_FILE);

        // Load the existing TOML file
        let toml_str = read_to_string(&config_path)
            .await
            .map_err(|_| eyre!("Node is not initialized in {:?}", config_path))?;

        let mut doc = toml_str.parse::<DocumentMut>()?;

        // Parse arguments
        let mut mutations = Vec::new();
        let mut hints = Vec::new();
        let mut has_hints = false;

        for arg in &self.args {
            match ConfigArg::from_str(arg) {
                Ok(ConfigArg::Mutation { key, value }) => {
                    if has_hints {
                        eprintln!(
                            "Warning: Ignoring mutation '{}' because hints are present",
                            key
                        );
                        continue;
                    }
                    mutations.push((key, value));
                }
                Ok(ConfigArg::Hint { key }) => {
                    has_hints = true;
                    hints.push(key);
                }
                Err(err) => {
                    bail!("Invalid argument '{}': {}", arg, err);
                }
            }
        }

        // Handle hints
        if has_hints {
            if !mutations.is_empty() {
                eprintln!("Warning: Mutations are ignored when hints are present");
            }
            return self.handle_hints(&hints).await;
        }

        if mutations.is_empty() {
            let filter_keys: Vec<String> = self
                .args
                .iter()
                .filter(|arg| !arg.contains('=') && !arg.ends_with('?'))
                .cloned()
                .collect();

            return self.print_config(&doc, &filter_keys).await;
        }

        // Handle mutations
        let original_doc = doc.clone();

        for (key, value) in &mutations {
            if let Err(e) = self.apply_mutation(&mut doc, &key, value.clone()) {
                bail!("Failed to apply mutation '{}': {}", key, e);
            }
        }

        // Validate the modified config
        self.validate_config(&doc).await?;

        // Show diff or modified config based on print format
        self.show_result(&original_doc, &doc).await?;

        // Save if requested
        if self.save {
            write(&config_path, doc.to_string()).await?;
            info!("Node configuration has been updated");
        } else if mutations.is_empty() {
            // Only print warning if no changes were made but save was requested
            if self.save {
                eprintln!("Warning: No changes to save");
            }
        } else {
            eprintln!(
                "\nnote: if this looks right, use `-s, --save` to persist these modifications"
            );
        }

        Ok(())
    }

    fn apply_mutation(&self, doc: &mut DocumentMut, key: &str, value: Value) -> EyreResult<()> {
        let key_parts: Vec<&str> = key.split('.').collect();
        let mut current = doc.as_item_mut();

        // Navigate to the parent of the target key
        for key_part in &key_parts[..key_parts.len() - 1] {
            if !current[*key_part].is_table() {
                current[*key_part] = Item::Table(toml_edit::Table::new());
            }
            current = &mut current[*key_part];
        }

        let final_key = key_parts[key_parts.len() - 1];
        current[final_key] = Item::Value(value);

        Ok(())
    }

    async fn handle_hints(&self, hints: &[String]) -> EyreResult<()> {
        match self.print {
            PrintFormat::Default | PrintFormat::Human => {
                for hint_key in hints {
                    self.print_schema_hint(hint_key).await?;
                }
            }
            PrintFormat::Toml => {
                let mut doc = DocumentMut::new();
                for hint_key in hints {
                    doc[hint_key] = Item::Value(Value::from("<?>"));
                }
                println!("{}", doc.to_string());
            }
            PrintFormat::Json => {
                let mut result = serde_json::Map::new();
                for hint_key in hints {
                    if let Some(schema) = self.get_schema_for_key(hint_key).await {
                        result.insert(hint_key.clone(), schema);
                    } else {
                        result.insert(
                            hint_key.clone(),
                            serde_json::json!({
                                "type": "unknown",
                                "description": "Unknown config key"
                            }),
                        );
                    }
                }
                println!("{}", serde_json::to_string_pretty(&result)?);
            }
        }
        Ok(())
    }

    async fn get_schema_for_key(&self, key: &str) -> Option<serde_json::Value> {
        let schema = schema_for!(ConfigFile);
        let schema_value = serde_json::to_value(schema).ok()?;

        if let Some(properties) = schema_value.get("properties") {
            if let Some(key_schema) = properties.get(key) {
                return Some(key_schema.clone());
            }
        }

        None
    }

    async fn print_schema_hint(&self, key: &str) -> EyreResult<()> {
        // Basic schema-based hint system
        if let Some(schema) = self.get_schema_for_key(key).await {
            if let Some(description) = schema.get("description") {
                if let Some(description_str) = description.as_str() {
                    println!(
                        "{}: {} # {}",
                        key,
                        get_type_from_schema(&schema),
                        description_str
                    );
                    return Ok(());
                }
            }
            println!("{}: {}", key, get_type_from_schema(&schema));
        } else {
            println!("{}: unknown config key", key);
        }
        Ok(())
    }

    async fn print_config(&self, doc: &DocumentMut, keys: &[String]) -> EyreResult<()> {
        if keys.is_empty() {
            // Print full config
            match self.print {
                PrintFormat::Default | PrintFormat::Toml => {
                    println!("{}", doc.to_string());
                }
                PrintFormat::Json => {
                    let value: serde_json::Value = toml::from_str(&doc.to_string())?;
                    println!("{}", serde_json::to_string_pretty(&value)?);
                }
                PrintFormat::Human => {
                    self.print_human_readable(doc).await?;
                }
            }
        } else {
            // Print specific keys by building a filtered document
            let mut result_doc = DocumentMut::new();

            for key in keys {
                let key_parts: Vec<&str> = key.split('.').collect();
                let mut current = doc.as_item();
                let mut result_current = result_doc.as_item_mut();

                for (i, part) in key_parts.iter().enumerate() {
                    if current[*part].is_none() {
                        bail!("Config key not found: {}", key);
                    }

                    if i < key_parts.len() - 1 {
                        if !result_current[*part].is_table() {
                            result_current[*part] = Item::Table(toml_edit::Table::new());
                        }
                        result_current = &mut result_current[*part];
                        current = &current[*part];
                    } else {
                        result_current[*part] = current.clone();
                    }
                }
            }

            match self.print {
                PrintFormat::Default | PrintFormat::Toml => {
                    println!("{}", result_doc.to_string());
                }
                PrintFormat::Json => {
                    let value: serde_json::Value = toml::from_str(&result_doc.to_string())?;
                    println!("{}", serde_json::to_string_pretty(&value)?);
                }
                PrintFormat::Human => {
                    self.print_human_readable(&result_doc).await?;
                }
            }
        }

        Ok(())
    }

    async fn print_human_readable(&self, doc: &DocumentMut) -> EyreResult<()> {
        let toml_str = doc.to_string();
        let lines: Vec<&str> = toml_str.lines().collect();

        for line in lines {
            if line.trim().is_empty() {
                println!();
                continue;
            }

            if line.starts_with('[') && line.ends_with(']') {
                // Section header
                println!("{}", line.blue().bold());
            } else if let Some((key, value)) = line.split_once('=') {
                // Key-value pair
                println!("  {}{} {}", key.trim().green(), "=".dimmed(), value.trim());
            } else {
                println!("{}", line);
            }
        }

        Ok(())
    }

    async fn show_result(&self, original: &DocumentMut, modified: &DocumentMut) -> EyreResult<()> {
        match self.print {
            PrintFormat::Default | PrintFormat::Human => {
                self.show_diff(original, modified).await?;
            }
            PrintFormat::Toml => {
                println!("{}", modified.to_string());
            }
            PrintFormat::Json => {
                let value: serde_json::Value = toml::from_str(&modified.to_string())?;
                println!("{}", serde_json::to_string_pretty(&value)?);
            }
        }
        Ok(())
    }

    async fn show_diff(&self, original: &DocumentMut, modified: &DocumentMut) -> EyreResult<()> {
        let original_str = original.to_string();
        let modified_str = modified.to_string();

        let diff = TextDiff::from_lines(&original_str, &modified_str);

        for change in diff.iter_all_changes() {
            match change.tag() {
                ChangeTag::Delete => {
                    print!("{}", format!("-{}", change).red());
                }
                ChangeTag::Insert => {
                    print!("{}", format!("+{}", change).green());
                }
                ChangeTag::Equal => {
                    print!(" {}", change);
                }
            }
        }

        Ok(())
    }

    async fn validate_config(&self, doc: &DocumentMut) -> EyreResult<()> {
        let tmp_dir = temp_dir();
        let tmp_path = tmp_dir.join(CONFIG_FILE);

        write(&tmp_path, doc.to_string()).await?;

        let tmp_path_utf8 = Utf8PathBuf::try_from(tmp_dir)?;
        let config = ConfigFile::load(&tmp_path_utf8).await?;

        drop(config);

        Ok(())
    }
}

// Helper function to extract type from JSON schema
fn get_type_from_schema(schema: &serde_json::Value) -> String {
    if let Some(type_str) = schema.get("type").and_then(|t| t.as_str()) {
        return type_str.to_string();
    }

    if let Some(any_of) = schema.get("anyOf") {
        if let Some(array) = any_of.as_array() {
            let types: Vec<String> = array
                .iter()
                .filter_map(|item| item.get("type").and_then(|t| t.as_str()))
                .map(|s| s.to_string())
                .collect();
            if !types.is_empty() {
                return types.join(" | ");
            }
        }
    }

    "unknown".to_string()
}
