use clap::{Parser, ValueEnum};
use color_eyre::owo_colors::OwoColorize;
use std::collections::BTreeMap;
use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use toml_edit::{Document, Item, Table, Value};

use calimero_config::config_file::ConfigFile;
use calimero_config::format::{print_config, PrintFormat};
use calimero_config::schema::{get_schema_hint, HintFormat};

#[derive(Debug, Clone, ValueEnum)]
enum OutputFormat {
    #[clap(alias = "default")]
    Default,
    Toml,
    Json,
}

impl From<OutputFormat> for HintFormat {
    fn from(fmt: OutputFormat) -> Self {
        match fmt {
            OutputFormat::Default => HintFormat::Human,
            OutputFormat::Toml => HintFormat::Toml,
            OutputFormat::Json => HintFormat::Json,
        }
    }
}

#[derive(Parser, Debug)]
#[command(
    name = "config",
    about = "Inspect or modify the merod configuration file",
    long_about = "Inspect, edit, or save merod configuration values.\n\n
To view the current configuration: merod config\n
To view a specific section: merod config sync\n
To print in JSON: merod config --print json\n
To edit values: merod config key=value\n
To view schema hints: merod config key?\n\n
Edits are only saved if you use the --save flag. Schema hints
show what keys and value types are allowed.",
    after_help = "EXAMPLES:\n
\n
View full config (in TOML):\n
merod config\n
\n
View full config in JSON:\n
merod config --print json\n
\n
View part of the config:\n
merod config sync server.admin\n
\n
Edit values in memory:\n
merod config discovery.mdns=false sync.interval_ms=50000\n
\n
Save edits to file:\n
merod config discovery.mdns=false -s\n
\n
Show diff before saving:\n
merod config discovery.mdns=false --print default\n
\n
Show config schema hint:\n
merod config discovery?\n
merod config discovery.relay? --print json"
)]
pub struct ConfigCmd {
    #[arg(value_name = "ARGS")]
    args: Vec<String>,

    /// Print the config (full or partial) in given format
    #[arg(long = "print", value_enum, default_value = "default")]
    print: OutputFormat,

    /// Save modifications to the config file
    #[arg(short, long)]
    save: bool,
}

impl ConfigCmd {
    pub fn run(&self, config_path: PathBuf) -> anyhow::Result<()> {
        // Load or create default config from path
        let mut config = ConfigFile::load_or_default(&config_path)?;

        // Separate CLI arguments into edits, hints, and plain keys
        let mut edits: BTreeMap<String, String> = BTreeMap::new();
        let mut hints: Vec<String> = Vec::new();
        let mut keys_to_print: Vec<String> = Vec::new();

        for arg in &self.args {
            if arg.contains('=') {
                // key=value → edit
                let parts: Vec<_> = arg.splitn(2, '=').collect();
                edits.insert(parts[0].to_string(), parts[1].to_string());
            } else if arg.ends_with('?') {
                // key? → schema hint
                hints.push(arg.trim_end_matches('?').to_string());
            } else {
                // plain key → partial print
                keys_to_print.push(arg.to_string());
            }
        }

        // 1. Handle hints first (exclusive mode)
        if !hints.is_empty() {
            // Do not allow edits with hints per requirements
            if !edits.is_empty() || !keys_to_print.is_empty() {
                eprintln!("{}", "Warning: schema hints ignore edits and partial print keys.".yellow());
            }

            let hint_fmt: HintFormat = self.print.clone().into();

            for key in &hints {
                // Safe retrieval of schema hints; include error messages
                match get_schema_hint(key, hint_fmt) {
                    Ok(rendered) => println!("{rendered}"),
                    Err(e) => eprintln!("{} {}: {}", "Error getting schema hint for".red(), key, e),
                }
            }
            return Ok(());
        }

        // 2. Handle edits (in-memory)
        if !edits.is_empty() {
            // Apply edits with validation; returns diff and updated config doc
            let (diff, updated_doc) = config.apply_edits(&edits)?;

            if diff.is_empty() {
                eprintln!("{}", "No changes made; skipping save.".yellow());
            } else {
                match self.print {
                    OutputFormat::Default => {
                        // Show diff in human-readable colored format
                        println!("{}", Diff(&diff));
                        // Note about saving on stderr
                        eprintln!(
                            "{}",
                            "note: if this looks right, use -s, --save to persist these modifications"
                                .italic()
                                .yellow()
                        );
                    }
                    OutputFormat::Toml => print_config(&updated_doc, PrintFormat::Toml)?,
                    OutputFormat::Json => print_config(&updated_doc, PrintFormat::Json)?,
                }

                if self.save {
                    config.save(&updated_doc)?;
                }
            }
            return Ok(());
        }

        // 3. Handle printing only (no edits)
        if !keys_to_print.is_empty() {
            // Print requested keys only, safely extracting subtrees
            let view = config.view_keys(&keys_to_print)?;
            match self.print {
                OutputFormat::Default | OutputFormat::Toml => print_config(&view, PrintFormat::Toml)?,
                OutputFormat::Json => print_config(&view, PrintFormat::Json)?,
            }
        } else {
            // Print whole config
            let full = config.as_map();
            match self.print {
                OutputFormat::Default | OutputFormat::Toml => print_config(&full, PrintFormat::Toml)?,
                OutputFormat::Json => print_config(&full, PrintFormat::Json)?,
            }
        }

        Ok(())
    }
}

/// Wrapper for displaying config diffs in human-readable form with colors
pub struct Diff<'a>(pub &'a BTreeMap<String, (Option<String>, Option<String>)>);

impl<'a> std::fmt::Display for Diff<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.0.is_empty() {
            writeln!(f, "No changes detected.")?;
            return Ok(());
        }

        for (key, (old_val, new_val)) in self.0 {
            match (old_val, new_val) {
                (Some(old), Some(new)) if old != new => {
                    writeln!(
                        f,
                        "{} {} {}",
                        key.green().bold(),
                        "changed from".yellow(),
                        old.red()
                    )?;
                    writeln!(f, "{} {}", "to".yellow(), new.green())?;
                }
                (None, Some(new)) => {
                    writeln!(
                        f,
                        "{} {} {}",
                        key.green().bold(),
                        "set to".yellow(),
                        new.green()
                    )?;
                }
                (Some(old), None) => {
                    writeln!(
                        f,
                        "{} {} {}",
                        key.green().bold(),
                        "removed (was)".yellow(),
                        old.red()
                    )?;
                }
                _ => {
                    // No change or unexpected case, skip printing
                }
            }
        }
        Ok(())
    }
}

impl ConfigFile {
    /// Applies the edits to the config TOML document in memory, safely creating intermediate tables if needed.
    /// Returns a diff map of changed keys (old_value, new_value) and the updated TOML document.
    pub fn apply_edits(
        &mut self,
        edits: &BTreeMap<String, String>,
    ) -> Result<(BTreeMap<String, (Option<String>, Option<String>)>, Document)> {
        let mut new_doc = self.doc.clone();
        let mut diff = BTreeMap::new();

        for (key, val_str) in edits {
            let parts: Vec<&str> = key.split('.').collect();
            let mut current_item = &mut new_doc.root;

            for (i, part) in parts.iter().enumerate() {
                let is_last = i == parts.len() - 1;

                if is_last {
                    let old_val_opt = current_item.get(*part).map(|item| item.to_string());

                    let new_val = parse_toml_value(val_str)
                        .with_context(|| format!("Invalid value '{}' for key '{}'", val_str, key))?;

                    current_item[*part] = Item::Value(new_val);

                    if old_val_opt.as_deref() != Some(val_str) {
                        diff.insert(key.clone(), (old_val_opt, Some(val_str.clone())));
                    }
                } else {
                    match current_item.get_mut(*part) {
                        Some(Item::Table(t)) => {
                            current_item = t;
                        }
                        Some(_) => {
                            bail!("Key '{}' is not a table, can't set nested key '{}'", &parts[..=i].join("."), key);
                        }
                        None => {
                            // Insert intermediate empty table and descend into it
                            current_item[*part] = Item::Table(Table::new());
                            if let Item::Table(t) = current_item.get_mut(*part).unwrap() {
                                current_item = t;
                            } else {
                                unreachable!();
                            }
                        }
                    }
                }
            }
        }

        // Validate updated config by deserializing into ConfigFile
        let updated_config: ConfigFile = toml_edit::de::from_document(new_doc.clone())
            .context("Failed to validate updated config")?;

        *self = updated_config;

        Ok((diff, new_doc))
    }
}

/// Helper: parse string into a toml_edit::Value with simple heuristics
fn parse_toml_value(s: &str) -> Result<Value> {
    if let Ok(b) = s.parse::<bool>() {
        return Ok(Value::Boolean(b));
    }
    if let Ok(i) = s.parse::<i64>() {
        return Ok(Value::Integer(i));
    }
    if let Ok(f) = s.parse::<f64>() {
        return Ok(Value::Float(f));
    }
    // fallback: string
    Ok(Value::String(s.into()))
}
