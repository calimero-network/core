#![allow(unused_results, reason = "Occurs in macro")]

use std::collections::{HashMap, HashSet};
use std::env::temp_dir;
use std::str::FromStr;

use calimero_config::{ConfigFile, CONFIG_FILE};
use camino::Utf8PathBuf;
use clap::{Parser, ValueEnum};
use color_eyre::owo_colors::OwoColorize;
use eyre::{bail, eyre, Result as EyreResult};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value as JsonValue};
use tokio::fs::{read_to_string, write};
use toml_edit::{Item, Value};
use tracing::info;

use crate::cli;

/// Configure the node
///
/// Examples:
///   # Print full config in default format (TOML)
///   $ merod config
///
///   # Print full config in JSON format
///   $ merod config --print json
///
///   # Print specific sections
///   $ merod config sync server.admin
///
///   # Print specific sections in JSON
///   $ merod config sync server.admin --print json
///
///   # Show hints for configuration keys
///   $ merod config discovery? discovery.relay?
///
///   # Modify configuration values (shows diff)
///   $ merod config discovery.mdns=false sync.interval_ms=50000
///
///   # Modify and save configuration
///   $ merod config discovery.mdns=false sync.interval_ms=50000 --save
#[derive(Debug, Parser)]
pub struct ConfigCommand {
    /// Key-value pairs to be added or updated in the TOML file
    #[clap(value_name = "ARGS")]
    args: Vec<KeyValueOrHint>,

    /// Output format for printing
    #[clap(long, value_enum, default_value_t = PrintFormat::Default)]
    print: PrintFormat,

    /// Save modifications to config file
    #[clap(short, long)]
    save: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ConfigSchema {
    description: Option<String>,
    #[serde(rename = "type")]
    type_info: ConfigType,
    #[serde(skip_serializing_if = "Option::is_none")]
    default: Option<SchemaValue>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum ConfigType {
    String,
    Integer,
    Float,
    Boolean,
    Object(Box<HashMap<String, ConfigSchema>>),
    Array(Box<ConfigSchema>),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
enum SchemaValue {
    String(String),
    Integer(i64),
    Float(f64),
    Boolean(bool),
    // For objects and arrays we'll just use null since we don't need their values in schema
    Null,
}

// Add these conversion functions
impl From<&Value> for SchemaValue {
    fn from(value: &Value) -> Self {
        match value {
            Value::String(s) => SchemaValue::String(s.value().to_string()),
            Value::Integer(i) => SchemaValue::Integer(*i.value()),
            Value::Float(f) => SchemaValue::Float(*f.value()),
            Value::Boolean(b) => SchemaValue::Boolean(*b.value()),
            _ => SchemaValue::Null,
        }
    }
}

impl From<SchemaValue> for Value {
    fn from(value: SchemaValue) -> Self {
        match value {
            SchemaValue::String(s) => Value::String(toml_edit::Formatted::new(s)),
            SchemaValue::Integer(i) => Value::Integer(toml_edit::Formatted::new(i)),
            SchemaValue::Float(f) => Value::Float(toml_edit::Formatted::new(f)),
            SchemaValue::Boolean(b) => Value::Boolean(toml_edit::Formatted::new(b)),
            SchemaValue::Null => Value::String(toml_edit::Formatted::new(String::new())),
        }
    }
}

#[derive(Clone, Debug)]
enum KeyValueOrHint {
    KeyValue(KeyValuePair),
    Hint(String),
}

#[derive(ValueEnum, Clone, Debug, PartialEq)]
enum PrintFormat {
    Default,
    Toml,
    Json,
    Human,
}

#[derive(Clone, Debug)]
struct KeyValuePair {
    key: String,
    value: Value,
}

impl FromStr for KeyValueOrHint {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.ends_with('?') {
            Ok(KeyValueOrHint::Hint(s.trim_end_matches('?').to_owned()))
        } else {
            let mut parts = s.splitn(2, '=');
            let key = parts.next().ok_or("Missing key")?.to_owned();

            let value = if let Some(value_part) = parts.next() {
                Value::from_str(value_part).map_err(|e| e.to_string())?
            } else {
                return Err("Missing value".to_owned());
            };

            Ok(KeyValueOrHint::KeyValue(KeyValuePair { key, value }))
        }
    }
}

impl ConfigCommand {
    pub async fn run(self, root_args: &cli::RootArgs) -> EyreResult<()> {
        let path = root_args.home.join(&root_args.node_name);

        if !ConfigFile::exists(&path) {
            bail!("Node is not initialized in {:?}", path);
        }

        let path = path.join(CONFIG_FILE);
        let toml_str = read_to_string(&path)
            .await
            .map_err(|_| eyre!("Node is not initialized in {:?}", path))?;

        let mut doc = toml_str.parse::<toml_edit::DocumentMut>()?;

        // Check for hint requests first
        let hint_keys: Vec<_> = self
            .args
            .iter()
            .filter_map(|arg| match arg {
                KeyValueOrHint::Hint(key) => Some(key),
                _ => None,
            })
            .collect();

        if !hint_keys.is_empty() {
            // Print warning if there are both hints and key-value pairs
            if self
                .args
                .iter()
                .any(|arg| matches!(arg, KeyValueOrHint::KeyValue(_)))
            {
                eprintln!("Warning: Key-value modifications are ignored when showing hints");
            }

            for key in hint_keys {
                self.print_hint(key)?;
            }
            return Ok(());
        }

        // Process key-value modifications
        let mut modified_keys = HashSet::new();
        let original_doc = doc.clone();

        for arg in &self.args {
            if let KeyValueOrHint::KeyValue(kv) = arg {
                self.process_key_value(&mut doc, &kv.key, kv.value.clone())?;
                modified_keys.insert(kv.key.clone());
            }
        }

        // Validate before proceeding
        self.validate_toml(&doc).await?;

        if modified_keys.is_empty() {
            // No modifications, just print the config
            self.print_config(&doc, &[])?;
        } else {
            // Show diff between original and modified config
            if self.print == PrintFormat::Default {
                self.show_diff(&original_doc, &doc, &modified_keys)?;
            } else {
                self.print_config(&doc, &[])?;
            }

            if self.save {
                write(&path, doc.to_string()).await?;
                info!("Node configuration has been updated");
            } else {
                eprintln!(
                    "\nnote: if this looks right, use `-s, --save` to persist these modifications"
                );
            }
        }

        Ok(())
    }

    fn process_key_value(
        &self,
        doc: &mut toml_edit::DocumentMut,
        key: &str,
        value: Value,
    ) -> EyreResult<()> {
        let key_parts: Vec<&str> = key.split('.').collect();
        if key_parts.is_empty() {
            return Err(eyre!("Empty key provided"));
        }

        let mut current = doc.as_item_mut();

        for key in &key_parts[..key_parts.len() - 1] {
            // toml_edit::Item does not have contains_key, so we must check if the key exists and is a table
            let needs_insert = match current.get_mut(*key) {
                Some(item) if item.is_table() => false,
                Some(_) | None => true,
            };
            if needs_insert {
                current[*key] = Item::Table(toml_edit::Table::new());
            }

            current = current.get_mut(*key).ok_or_else(|| {
                eyre!(
                    "Failed to access key '{}' while processing '{}'",
                    key,
                    key_parts.join(".")
                )
            })?;

            if !current.is_table() {
                return Err(eyre!(
                    "Cannot create nested key '{}' - parent '{}' is not a table",
                    key_parts.join("."),
                    key
                ));
            }
        }

        let last_key = key_parts[key_parts.len() - 1];

        // Validate type if the key already exists
        if let Some(existing) = current.get(last_key) {
            if existing.is_table() && !value.is_inline_table() {
                return Err(eyre!(
                    "Cannot set primitive value on existing table '{}'",
                    key_parts.join(".")
                ));
            }
        }

        current[last_key] = Item::Value(value);
        Ok(())
    }

    // Get the full config schema
    fn get_schema() -> HashMap<String, ConfigSchema> {
        let mut schema = HashMap::new();

        // Network configuration
        let mut network = HashMap::new();

        // Discovery config
        let mut discovery = HashMap::new();
        discovery.insert(
            "mdns".to_string(),
            ConfigSchema {
                description: Some("Enable mDNS discovery".to_string()),
                type_info: ConfigType::Boolean,
                default: Some(SchemaValue::Boolean(true)),
            },
        );
        discovery.insert(
            "advertise_address".to_string(),
            ConfigSchema {
                description: Some("Advertise observed address".to_string()),
                type_info: ConfigType::Boolean,
                default: Some(SchemaValue::Boolean(false)),
            },
        );

        network.insert(
            "discovery".to_string(),
            ConfigSchema {
                description: Some("Discovery configuration".to_string()),
                type_info: ConfigType::Object(Box::new(discovery)),
                default: None,
            },
        );

        schema.insert(
            "network".to_string(),
            ConfigSchema {
                description: Some("Network configuration".to_string()),
                type_info: ConfigType::Object(Box::new(network)),
                default: None,
            },
        );

        schema
    }

    fn print_config(&self, doc: &toml_edit::DocumentMut, keys: &[&str]) -> EyreResult<()> {
        match self.print {
            PrintFormat::Default | PrintFormat::Toml => {
                if keys.is_empty() {
                    println!("{}", doc.to_string());
                } else {
                    for key in keys {
                        if let Some(item) = doc.as_item().get(key) {
                            println!("[{}]\n{}", key, item);
                        }
                    }
                }
            }
            PrintFormat::Json => {
                let value = if keys.is_empty() {
                    from_item(doc.as_item().clone())?
                } else {
                    let mut map = Map::new();
                    for key in keys {
                        if let Some(item) = doc.as_item().get(key) {
                            map.insert(key.to_string(), from_item(item.clone())?);
                        }
                    }
                    serde_json::Value::Object(map)
                };
                println!("{}", serde_json::to_string_pretty(&value)?);
            }
            PrintFormat::Human => {
                self.print_human_format(doc, keys)?;
            }
        }
        Ok(())
    }

    fn print_human_format(&self, doc: &toml_edit::DocumentMut, keys: &[&str]) -> EyreResult<()> {
        let value = if keys.is_empty() {
            from_item(doc.as_item().clone())?
        } else {
            let mut map = Map::new();
            for key in keys {
                if let Some(item) = doc.as_item().get(key) {
                    map.insert(key.to_string(), from_item(item.clone())?);
                }
            }
            serde_json::Value::Object(map)
        };

        fn print_value(value: &serde_json::Value, indent: usize) {
            match value {
                serde_json::Value::String(s) => {
                    println!("{:indent$}\"{}\"", "", s, indent = indent)
                }
                serde_json::Value::Number(i) => println!("{:indent$}{}", "", i, indent = indent),
                serde_json::Value::Bool(b) => println!("{:indent$}{}", "", b, indent = indent),
                serde_json::Value::Array(arr) => {
                    println!("{:indent$}[", "", indent = indent);
                    for item in arr {
                        print_value(item, indent + 2);
                    }
                    println!("{:indent$}]", "", indent = indent);
                }
                serde_json::Value::Object(table) => {
                    for (k, v) in table {
                        println!("{:indent$}{}:", "", k.bold(), indent = indent);
                        print_value(v, indent + 2);
                    }
                }
                serde_json::Value::Null => println!("{:indent$}null", "", indent = indent),
            }
        }

        print_value(&value, 0);
        Ok(())
    }

    fn show_diff(
        &self,
        original: &toml_edit::DocumentMut,
        modified: &toml_edit::DocumentMut,
        modified_keys: &HashSet<String>,
    ) -> EyreResult<()> {
        for key in modified_keys {
            let key_parts: Vec<&str> = key.split('.').collect();
            let table_name = key_parts[0];

            println!("[{}]", table_name);

            let original_value = original.as_item().get(key);
            let modified_value = modified.as_item().get(key);

            if let Some(orig) = original_value {
                println!("-{} = {}", key, orig);
            } else {
                println!("-{} = (not set)", key);
            }

            if let Some(modif) = modified_value {
                println!("+{} = {}", key, modif);
            } else {
                println!("+{} = (removed)", key);
            }
        }
        Ok(())
    }

    fn print_hint(&self, key: &str) -> EyreResult<()> {
        let schema = Self::get_schema();
        let key_parts: Vec<&str> = key.split('.').collect();

        let mut current_schema = &schema;
        let mut path = Vec::new();

        for part in key_parts {
            path.push(part);
            if let ConfigType::Object(fields) = &current_schema
                .get(part)
                .ok_or_else(|| eyre!("Unknown config key: {}", path.join(".")))?
                .type_info
            {
                current_schema = &fields;
            } else {
                return Err(eyre!("Key {} is not an object", path.join(".")));
            }
        }

        match self.print {
            PrintFormat::Default | PrintFormat::Human => {
                for (field, field_schema) in current_schema {
                    let type_str = match &field_schema.type_info {
                        ConfigType::String => "string",
                        ConfigType::Integer => "integer",
                        ConfigType::Float => "float",
                        ConfigType::Boolean => "boolean",
                        ConfigType::Object(_) => "object",
                        ConfigType::Array(_) => "array",
                    };

                    println!(
                        "  .{}: {} # {}",
                        field,
                        type_str.cyan(),
                        field_schema.description.as_deref().unwrap_or("")
                    );
                }
            }
            PrintFormat::Toml => {
                println!("# Schema for {}", key);
                for (field, field_schema) in current_schema {
                    let type_str = match &field_schema.type_info {
                        ConfigType::String => "string",
                        ConfigType::Integer => "integer",
                        ConfigType::Float => "float",
                        ConfigType::Boolean => "boolean",
                        ConfigType::Object(_) => "object",
                        ConfigType::Array(_) => "array",
                    };
                    println!(
                        "#   .{}: {} # {}",
                        field,
                        type_str,
                        field_schema.description.as_deref().unwrap_or("")
                    );
                }
            }
            PrintFormat::Json => {
                let mut schema_map = Map::new();
                for (field, field_schema) in current_schema {
                    let mut field_info = Map::new();
                    field_info.insert(
                        "type".to_string(),
                        match &field_schema.type_info {
                            ConfigType::String => "string".into(),
                            ConfigType::Integer => "integer".into(),
                            ConfigType::Float => "number".into(),
                            ConfigType::Boolean => "boolean".into(),
                            ConfigType::Object(_) => "object".into(),
                            ConfigType::Array(_) => "array".into(),
                        },
                    );
                    if let Some(desc) = &field_schema.description {
                        field_info.insert("description".to_string(), desc.as_str().into());
                    }
                    schema_map.insert(field.to_string(), field_info.into());
                }
                println!("{}", serde_json::to_string_pretty(&schema_map)?);
            }
        }

        Ok(())
    }

    pub async fn validate_toml(&self, doc: &toml_edit::DocumentMut) -> EyreResult<()> {
        let tmp_dir = temp_dir();
        let tmp_path = tmp_dir.join(CONFIG_FILE);

        write(&tmp_path, doc.to_string()).await?;

        let tmp_path_utf8 = Utf8PathBuf::try_from(tmp_dir)?;

        drop(ConfigFile::load(&tmp_path_utf8).await?);

        Ok(())
    }
}

fn from_item(item: Item) -> EyreResult<JsonValue> {
    match item {
        Item::Value(value) => from_value(value),
        Item::Table(table) => {
            let mut map = Map::new();
            for (k, v) in table.iter() {
                map.insert(k.to_owned(), from_item(v.clone())?);
            }
            Ok(JsonValue::Object(map))
        }
        Item::None => Ok(JsonValue::Null),
        Item::ArrayOfTables(array) => {
            let mut vec = Vec::new();
            for table in array.iter() {
                let mut map = Map::new();
                for (k, v) in table.iter() {
                    map.insert(k.to_owned(), from_item(v.clone())?);
                }
                vec.push(JsonValue::Object(map));
            }
            Ok(JsonValue::Array(vec))
        }
    }
}

fn from_value(value: Value) -> EyreResult<JsonValue> {
    Ok(match value {
        Value::String(s) => JsonValue::String(s.value().to_string()),
        Value::Integer(i) => JsonValue::Number((*i.value()).into()),
        Value::Float(f) => {
            if let Some(n) = serde_json::Number::from_f64(*f.value()) {
                JsonValue::Number(n)
            } else {
                return Err(eyre!("Invalid float value"));
            }
        }
        Value::Boolean(b) => JsonValue::Bool(*b.value()),
        Value::Datetime(dt) => JsonValue::String(dt.to_string()),
        Value::Array(arr) => {
            let mut vec = Vec::new();
            for v in arr.iter() {
                vec.push(from_value(v.clone())?);
            }
            JsonValue::Array(vec)
        }
        Value::InlineTable(table) => {
            let mut map = Map::new();
            for (k, v) in table.iter() {
                map.insert(k.to_owned(), from_value(v.clone())?);
            }
            JsonValue::Object(map)
        }
    })
}
