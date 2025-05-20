use std::collections::BTreeMap;
use std::io::{self, Write};
use std::path::PathBuf;

use anyhow::{anyhow, Context, Result};
use clap::Args;
use colored::*;
use config::ConfigFile;
use serde_json::{json, Value as JsonValue};
use schemars::schema::Schema;

/// Config subcommand options
#[derive(Debug, Args)]
pub struct ConfigOpts {
    /// Save changes to config file
    #[arg(short, long)]
    save: bool,

    /// Output format (default, toml, json)
    #[arg(long, value_parser = ["default", "toml", "json"], default_value = "default")]
    print: String,

    /// Configuration edits or queries
    pub args: Vec<String>,
}

/// Entry point for `merod config` command
pub fn run(opts: ConfigOpts) -> Result<()> {
    let mut config = ConfigFile::load_or_default()?;
    let original = config.to_value()?;
    let mut updated = config.clone();

    let mut keys_edited = vec![];
    let mut schema_queries = vec![];

    for arg in &opts.args {
        if let Some(eq_idx) = arg.find('=') {
            let (key, value) = arg.split_at(eq_idx);
            let value = &value[1..]; // skip '='

            updated.set_from_str(key, value)?;
            keys_edited.push(key.to_string());
        } else if arg.ends_with('?') {
            schema_queries.push(arg.trim_end_matches('?').to_string());
        }
    }

    // If only schema queries are requested
    if !schema_queries.is_empty() && keys_edited.is_empty() && schema_queries.len() == opts.args.len() {
        let fmt = match opts.print.as_str() {
            "json" => HintFormat::Json,
            "toml" => HintFormat::Toml,
            _ => HintFormat::Human,
        };
        for key in schema_queries {
            let hint = get_schema_hint(&key, fmt)?;
            println!("{hint}");
        }
        return Ok(());
    }

    let updated_val = updated.to_value()?;
    let is_changed = original != updated_val;
    let output_fmt = match opts.print.as_str() {
        "default" => "toml",
        other => other,
    };

    if !keys_edited.is_empty() {
        if is_changed {
            match output_fmt {
                "json" => {
                    let out = serde_json::to_string_pretty(&updated_val)?;
                    println!("{out}");
                }
                "toml" => {
                    let out = toml::to_string_pretty(&updated)?;
                    println!("{out}");
                }
                _ => unreachable!(),
            }

            if opts.save {
                updated.save()?;
                eprintln!("{}", "Saved updated config.".green());
            } else {
                eprintln!("{}", "Note: if this looks right, use `-s, --save` to persist these modifications".yellow());
            }
        } else {
            eprintln!("{}", "No changes detected.".yellow());
        }
    } else if opts.args.is_empty() {
        // No keys = print full config
        match output_fmt {
            "json" => println!("{}", serde_json::to_string_pretty(&original)?),
            "toml" => println!("{}", toml::to_string_pretty(&config)?),
            _ => unreachable!(),
        }
    } else {
        // Print partial config
        let mut partial = serde_json::Map::new();
        for key in &opts.args {
            let parts: Vec<&str> = key.split('.').collect();
            if let Some(val) = get_value_by_path(&original, &parts) {
                insert_into_map(&mut partial, &parts, val.clone());
            } else {
                return Err(anyhow!("Key '{}' not found", key));
            }
        }

        match output_fmt {
            "json" => println!("{}", serde_json::to_string_pretty(&JsonValue::Object(partial))?),
            "toml" => {
                let as_toml = json_to_toml(&JsonValue::Object(partial));
                println!("{}", toml::to_string_pretty(&as_toml)?);
            }
            _ => {
                for (k, v) in flatten_json(&JsonValue::Object(partial), None) {
                    println!("{} = {}", k, v);
                }
            }
        }
    }

    Ok(())
}

/// Show unified diff between old and new JSON values
fn print_diff(old: &JsonValue, new: &JsonValue) -> Result<()> {
    let diff = diffs::json::diff(old, new);

    for d in diff {
        match d {
            diffs::json::Diff::Added(p, v) => {
                println!("{} {}", "+".green(), format!("{} = {}", p, v).green());
            }
            diffs::json::Diff::Removed(p, v) => {
                println!("{} {}", "-".red(), format!("{} = {}", p, v).red());
            }
            diffs::json::Diff::Modified(p, old_v, new_v) => {
                println!("{} {}", "-".red(), format!("{} = {}", p, old_v).red());
                println!("{} {}", "+".green(), format!("{} = {}", p, new_v).green());
            }
            _ => {}
        }
    }

    Ok(())
}

/// Flatten nested JSON object into flat map with dot keys
fn flatten_json(val: &JsonValue, prefix: Option<String>) -> Vec<(String, JsonValue)> {
    let mut result = vec![];
    match val {
        JsonValue::Object(map) => {
            for (k, v) in map {
                let full_key = match &prefix {
                    Some(p) => format!("{p}.{k}"),
                    None => k.clone(),
                };
                result.extend(flatten_json(v, Some(full_key)));
            }
        }
        _ => {
            if let Some(k) = prefix {
                result.push((k, val.clone()));
            }
        }
    }
    result
}

/// Access value by dotted key path
fn get_value_by_path(val: &JsonValue, path: &[&str]) -> Option<&JsonValue> {
    let mut current = val;
    for p in path {
        match current {
            JsonValue::Object(map) => {
                current = map.get(*p)?;
            }
            _ => return None,
        }
    }
    Some(current)
}

/// Insert a value into a nested JSON map
fn insert_into_map(map: &mut serde_json::Map<String, JsonValue>, path: &[&str], val: JsonValue) {
    if path.len() == 1 {
        map.insert(path[0].to_string(), val);
        return;
    }

    let entry = map.entry(path[0].to_string()).or_insert_with(|| json!({}));
    if let JsonValue::Object(m) = entry {
        insert_into_map(m, &path[1..], val);
    }
}

/// Enum for schema hint formats
#[derive(Clone, Copy)]
enum HintFormat {
    Human,
    Json,
    Toml,
}

/// Show schema hint for a key
fn get_schema_hint(key: &str, format: HintFormat) -> Result<String> {
    let schema = ConfigFile::schema();
    let parts: Vec<&str> = key.split('.').collect();
    let subschema = get_subschema_for_path(&schema.schema, &parts)
        .ok_or_else(|| anyhow!("Unknown key '{}'", key))?;

    match format {
        HintFormat::Human => {
            let typ = subschema.instance_type.as_ref()
                .and_then(|t| t.first())
                .map(|t| t.to_string())
                .unwrap_or_else(|| "unknown".to_string());

            let desc = subschema.metadata.as_ref()
                .and_then(|m| m.description.clone())
                .unwrap_or_default();

            Ok(format!("{}: {} {}", key.bold(), typ, desc))
        }
        HintFormat::Toml => {
            let example = subschema.default.clone().unwrap_or(json!(""));
            let mut toml_map = toml::value::Table::new();
            insert_toml_value(&mut toml_map, &parts, &example);
            Ok(toml::to_string_pretty(&toml::Value::Table(toml_map))?)
        }
        HintFormat::Json => {
            let mut json_map = serde_json::Map::new();
            insert_json_value(&mut json_map, &parts, subschema);
            Ok(serde_json::to_string_pretty(&JsonValue::Object(json_map))?)
        }
    }
}

/// Traverse schema to get subschema by path
fn get_subschema_for_path<'a>(schema: &'a Schema, path: &[&str]) -> Option<&'a Schema> {
    if path.is_empty() {
        return Some(schema);
    }

    if let Some(props) = schema.object.as_ref().map(|o| &o.properties) {
        if let Some(sub) = props.get(path[0]) {
            get_subschema_for_path(sub, &path[1..])
        } else {
            None
        }
    } else {
        None
    }
}

/// Insert TOML value into nested table
fn insert_toml_value(map: &mut toml::value::Table, path: &[&str], val: &JsonValue) {
    if path.len() == 1 {
        map.insert(path[0].to_string(), json_to_toml(val));
        return;
    }

    let entry = map.entry(path[0].to_string()).or_insert_with(|| toml::Value::Table(Default::default()));
    if let toml::Value::Table(ref mut sub) = entry {
        insert_toml_value(sub, &path[1..], val);
    }
}

/// Insert JSON schema value at path
fn insert_json_value(map: &mut serde_json::Map<String, JsonValue>, path: &[&str], schema: &Schema) {
    if path.len() == 1 {
        let mut obj = serde_json::Map::new();
        if let Some(ty) = &schema.instance_type {
            obj.insert("type".to_string(), json!(ty));
        }
        if let Some(desc) = &schema.metadata.as_ref().and_then(|m| m.description.clone()) {
            obj.insert("description".to_string(), json!(desc));
        }
        if let Some(def) = &schema.default {
            obj.insert("default".to_string(), def.clone());
        }
        map.insert(path[0].to_string(), JsonValue::Object(obj));
        return;
    }

    let entry = map.entry(path[0].to_string()).or_insert_with(|| json!({}));
    if let JsonValue::Object(inner) = entry {
        insert_json_value(inner, &path[1..], schema);
    }
}

/// Convert JSON to TOML best-effort
fn json_to_toml(val: &JsonValue) -> toml::Value {
    match val {
        JsonValue::Null => toml::Value::String("null".to_string()),
        JsonValue::Bool(b) => toml::Value::Boolean(*b),
        JsonValue::Number(n) => {
            if let Some(i) = n.as_i64() {
                toml::Value::Integer(i)
            } else if let Some(f) = n.as_f64() {
                toml::Value::Float(f)
            } else {
                toml::Value::String(n.to_string())
            }
        }
        JsonValue::String(s) => toml::Value::String(s.clone()),
        JsonValue::Array(arr) => toml::Value::Array(arr.iter().map(json_to_toml).collect()),
        JsonValue::Object(map) => {
            let mut tbl = toml::value::Table::new();
            for (k, v) in map {
                tbl.insert(k.clone(), json_to_toml(v));
            }
            toml::Value::Table(tbl)
        }
    }
}
