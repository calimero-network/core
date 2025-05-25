#![allow(unused_results, reason = "Occurs in macro")]

use std::env::temp_dir;
use std::str::FromStr;

use calimero_config::{ConfigFile, CONFIG_FILE};
use camino::Utf8PathBuf;
use clap::{Parser, ValueEnum};
use color_eyre::owo_colors::OwoColorize;
use eyre::{bail, eyre, Result as EyreResult};
use serde_json::json;
use similar::{ChangeTag, TextDiff};
use tokio::fs::{read_to_string, write};
use toml_edit::{Item, Value};
use tracing::info;

use crate::cli;

/// Configure the node
#[derive(Debug, Parser)]
pub struct ConfigCommand {
    /// Key-value pairs to be added or updated in the config
    #[clap(value_name = "ARGS")]
    args: Vec<ConfigArg>,

    /// Output format
    #[clap(long, value_enum, default_value_t = PrintFormat::Toml)]
    print: PrintFormat,

    /// Save changes to config file
    #[clap(short, long)]
    save: bool,
}

#[derive(Clone, Debug, ValueEnum)]
enum PrintFormat {
    Toml,
    Json,
    Human,
}

#[derive(Clone, Debug)]
enum ConfigArg {
    KeyValue { key: String, value: Value },
    Hint { key: String },
}

impl FromStr for ConfigArg {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.ends_with('?') {
            Ok(ConfigArg::Hint {
                key: s.trim_end_matches('?').to_owned(),
            })
        } else {
            let mut parts = s.splitn(2, '=');
            let key = parts.next().ok_or("Missing key")?.to_owned();
            let value = parts.next().ok_or("Missing value")?;
            let value = Value::from_str(value).map_err(|e| e.to_string())?;

            Ok(ConfigArg::KeyValue { key, value })
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

        let original_doc = toml_str.parse::<toml_edit::DocumentMut>()?;
        let mut modified_doc = original_doc.clone();

        let mut has_hints = false;
        for arg in &self.args {
            if let ConfigArg::Hint { key } = arg {
                has_hints = true;
                self.print_hint(&key, &original_doc)?;
            }
        }

        if has_hints {
            if self
                .args
                .iter()
                .any(|arg| matches!(arg, ConfigArg::KeyValue { .. }))
            {
                eprintln!("{}", "warning: edits ignored when showing hints".yellow());
            }
            return Ok(());
        }

        let mut has_edits = false;
        for arg in &self.args {
            if let ConfigArg::KeyValue { key, value } = arg {
                has_edits = true;
                self.apply_edit(&mut modified_doc, key, value)?;
            }
        }

        if has_edits {
            self.validate_toml(&modified_doc).await?;
            self.show_diff(&original_doc, &modified_doc)?;

            if self.save {
                write(&path, modified_doc.to_string()).await?;
                info!("Node configuration has been updated");
            } else {
                eprintln!(
                    "{}",
                    "note: if this looks right, use `-s, --save` to persist these modifications"
                        .yellow()
                );
            }
        } else {
            self.print_config(&original_doc)?;
        }

        Ok(())
    }

    fn apply_edit(
        &self,
        doc: &mut toml_edit::DocumentMut,
        key: &str,
        value: &Value,
    ) -> EyreResult<()> {
        let key_parts: Vec<&str> = key.split('.').collect();
        let mut current = doc.as_item_mut();

        for key in &key_parts[..key_parts.len() - 1] {
            current = &mut current[key];
        }

        current[key_parts[key_parts.len() - 1]] = Item::Value(value.clone());
        Ok(())
    }

    fn print_hint(&self, key: &str, doc: &toml_edit::DocumentMut) -> EyreResult<()> {
        let key_parts: Vec<&str> = key.split('.').collect();
        let mut current = doc.as_item();

        for key in &key_parts {
            current = &current[key];
        }

        if let Some(table) = current.as_table() {
            match self.print {
                PrintFormat::Human => {
                    println!("{}: object", key);
                    for (k, v) in table.iter() {
                        println!("  .{}: {}", k, self.type_hint(v));
                    }
                }
                PrintFormat::Json => {
                    let schema = table
                        .iter()
                        .map(|(k, v)| {
                            json!({
                                "key": k,
                                "type": self.type_hint(v),
                                "description": "" // TODO: Add descriptions from schema
                            })
                        })
                        .collect::<Vec<_>>();
                    println!("{}", serde_json::to_string_pretty(&schema)?);
                }
                PrintFormat::Toml => {
                    let mut output = String::new();
                    output.push_str(&format!("# {}\n", key));
                    for (k, v) in table.iter() {
                        output.push_str(&format!("# .{}: {}\n", k, self.type_hint(v)));
                    }
                    println!("{}", output);
                }
            }
        } else if let Some(value) = current.as_value() {
            let item = Item::Value(value.clone());
            match self.print {
                PrintFormat::Human => println!("{}: {}", key, self.type_hint(&item)),
                PrintFormat::Json => println!(
                    "{}",
                    serde_json::to_string_pretty(&json!({
                        "key": key,
                        "type": self.type_hint(&item),
                        "description": ""
                    }))?
                ),
                PrintFormat::Toml => println!("# {}: {}", key, self.type_hint(&item)),
            }
        }

        Ok(())
    }

    fn type_hint(&self, item: &Item) -> String {
        match item {
            Item::None => "null".to_string(),
            Item::Value(Value::String(_)) => "string".to_string(),
            Item::Value(Value::Integer(_)) => "integer".to_string(),
            Item::Value(Value::Float(_)) => "float".to_string(),
            Item::Value(Value::Boolean(_)) => "boolean".to_string(),
            Item::Value(Value::Datetime(_)) => "datetime".to_string(),
            Item::Value(Value::Array(arr)) => {
                if let Some(first) = arr.get(0) {
                    format!("array[{}]", self.type_hint(&Item::Value(first.clone())))
                } else {
                    "array[]".to_string()
                }
            }
            Item::Value(Value::InlineTable(_)) => "object".to_string(),
            Item::Table(_) => "object".to_string(),
            Item::ArrayOfTables(_) => "array[object]".to_string(),
        }
    }
    fn show_diff(
        &self,
        original: &toml_edit::DocumentMut,
        modified: &toml_edit::DocumentMut,
    ) -> EyreResult<()> {
        match self.print {
            PrintFormat::Human => {
                let original_str = original.to_string();
                let modified_str = modified.to_string();
                let diff = TextDiff::from_lines(&original_str, &modified_str);

                for change in diff.iter_all_changes() {
                    match change.tag() {
                        ChangeTag::Delete => print!("-{}", change.red()),
                        ChangeTag::Insert => print!("+{}", change.green()),
                        ChangeTag::Equal => print!(" {}", change),
                    }
                }
            }
            PrintFormat::Json | PrintFormat::Toml => {
                self.print_config(modified)?;
            }
        }

        Ok(())
    }

    fn print_config(&self, doc: &toml_edit::DocumentMut) -> EyreResult<()> {
        match self.print {
            PrintFormat::Toml => println!("{}", doc.to_string()),
            PrintFormat::Json => {
                let value: serde_json::Value = toml::from_str(&doc.to_string())?;
                println!("{}", serde_json::to_string_pretty(&value)?);
            }
            PrintFormat::Human => {
                println!("{}", "Node Configuration:".bold().underline());
                println!();

                for (section, item) in doc.iter() {
                    match item {
                        Item::Table(table) => {
                            println!("{}", section.bold().blue());
                            self.print_table(table, 1)?;
                            println!();
                        }
                        Item::ArrayOfTables(array) => {
                            println!("{}", section.bold().blue());
                            for table in array.iter() {
                                self.print_table(table, 1)?;
                                println!("---");
                            }
                            println!();
                        }
                        Item::Value(value) => {
                            println!("{} = {}", section.bold(), self.format_value(value));
                        }
                        _ => {}
                    }
                }
            }
        }
        Ok(())
    }

    fn print_table(&self, table: &toml_edit::Table, indent: usize) -> EyreResult<()> {
        let indent_str = " ".repeat(indent * 2);

        for (key, item) in table.iter() {
            match item {
                Item::Value(value) => {
                    println!("{}{} = {}", indent_str, key, self.format_value(value));
                }
                Item::Table(nested_table) => {
                    println!("{}{}:", indent_str, key);
                    self.print_table(nested_table, indent + 1)?;
                }
                Item::ArrayOfTables(nested_array) => {
                    println!("{}{}:", indent_str, key);
                    for nested_table in nested_array.iter() {
                        self.print_table(nested_table, indent + 1)?;
                        println!("{}---", indent_str);
                    }
                }
                _ => {}
            }
        }

        Ok(())
    }

    fn format_value(&self, value: &Value) -> String {
        match value {
            Value::String(s) => format!("\"{}\"", s.green()),
            Value::Integer(i) => i.to_string().cyan().to_string(),
            Value::Float(f) => f.to_string().cyan().to_string(),
            Value::Boolean(b) => b.to_string().yellow().to_string(),
            Value::Datetime(dt) => dt.to_string().magenta().to_string(),
            Value::Array(arr) => {
                let items: Vec<String> = arr.iter().map(|v| self.format_value(v)).collect();
                format!("[{}]", items.join(", "))
            }
            Value::InlineTable(table) => {
                let items: Vec<String> = table
                    .iter()
                    .map(|(k, v)| format!("{} = {}", k, self.format_value(v)))
                    .collect();
                format!("{{ {} }}", items.join(", "))
            }
        }
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
