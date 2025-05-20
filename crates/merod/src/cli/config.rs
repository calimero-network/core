use clap::{Parser, ValueEnum};
use colored::*;
use std::collections::BTreeMap;
use std::path::PathBuf;

use config::config_file::{ConfigFile};
use config::format::{print_config, PrintFormat};
use config::schema::{get_schema_hint, HintFormat};

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
        let mut config = ConfigFile::load_or_default(&config_path)?;

        // Separate CLI arguments into edits, hints, and plain keys
        let mut edits: BTreeMap<String, String> = BTreeMap::new();
        let mut hints: Vec<String> = Vec::new();
        let mut keys_to_print: Vec<String> = Vec::new();

        for arg in &self.args {
            if arg.contains('=') {
                // key=value  → edit
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

        /* ------------------------------------------------------------------ */
        /* 1. Hints                                                           */
        /* ------------------------------------------------------------------ */
        if !hints.is_empty() {
            for key in &hints {
                let format = self.print.into();
                let rendered = get_schema_hint(key, format)?;
                println!("{rendered}");
            }
            return Ok(());
        }

        /* ------------------------------------------------------------------ */
        /* 2. Edits (in-memory, optional save)                                */
        /* ------------------------------------------------------------------ */
        if !edits.is_empty() {
            let (diff, updated) = config.apply_edits(&edits)?;

            if diff.is_empty() {
                eprintln!("{}", "no changes made; skipping save.".yellow());
            } else {
                match self.print {
                    OutputFormat::Default => {
                        // Human-readable diff
                        println!("{}", Diff(&diff));
                        eprintln!(
                            "{}",
                            "note: if this looks right, use -s, --save to persist these modifications"
                                .italic()
                                .yellow()
                        );
                    }
                    OutputFormat::Toml => print_config(&updated, PrintFormat::Toml)?,
                    OutputFormat::Json => print_config(&updated, PrintFormat::Json)?,
                }

                if self.save {
                    config.save(&updated)?;
                }
            }

            return Ok(());
        }

        /* ------------------------------------------------------------------ */
        /* 3. Pure printing (no edits)                                        */
        /* ------------------------------------------------------------------ */
        if !keys_to_print.is_empty() {
            // Print only the requested sub-sections
            let view = config.view_keys(&keys_to_print)?;
            match self.print {
                OutputFormat::Default | OutputFormat::Toml => {
                    print_config(&view, PrintFormat::Toml)?
                }
                OutputFormat::Json => print_config(&view, PrintFormat::Json)?,
            }
        } else {
            // Print the whole config
            let full = config.as_map();
            match self.print {
                OutputFormat::Default | OutputFormat::Toml => {
                    print_config(&full, PrintFormat::Toml)?
                }
                OutputFormat::Json => print_config(&full, PrintFormat::Json)?,
            }
        }

        Ok(())
    }
}

/// Wrapper for displaying config diffs in human-readable form
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
                    // No change or unknown case - skip printing
                }
            }
        }
        Ok(())
    }
}
