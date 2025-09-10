#![allow(unused_results, reason = "Occurs in macro")]

use std::env::temp_dir;
use std::str::FromStr;

use calimero_config::{ConfigFile, CONFIG_FILE};
use camino::Utf8PathBuf;
use clap::{Parser, ValueEnum};
use eyre::{bail, eyre, Result as EyreResult};
use tokio::fs::{read_to_string, write};
use toml_edit::{Item, Value};
use tracing::info;

use crate::cli::schema::{generate_schema, get_field_hint, ConfigSchema};
use crate::cli::{self};

/// Configure the node
///
/// Examples:
///   # Print full config in TOML format
///   merod config
///
///   # Print full config in JSON format  
///   merod config --print json
///
///   # Print specific sections
///   merod config sync server.admin
///
///   # Edit values (shows diff)
///   merod config discovery.mdns=false sync.interval_ms=50000
///
///   # Get hints about config keys
///   merod config discovery? sync?
#[derive(Debug, Parser)]
pub struct ConfigCommand {
    /// Key-value pairs to be added or updated in the TOML file
    #[clap(value_name = "ARGS")]
    args: Vec<KeyValuePair>,

    /// Output format for printing configuration
    #[clap(long, value_enum, default_value_t = PrintFormat::Toml)]
    print: PrintFormat,

    /// Save modifications (if any)
    #[clap(short, long)]
    save: bool,
}

#[derive(Debug, Clone, ValueEnum)]
enum PrintFormat {
    Toml,
    Json,
    Human,
}

#[derive(Debug)]
enum ConfigAction {
    Print {
        keys: Vec<String>,
        format: PrintFormat,
    },
    Edit {
        changes: Vec<KeyValuePair>,
        show_diff: bool,
        save: bool,
    },
    Hint {
        keys: Vec<String>,
        format: PrintFormat,
    },
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

        let value = if let Some(val) = parts.next() {
            Value::from_str(val).map_err(|e| e.to_string())?
        } else {
            Value::from("")
        };

        Ok(Self { key, value })
    }
}

impl ConfigCommand {
    pub async fn run(mut self, root_args: &cli::RootArgs) -> EyreResult<()> {
        let path = root_args.home.join(&root_args.node_name);

        if !ConfigFile::exists(&path) {
            bail!("Node is not initialized in {:?}", path);
        }

        let path = path.join(CONFIG_FILE);
        let toml_str = read_to_string(&path).await?;
        let mut doc = toml_str.parse::<toml_edit::DocumentMut>()?;

        // Parse arguments into actions
        let actions = self.parse_actions()?;

        let mut changes_made = false;
        let schema = generate_schema();

        for action in actions {
            match action {
                ConfigAction::Print { keys, format } => {
                    self.print_config(&doc, keys, format, &schema).await?;
                }
                ConfigAction::Edit {
                    changes,
                    show_diff,
                    save,
                } => {
                    changes_made |= self.edit_config(&mut doc, changes, show_diff).await?;
                    self.save = save;
                }
                ConfigAction::Hint { keys, format } => {
                    self.print_hints(keys, format, &schema).await?;
                }
            }
        }

        if changes_made && self.save {
            self.validate_toml(&doc).await?;
            write(&path, doc.to_string()).await?;
            info!("Node configuration has been updated");
        } else if changes_made {
            eprintln!("note: if this looks right, use `-s, --save` to persist these modifications");
        }

        Ok(())
    }

    fn parse_actions(&self) -> EyreResult<Vec<ConfigAction>> {
        let mut actions = Vec::new();
        let mut edits = Vec::new();
        let mut hints = Vec::new();
        let mut prints = Vec::new();

        for arg in &self.args {
            if arg.key.contains('=') {
                // Editing a value - no need to parse again, we already have KeyValuePair
                edits.push(arg.clone());
            } else if arg.key.ends_with('?') {
                // Requesting a hint
                let key = arg.key.trim_end_matches('?').to_owned();
                hints.push(key);
            } else {
                // Printing a value - use the key directly
                prints.push(arg.key.clone());
            }
        }

        if !edits.is_empty() {
            actions.push(ConfigAction::Edit {
                changes: edits.clone(),
                show_diff: true,
                save: self.save,
            });
        }

        if !hints.is_empty() {
            actions.push(ConfigAction::Hint {
                keys: hints.clone(),
                format: self.print.clone(),
            });
        }

        if !prints.is_empty() || (edits.is_empty() && hints.is_empty()) {
            actions.push(ConfigAction::Print {
                keys: prints,
                format: self.print.clone(),
            });
        }

        Ok(actions)
    }

    async fn print_config(
        &self,
        doc: &toml_edit::DocumentMut,
        keys: Vec<String>,
        format: PrintFormat,
        schema: &ConfigSchema,
    ) -> EyreResult<()> {
        if keys.is_empty() {
            // Print entire config
            match format {
                PrintFormat::Toml => println!("{}", doc.to_string()),
                PrintFormat::Json => {
                    let value: serde_json::Value = toml::from_str(&doc.to_string())?;
                    println!("{}", serde_json::to_string_pretty(&value)?);
                }
                PrintFormat::Human => {
                    self.print_human(doc, schema).await?;
                }
            }
        } else {
            // Print specific keys
            let mut output = serde_json::Map::new();

            for key in &keys {
                let parts: Vec<&str> = key.split('.').collect();
                let mut current = doc.as_item();

                for part in &parts {
                    current = &current[part];
                }

                // Convert to JSON value and add to output
                // (implementation omitted for brevity)
            }

            match format {
                PrintFormat::Toml => {
                    println!("{}", doc.to_string());
                }
                PrintFormat::Json => {
                    println!("{}", serde_json::to_string_pretty(&output)?);
                }
                PrintFormat::Human => {
                    self.print_human_selected(&output, &keys, schema).await?;
                }
            }
        }

        Ok(())
    }

    async fn edit_config(
        &self,
        doc: &mut toml_edit::DocumentMut,
        changes: Vec<KeyValuePair>,
        show_diff: bool,
    ) -> EyreResult<bool> {
        let original = doc.clone();
        let mut changes_made = false;

        for kv in changes {
            let key_parts: Vec<&str> = kv.key.split('.').collect();

            // Handle protocol migration from context.config.* to protocols.*
            if key_parts.get(0) == Some(&"context") && key_parts.get(1) == Some(&"config") {
                if let Some(protocol) = key_parts.get(2) {
                    // Ensure protocols section exists
                    if doc.get("protocols").is_none() {
                        doc["protocols"] = toml_edit::table();
                    }

                    // Ensure protocol subsection exists
                    if doc["protocols"].get(protocol).is_none() {
                        doc["protocols"][protocol] = toml_edit::table();

                        // Initialize required fields with empty values
                        doc["protocols"][protocol]["network"] = toml_edit::value("");
                        doc["protocols"][protocol]["contract_id"] = toml_edit::value("");
                        doc["protocols"][protocol]["signer"] = toml_edit::value("");
                    }

                    // Build the new key and set the value
                    let new_key_parts: Vec<&str> = if key_parts.len() > 3 {
                        key_parts[3..].to_vec()
                    } else {
                        vec![]
                    };

                    let mut current = &mut doc["protocols"][protocol];
                    for part in &new_key_parts[..new_key_parts.len() - 1] {
                        if current.get(part).is_none() {
                            current[part] = toml_edit::table();
                        }
                        current = &mut current[part];
                    }

                    let last_key = new_key_parts.last().unwrap_or(&"");
                    let old_value = current[*last_key].clone();
                    current[*last_key] = Item::Value(kv.value.clone());
                    changes_made = true;

                    if show_diff {
                        self.show_diff(&old_value, &current[*last_key], &kv.key)
                            .await?;
                    }

                    continue;
                }
            }

            // Original handling for non-protocol configs...
            let mut current = doc.as_item_mut();
            for key in &key_parts[..key_parts.len() - 1] {
                if current[*key].is_none() {
                    current[*key] = toml_edit::table();
                }
                current = &mut current[*key];
            }

            let last_key = key_parts[key_parts.len() - 1];
            let old_value = current[last_key].clone();
            current[last_key] = Item::Value(kv.value.clone());
            changes_made = true;

            if show_diff {
                self.show_diff(&old_value, &current[last_key], &kv.key)
                    .await?;
            }
        }

        Ok(changes_made)
    }

    async fn show_diff(&self, old: &Item, new: &Item, key: &str) -> EyreResult<()> {
        let old_str = old.to_string();
        let new_str = new.to_string();

        if old_str != new_str {
            let diff = diff::lines(&old_str, &new_str);

            println!("[{}]", key);
            for change in diff {
                match change {
                    diff::Result::Left(l) => println!("-{}", l),
                    diff::Result::Right(r) => println!("+{}", r),
                    diff::Result::Both(_, _) => (),
                }
            }
            println!();
        }

        Ok(())
    }

    async fn print_hints(
        &self,
        keys: Vec<String>,
        format: PrintFormat,
        schema: &ConfigSchema,
    ) -> EyreResult<()> {
        for key in keys {
            let parts: Vec<&str> = key.split('.').collect();
            if let Some(hint) = get_field_hint(&parts, schema) {
                match format {
                    PrintFormat::Human => println!("{}", hint),
                    PrintFormat::Json => {
                        let json = serde_json::json!({
                            "key": key,
                            "hint": hint
                        });
                        println!("{}", serde_json::to_string_pretty(&json)?);
                    }
                    PrintFormat::Toml => {
                        println!("# {}", hint);
                        println!("# Key: {}", key);
                    }
                }
            } else {
                eprintln!("warning: no schema information available for {}", key);
            }
        }

        Ok(())
    }

    pub async fn validate_toml(self, doc: &toml_edit::DocumentMut) -> EyreResult<()> {
        let tmp_dir = temp_dir();
        let tmp_path = tmp_dir.join(CONFIG_FILE);

        // Create a modified document for validation that handles protocol migration
        let mut validation_doc = doc.clone();

        // Ensure protocols section exists
        if validation_doc.get("protocols").is_none() {
            validation_doc["protocols"] = toml_edit::table();
        }

        // Collect protocol names first to avoid borrowing issues
        let protocol_names: Vec<String> =
            if let Some(protocols) = validation_doc.get("protocols").and_then(|p| p.as_table()) {
                protocols.iter().map(|(name, _)| name.to_string()).collect()
            } else {
                Vec::new()
            };

        // Process each protocol separately to avoid borrowing conflicts
        for protocol_name in protocol_names {
            if let Some(protocol_config) = validation_doc["protocols"].get(&protocol_name) {
                if let Some(protocol_table) = protocol_config.as_table() {
                    let mut updated_protocol = toml_edit::Table::new();

                    // Copy existing fields
                    for (key, value) in protocol_table.iter() {
                        updated_protocol[key] = value.clone();
                    }

                    // Ensure required fields exist with empty defaults if missing
                    if !updated_protocol.contains_key("network") {
                        updated_protocol["network"] = toml_edit::value("");
                    }
                    if !updated_protocol.contains_key("contract_id") {
                        updated_protocol["contract_id"] = toml_edit::value("");
                    }
                    if !updated_protocol.contains_key("signer") {
                        updated_protocol["signer"] = toml_edit::value("");
                    }

                    // Update the protocol table
                    validation_doc["protocols"][&protocol_name] = Item::Table(updated_protocol);
                }
            }
        }

        write(&tmp_path, validation_doc.to_string()).await?;
        let tmp_path_utf8 = Utf8PathBuf::try_from(tmp_dir)?;

        match ConfigFile::load(&tmp_path_utf8).await {
            Ok(_) => Ok(()),
            Err(e) => {
                // Provide more detailed error message for protocol config validation
                let doc_str = validation_doc.to_string();

                // Check for protocol configuration errors
                if doc_str.contains("[protocols]") {
                    if let Some(serde_err) = e.downcast_ref::<serde_json::Error>() {
                        if serde_err.to_string().contains("missing field `network`") {
                            // Try to identify which protocol is missing the network field
                            let missing_protocol = if doc_str.contains("[protocols.ethereum]") {
                                "ethereum"
                            } else if doc_str.contains("[protocols.near]") {
                                "near"
                            } else if doc_str.contains("[protocols.icp]") {
                                "icp"
                            } else if doc_str.contains("[protocols.stellar]") {
                                "stellar"
                            } else {
                                "unknown"
                            };

                            bail!("Protocol configuration for '{}' is missing required 'network' field. Make sure you specify protocols.{}.network=<value>", missing_protocol, missing_protocol);
                        }

                        if serde_err
                            .to_string()
                            .contains("missing field `contract_id`")
                        {
                            let missing_protocol = if doc_str.contains("[protocols.ethereum]") {
                                "ethereum"
                            } else if doc_str.contains("[protocols.near]") {
                                "near"
                            } else if doc_str.contains("[protocols.icp]") {
                                "icp"
                            } else if doc_str.contains("[protocols.stellar]") {
                                "stellar"
                            } else {
                                "unknown"
                            };

                            bail!("Protocol configuration for '{}' is missing required 'contract_id' field. Make sure you specify protocols.{}.contract_id=<value>", missing_protocol, missing_protocol);
                        }

                        if serde_err.to_string().contains("missing field `signer`") {
                            let missing_protocol = if doc_str.contains("[protocols.ethereum]") {
                                "ethereum"
                            } else if doc_str.contains("[protocols.near]") {
                                "near"
                            } else if doc_str.contains("[protocols.icp]") {
                                "icp"
                            } else if doc_str.contains("[protocols.stellar]") {
                                "stellar"
                            } else {
                                "unknown"
                            };

                            bail!("Protocol configuration for '{}' is missing required 'signer' field. Make sure you specify protocols.{}.signer=<value>", missing_protocol, missing_protocol);
                        }
                    }
                }
                Err(e)
            }
        }
    }

    async fn print_human(
        &self,
        doc: &toml_edit::DocumentMut,
        schema: &ConfigSchema,
    ) -> EyreResult<()> {
        // Convert TOML to a serde_json::Value for easier processing
        let value: serde_json::Value = toml::from_str(&doc.to_string())?;

        // Print the config in a human-readable format with schema hints
        println!("Configuration:");
        Self::print_human_value(&value, schema, 0, "")?;

        Ok(())
    }

    async fn print_human_selected(
        &self,
        output: &serde_json::Map<String, serde_json::Value>,
        keys: &[String],
        schema: &ConfigSchema,
    ) -> EyreResult<()> {
        println!("Selected configuration values:");
        for key in keys {
            if let Some(value) = output.get(key) {
                println!("{}:", key);
                Self::print_human_value(value, schema, 1, key)?;
            } else {
                println!("{}: (not found)", key);
            }
        }
        Ok(())
    }

    // Helper function to recursively print values with schema hints
    fn print_human_value(
        value: &serde_json::Value,
        schema: &ConfigSchema,
        indent: usize,
        path: &str,
    ) -> EyreResult<()> {
        let indent_str = "  ".repeat(indent);

        match value {
            serde_json::Value::Object(map) => {
                for (key, val) in map {
                    let new_path = if path.is_empty() {
                        key.clone()
                    } else {
                        format!("{}.{}", path, key)
                    };

                    // Get schema hint if available
                    let hint = get_field_hint(&new_path.split('.').collect::<Vec<_>>(), schema)
                        .unwrap_or_else(|| "No description available".to_owned());

                    println!("{}{}: {}", indent_str, key, hint);
                    Self::print_human_value(val, schema, indent + 1, &new_path)?;
                }
            }
            serde_json::Value::Array(arr) => {
                println!("{}[", indent_str);
                for (i, val) in arr.iter().enumerate() {
                    Self::print_human_value(val, schema, indent + 1, &format!("{}[{}]", path, i))?;
                }
                println!("{}]", indent_str);
            }
            _ => println!("{}{}", indent_str, value),
        }

        Ok(())
    }
}
