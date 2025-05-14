// crates/merod/src/cli/config.rs

impl ConfigCommand {
    pub fn run(self, root_args: &cli::RootArgs) -> EyreResult<()> {
        let path = root_args.home.join(&root_args.node_name);

        if !ConfigFile::exists(&path) {
            bail!("Node is not initialized in {:?}", path);
        }

        let config_path = path.join(CONFIG_FILE);

        match self.command {
            ConfigSubcommand::Set { ref args, ref file } => {
                let toml_str = if let Some(file_path) = file {
                    // Read from the file specified by --file flag
                    read_to_string(file_path)
                        .map_err(|_| eyre!("Failed to read from file {:?}", file_path))?
                } else {
                    // Fall back to reading from the default config file
                    read_to_string(&config_path)
                        .map_err(|_| eyre!("Node is not initialized in {:?}", config_path))?
                };

                let mut doc = toml_str.parse::<toml_edit::DocumentMut>()?;

                if let Some(args) = args {
                    for kv in args.iter() {
                        let key_parts: Vec<&str> = kv.key.split('.').collect();

                        let mut current = doc.as_item_mut();

                        for key in &key_parts[..key_parts.len() - 1] {
                            // Check if the key exists, if not create it
                            if let Some(Item::Table(ref mut table)) = current.get_mut(key) {
                                current = table;
                            } else {
                                // If the key doesn't exist, create a new table for the key
                                let new_table = toml_edit::Table::new();
                                current[key] = Item::Table(new_table);
                                current = current.get_mut(key).unwrap().as_table_mut().unwrap();
                            }
                        }

                        // Set the final key value
                        current[key_parts[key_parts.len() - 1]] = Item::Value(kv.value.clone());
                    }
                }

                self.validate_toml(&doc)?;

                write(&config_path, doc.to_string())?;

                info!("Node configuration has been updated");
            }

            ConfigSubcommand::Print { format, ref filter, ref output } => {
                let config = ConfigFile::load(&path)?;

                let printed_config = config.print(format)?;

                if let Some(output_path) = output {
                    // Save the printed config to the specified output file
                    write(output_path, printed_config)
                        .map_err(|_| eyre!("Failed to write to output file {:?}", output_path))?;
                    info!("Config has been written to {:?}", output_path);
                } else {
                    // Print to stdout
                    println!("{}", printed_config);
                }

                if let Some(keys) = filter {
                    // Filter and print only specified keys
                    for key in keys {
                        if let Some(value) = config.get_value(key) {
                            println!("{} = {}", key, value);
                        }
                    }
                }
            }

            ConfigSubcommand::Hints => {
                ConfigFile::print_hints();
            }
        }

        Ok(())
    }

    // Validate and write TOML configuration
    fn validate_toml(&self, doc: &toml_edit::DocumentMut) -> EyreResult<()> {
        let tmp_dir = temp_dir();
        let tmp_path = tmp_dir.join(CONFIG_FILE);

        write(&tmp_path, doc.to_string())?;

        let tmp_path_utf8 = Utf8PathBuf::try_from(tmp_dir)?;

        drop(ConfigFile::load(&tmp_path_utf8)?);

        Ok(())
    }
}
