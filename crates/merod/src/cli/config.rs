#![allow(unused_results, reason = "Occurs in macro")]

use std::io::Write;
use std::str::FromStr;

use calimero_config::{ConfigFile, CONFIG_FILE};
use camino::Utf8PathBuf;
use clap::{Parser, ValueEnum};
use comfy_table::{Cell, Color as ComfyColor, Table};
use eyre::{bail, eyre, Result as EyreResult};
use lazy_static::lazy_static;
use similar::{ChangeTag, TextDiff};
use termcolor::{Color as TermColor, ColorChoice, ColorSpec, StandardStream, WriteColor};
use tokio::fs::{read_to_string, write};
use toml_edit::{Item, Value};
use tracing::info;

use crate::cli;

/// Configure the node
#[derive(Debug, Parser, Clone)]
pub struct ConfigCommand {
    /// Key-value pairs to be added or updated in the TOML file
    #[clap(value_name = "ARGS")]
    args: Vec<KeyValuePair>,

    /// Output format for printing config
    #[clap(long, value_name = "FORMAT", default_value = "default")]
    print: PrintFormat,

    /// Save modifications to config file
    #[clap(short, long)]
    save: bool,
}

#[derive(Clone, Debug, ValueEnum)]
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
        let toml_str = read_to_string(&path)
            .await
            .map_err(|_| eyre!("Node is not initialized in {:?}", path))?;
        let mut doc = toml_str.parse::<toml_edit::DocumentMut>()?;

        let (hints, edits): (Vec<_>, Vec<_>) = self
            .args
            .clone()
            .into_iter()
            .partition(|kv| kv.key.ends_with('?'));

        if !hints.is_empty() {
            return self.print_hints(&hints);
        }

        let mut changes_made = false;

        if !edits.is_empty() {
            for kv in &edits {
                let key_parts: Vec<&str> = kv.key.split('.').collect();
                let mut current = doc.as_item_mut();

                for key in &key_parts[..key_parts.len() - 1] {
                    current = &mut current[key];
                }

                let last_key = key_parts[key_parts.len() - 1];
                let old_value = current[last_key].clone();
                current[last_key] = Item::Value(kv.value.clone());

                if old_value.to_string() != current[last_key].to_string() {
                    changes_made = true;
                }
            }
        }

        self.clone().validate_toml(&doc).await?;

        if changes_made {
            if self.save {
                write(&path, doc.to_string()).await?;
                info!("Node configuration has been updated");
            } else {
                self.print_diff(&toml_str, &doc.to_string())?;
                eprintln!(
                    "\nnote: if this looks right, use `-s, --save` to persist these modifications"
                );
            }
        } else if edits.is_empty() {
            self.print_config(&doc)?;
        } else {
            eprintln!("warning: no changes were made to the configuration");
        }

        Ok(())
    }

    pub async fn validate_toml(&self, doc: &toml_edit::DocumentMut) -> EyreResult<()> {
        let temp_file = tempfile::NamedTempFile::new()?;
        let temp_path = temp_file.path().to_owned();

        write(&temp_path, doc.to_string()).await?;

        let temp_path_utf8 = Utf8PathBuf::try_from(temp_path)
            .map_err(|_| eyre!("Failed to convert temp path to UTF-8"))?;

        ConfigFile::load(&temp_path_utf8)
            .await
            .map_err(|e| eyre!("Config validation failed: {}", e))?;

        temp_file.close()?;

        Ok(())
    }

    fn print_hints(&self, hints: &[KeyValuePair]) -> EyreResult<()> {
        let mut table = Table::new();
        table.load_preset("││──├─┤─┼─└ ┴┬┌ ┐");
        table.set_header(vec![
            Cell::new("Key").fg(ComfyColor::Blue),
            Cell::new("Type").fg(ComfyColor::Yellow),
            Cell::new("Description").fg(ComfyColor::Green),
        ]);

        for kv in hints {
            let key = kv.key.trim_end_matches('?');
            if let Some(schema) = CONFIG_SCHEMA.find(key) {
                table.add_row(vec![
                    Cell::new(key),
                    Cell::new(schema.type_info),
                    Cell::new(schema.description),
                ]);

                for child in &schema.children {
                    let child_key = format!("{}.{}", key, child.path);
                    table.add_row(vec![
                        Cell::new(child_key),
                        Cell::new(child.type_info),
                        Cell::new(child.description),
                    ]);
                }
            } else {
                eprintln!("warning: no schema found for key '{}'", key);
            }
        }

        println!("{}", table);
        Ok(())
    }

    fn print_diff(&self, old: &str, new: &str) -> EyreResult<()> {
        let diff = TextDiff::from_lines(old, new);
        let mut stdout = StandardStream::stdout(ColorChoice::Auto);

        for op in diff.ops() {
            for change in diff.iter_changes(op) {
                let (sign, color) = match change.tag() {
                    ChangeTag::Delete => ("-", TermColor::Red),
                    ChangeTag::Insert => ("+", TermColor::Green),
                    ChangeTag::Equal => (" ", TermColor::White),
                };

                stdout.set_color(ColorSpec::new().set_fg(Some(color)).set_intense(true))?;
                write!(&mut stdout, "{}{}", sign, change)?;
            }
        }

        stdout.reset()?;
        Ok(())
    }

    fn print_config(&self, doc: &toml_edit::DocumentMut) -> EyreResult<()> {
        match self.print {
            PrintFormat::Default | PrintFormat::Toml => {
                println!("{}", doc.to_string());
            }
            PrintFormat::Json => {
                let value: serde_json::Value = toml::from_str(&doc.to_string())?;
                println!("{}", serde_json::to_string_pretty(&value)?);
            }
            PrintFormat::Human => {
                self.print_human(doc)?;
            }
        }
        Ok(())
    }

    fn print_human(&self, doc: &toml_edit::DocumentMut) -> EyreResult<()> {
        let mut table = Table::new();

        table.load_preset("││──├─┤─┼─└ ┴┬┌ ┐");
        table.set_header(vec![
            Cell::new("Key").fg(ComfyColor::Blue),
            Cell::new("Value").fg(ComfyColor::Green),
            Cell::new("Type").fg(ComfyColor::Yellow),
        ]);

        fn add_to_table(table: &mut Table, prefix: &str, item: &Item, schema: &ConfigSchema) {
            match item {
                Item::None => (),
                Item::Value(value) => {
                    table.add_row(vec![
                        Cell::new(prefix),
                        Cell::new(value.to_string()),
                        Cell::new(schema.type_info),
                    ]);
                }
                Item::Table(table_data) => {
                    for (key, value) in table_data.iter() {
                        let full_path = if prefix.is_empty() {
                            key.to_owned()
                        } else {
                            format!("{}.{}", prefix, key)
                        };

                        if let Some(child_schema) = schema.find(&full_path) {
                            if value.is_table() {
                                add_to_table(table, &full_path, value, child_schema);
                            } else {
                                table.add_row(vec![
                                    Cell::new(full_path),
                                    Cell::new(value.as_str().unwrap_or("")),
                                    Cell::new(child_schema.type_info),
                                ]);
                            }
                        }
                    }
                }
                Item::ArrayOfTables(_) => {
                    // Handle arrays if needed
                }
            }
        }

        // Print top-level sections first
        for (key, value) in doc.iter() {
            if let Some(schema) = CONFIG_SCHEMA.find(key) {
                add_to_table(&mut table, key, value, schema);
            }
        }

        println!("{}", table);
        Ok(())
    }
}

#[derive(Debug, Clone)]
struct ConfigSchema {
    path: &'static str,
    type_info: &'static str,
    description: &'static str,
    children: Vec<ConfigSchema>,
}

impl ConfigSchema {
    fn find(&self, path: &str) -> Option<&ConfigSchema> {
        let mut parts = path.split('.');
        let first = parts.next()?;

        if first != self.path {
            return None;
        }

        let mut current = self;
        for part in parts {
            current = current.children.iter().find(|c| c.path == part)?;
        }
        Some(current)
    }
}

lazy_static! {
    static ref CONFIG_SCHEMA: ConfigSchema = ConfigSchema {
        path: "root",
        type_info: "object",
        description: "Root configuration",
        children: vec![
            ConfigSchema {
                path: "sync",
                type_info: "object",
                description: "Sync configuration",
                children: vec![
                    ConfigSchema {
                        path: "timeout_ms",
                        type_info: "u64",
                        description: "Timeout for sync operations in milliseconds",
                        children: vec![],
                    },
                    ConfigSchema {
                        path: "interval_ms",
                        type_info: "u64",
                        description: "Interval between sync operations in milliseconds",
                        children: vec![],
                    },
                ],
            },
            ConfigSchema {
                path: "discovery",
                type_info: "object",
                description: "Discovery configuration",
                children: vec![
                    ConfigSchema {
                        path: "mdns",
                        type_info: "boolean",
                        description: "Enable mDNS discovery",
                        children: vec![],
                    },
                    ConfigSchema {
                        path: "relay",
                        type_info: "object",
                        description: "Relay configuration",
                        children: vec![ConfigSchema {
                            path: "registrations_limit",
                            type_info: "usize",
                            description: "Max number of active relay registrations",
                            children: vec![],
                        },],
                    },
                ],
            },
        ],
    };
}
