use std::fs::{read_to_string, write};
use std::env::temp_dir;

use camino::Utf8PathBuf;
use eyre::{eyre, bail, Result as EyreResult};
use tracing::info;
use toml_edit::{DocumentMut, Item};

use crate::cli::{self, ConfigKeyVal, ConfigPrintFormat, ConfigSubcommand};
use crate::config_file::{ConfigFile, CONFIG_FILE};

impl ConfigCommand {
    pub fn run(self, root_args: &cli::RootArgs) -> EyreResult<()> {
        let path = root_args.home.join(&root_args.node_name);

        if !ConfigFile::exists(&path) {
            bail!("Node is not initialized in {:?}", path);
        }

        let config_path = path.join(CONFIG_FILE);

        match self.command {
            ConfigSubcommand::Set { args } => {
                let file = &args.file;
                let print_format = args.print.clone().unwrap_or(ConfigPrintFormat::Default);
                let should_save = args.save;

                let toml_str = if let Some(file_path) = file {
                    read_to_string(file_path)
                        .map_err(|_| eyre!("Failed to read from file {:?}", file_path))?
                } else {
                    read_to_string(&config_path)
                        .map_err(|_| eyre!("Node is not initialized in {:?}", config_path))?
                };

                let mut doc = toml_str.parse::<DocumentMut>()?;
                let original = doc.clone(); // Save original for diff

                let mut modified = false;
                let mut hint_keys = vec![];

                if let Some(kvs) = &args.args {
                    for kv in kvs {
                        if kv.key.ends_with('?') {
                            hint_keys.push(kv.key.trim_end_matches('?').to_string());
                            continue;
                        }

                        let key_parts: Vec<&str> = kv.key.split('.').collect();
                        let mut current = doc.as_item_mut();

                        for key in &key_parts[..key_parts.len() - 1] {
                            current = current[key]
                                .or_insert(Item::Table(Default::default()))
                                .as_table_mut()
                                .unwrap();
                        }

                        let last = key_parts[key_parts.len() - 1];
                        let old_value = current.get(last).cloned();
                        current[last] = Item::Value(kv.value.clone());

                        if Some(&Item::Value(kv.value.clone())) != old_value.as_ref() {
                            modified = true;
                        }
                    }
                }

                // Handle schema hints
                if !hint_keys.is_empty() {
                    for hint_key in hint_keys {
                        ConfigFile::print_schema_for_key(&hint_key);
                    }
                    return Ok(());
                }

                // Validate new config
                self.validate_toml(&doc)?;

                // Handle output
                match print_format {
                    ConfigPrintFormat::Default => {
                        if modified {
                            let old_lines: Vec<_> = original.to_string().lines().collect();
                            let new_lines: Vec<_> = doc.to_string().lines().collect();
                            for (old, new) in old_lines.iter().zip(new_lines.iter()) {
                                if old != new {
                                    eprintln!("-{}", old);
                                    eprintln!("+{}", new);
                                }
                            }
                            eprintln!("\nNote: if this looks right, use `-s, --save` to persist these modifications.");
                        } else {
                            eprintln!("No changes made.");
                        }
                    }
                    ConfigPrintFormat::Toml => {
                        println!("{}", doc.to_string());
                    }
                    ConfigPrintFormat::Json => {
                        let config: toml::Value = doc.clone().try_into()?;
                        println!("{}", serde_json::to_string_pretty(&config)?);
                    }
                }

                if should_save && modified {
                    write(&config_path, doc.to_string())?;
                    info!("Node configuration has been updated and saved.");
                } else if should_save {
                    eprintln!("No changes detected. Nothing was saved.");
                }

                Ok(())
            }

            ConfigSubcommand::Print { format, ref filter, ref output } => {
                let config = ConfigFile::load(&path)?;

                let printed_config = config.print(format)?;

                if let Some(output_path) = output {
                    write(output_path, printed_config)
                        .map_err(|_| eyre!("Failed to write to output file {:?}", output_path))?;
                    info!("Config has been written to {:?}", output_path);
                } else {
                    println!("{}", printed_config);
                }

                if let Some(keys) = filter {
                    for key in keys {
                        if let Some(value) = config.get_value(key) {
                            println!("{} = {}", key, value);
                        }
                    }
                }

                Ok(())
            }

            ConfigSubcommand::Hints => {
                ConfigFile::print_hints();
                Ok(())
            }
        }
    }

    fn validate_toml(&self, doc: &DocumentMut) -> EyreResult<()> {
        let tmp_dir = temp_dir();
        let tmp_path = tmp_dir.join(CONFIG_FILE);

        write(&tmp_path, doc.to_string())?;

        let tmp_path_utf8 = Utf8PathBuf::try_from(tmp_dir)?;

        drop(ConfigFile::load(&tmp_path_utf8)?);

        Ok(())
    }
}
