// crates/merod/src/cli/config.rs

use std::fs::{read_to_string, write};
use std::path::PathBuf;

use camino::Utf8PathBuf;
use colored::*;
use eyre::{bail, eyre, Result as EyreResult};
use serde_json::json;
use toml_edit::{DocumentMut, Item};

use crate::cli::RootArgs;
use config::{ConfigFile, OutputFormat};

#[derive(Debug, clap::Subcommand)]
pub enum ConfigSubcommand {
    /// View or modify configuration values
    #[command(alias = "set")]
    Set {
        /// Key-value pairs to edit, or schema hint keys (key?)
        #[arg(value_name = "KEY=VALUE / KEY?")]
        args: Vec<String>,

        /// Format to print output [default|toml|json]
        #[arg(long, value_enum, default_value_t = OutputFormat::Pretty)]
        print: OutputFormat,

        /// Save changes to file
        #[arg(short, long)]
        save: bool,
    },

    /// Print the config file
    Print {
        /// Output format [default|toml|json]
        #[arg(long, value_enum, default_value_t = OutputFormat::Pretty)]
        format: OutputFormat,

        /// Specific keys to filter and print
        #[arg(value_name = "KEYS")]
        filter: Vec<String>,

        /// Optional output path
        #[arg(long)]
        output: Option<PathBuf>,
    },

    /// Show config schema hints
    Hints,
}

#[derive(Debug, clap::Args)]
pub struct ConfigCommand {
    #[command(subcommand)]
    pub command: ConfigSubcommand,
}

impl ConfigCommand {
    pub fn run(self, root_args: &RootArgs) -> EyreResult<()> {
        let path = root_args.home.join(&root_args.node_name);
        if !ConfigFile::exists(&path) {
            bail!("Node is not initialized in {:?}", path);
        }

        let config_path = path.join(config::CONFIG_FILE);

        match self.command {
            ConfigSubcommand::Hints => {
                ConfigFile::print_hints();
            }

            ConfigSubcommand::Print { format, filter, output } => {
                let config = ConfigFile::load(&path)?;
                if filter.is_empty() {
                    let output_str = match format {
                        OutputFormat::Pretty => format!("{:#?}", config),
                        OutputFormat::Json => serde_json::to_string_pretty(&config)?,
                    };

                    if let Some(out) = output {
                        write(&out, output_str)?;
                        eprintln!("Config written to {:?}", out);
                    } else {
                        println!("{}", output_str);
                    }
                } else {
                    for key in filter {
                        if let Some(value) = config.get_value(&key) {
                            println!("{} = {}", key, value);
                        } else {
                            eprintln!("Key `{}` not found in config", key);
                        }
                    }
                }
            }

            ConfigSubcommand::Set { args, print, save } => {
                let mut hint_keys = vec![];
                let mut kv_edits = vec![];

                for arg in args {
                    if arg.ends_with('?') {
                        hint_keys.push(arg.trim_end_matches('?').to_string());
                    } else if let Some((k, v)) = arg.split_once('=') {
                        kv_edits.push((k.to_string(), v.to_string()));
                    } else {
                        eprintln!("Invalid argument format: {}", arg);
                        continue;
                    }
                }

                if !hint_keys.is_empty() {
                    for key in hint_keys {
                        ConfigFile::print_hint_for_key(&key);
                    }
                    return Ok(()); // Don't apply any changes if hints are requested
                }

                let original = read_to_string(&config_path)?;
                let mut doc = original.parse::<DocumentMut>()?;

                let mut changed = false;
                let mut diffs = vec![];

                for (key, val) in kv_edits {
                    let parts: Vec<&str> = key.split('.').collect();
                    let mut current = doc.as_item_mut();

                    for part in &parts[..parts.len() - 1] {
                        current = current
                            .as_table_like_mut()
                            .unwrap()
                            .entry(part)
                            .or_insert(Item::Table(toml_edit::Table::new()));
                    }

                    let last = parts.last().unwrap();
                    let new_value = toml_edit::value(val.clone());

                    let old_item = current.get(last).cloned();
                    if old_item != Some(Item::Value(new_value.clone())) {
                        changed = true;
                        diffs.push((key.clone(), old_item, Some(new_value.clone())));
                        current[last] = Item::Value(new_value);
                    }
                }

                if changed {
                    // Validate changes
                    self.validate_toml(&doc)?;

                    match print {
                        OutputFormat::Pretty => {
                            for (k, old, new) in &diffs {
                                if let Some(o) = old {
                                    eprintln!("{}", format!("-{} = {}", k, o).red());
                                }
                                if let Some(n) = new {
                                    eprintln!("{}", format!("+{} = {}", k, n).green());
                                }
                            }
                        }
                        OutputFormat::Json => {
                            let json_obj: serde_json::Value = toml::de::from_str(&doc.to_string())?;
                            println!("{}", serde_json::to_string_pretty(&json_obj)?);
                        }
                    }

                    eprintln!(
                        "{}",
                        "note: if this looks right, use `-s, --save` to persist these modifications"
                            .yellow()
                    );

                    if save {
                        write(&config_path, doc.to_string())?;
                        eprintln!("Changes saved to config.");
                    }
                } else {
                    eprintln!("No changes detected.");
                }
            }
        }

        Ok(())
    }

    fn validate_toml(&self, doc: &DocumentMut) -> EyreResult<()> {
        let tmp_path = std::env::temp_dir().join(config::CONFIG_FILE);
        write(&tmp_path, doc.to_string())?;
        drop(ConfigFile::load(&Utf8PathBuf::from_path_buf(tmp_path)?));
        Ok(())
    }
}
