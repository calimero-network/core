use std::collections::HashMap;
use std::fmt::Debug;
use std::fs::File;
use std::io::{self, Write};
use serde::{Deserialize, Serialize};
use serde_json::json;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Item {
    Value(String),
    Map(HashMap<String, Item>),
}

// Function to navigate and set a value in the nested HashMap
fn set_value_in_config(current: &mut HashMap<String, Item>, key_parts: &[&str], value: String) -> Result<(), String> {
    let mut current_map = current;

    // Iterate through key parts to navigate the nested structure
    for &key in &key_parts[..key_parts.len() - 1] {
        match current_map.get_mut(key) {
            Some(Item::Map(map)) => {
                current_map = map; // Navigate deeper if it's a Map
            }
            Some(_) => {
                // If we find a value but not a map, return an error
                return Err(format!("Expected a map at key '{}', but found a value", key));
            }
            None => {
                // If the key does not exist, insert a new Map at that level
                let new_map = HashMap::new();
                current_map.insert(key.to_string(), Item::Map(new_map));
                if let Item::Map(ref mut map) = current_map[key] {
                    current_map = map; // Navigate into the new map
                }
            }
        }
    }

    // Set the value for the final key part
    let last_key = key_parts[key_parts.len() - 1];
    current_map.insert(last_key.to_string(), Item::Value(value));

    Ok(())
}

// Function to print the configuration based on the format
fn print_config(config: &HashMap<String, Item>, print_format: &str) {
    match print_format {
        "json" => {
            let json = serde_json::to_string(config).unwrap();
            println!("{}", json);
        },
        "toml" => {
            // Use your own TOML serialization here
            let toml = toml::to_string(config).unwrap();
            println!("{}", toml);
        },
        "default" => {
            for (key, value) in config.iter() {
                match value {
                    Item::Value(val) => println!("{} = {}", key, val),
                    Item::Map(map) => {
                        println!("{} = {{", key);
                        print_config(map, "default");
                        println!("}}");
                    }
                }
            }
        },
        _ => {
            eprintln!("Unsupported print format: {}", print_format);
        }
    }
}

// Main function to parse and run commands
fn main() {
    // Example configuration structure
    let mut config: HashMap<String, Item> = HashMap::new();

    // Simulate setting a value in a nested configuration
    let key_parts = ["a", "b", "c"];
    let value = "new_value".to_string();
    if let Err(e) = set_value_in_config(&mut config, &key_parts, value) {
        eprintln!("Error: {}", e);
    } else {
        println!("Config updated successfully!");
    }

    // Example: Print the config in JSON format
    println!("Printing configuration in JSON format:");
    print_config(&config, "json");

    // Example: Save the updated config to a file if needed
    if let Err(e) = save_config_to_file(&config, "config.toml") {
        eprintln!("Error saving config to file: {}", e);
    }
}

// Function to save the config to a file
fn save_config_to_file(config: &HashMap<String, Item>, path: &str) -> io::Result<()> {
    let file = File::create(path)?;
    let mut writer = io::BufWriter::new(file);

    // Serialize to TOML or JSON and write to the file
    let toml = toml::to_string(config).unwrap();  // Ensure proper error handling here
    writer.write_all(toml.as_bytes())?;
    writer.flush()?;

    println!("Configuration saved to {}", path);
    Ok(())
}
