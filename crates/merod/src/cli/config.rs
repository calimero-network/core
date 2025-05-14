//! src/cli/config.rs
//! Phase-1: only parse flags and selectors; no file IO yet.

use camino::Utf8PathBuf;
use clap::{Parser, ValueEnum};
use eyre::Result as EyreResult;

use super::RootArgs;   // `RootArgs` is defined in cli/mod.rs (your old cli.rs)

/*───────────────────────────────────────────────────────────
   merod config …   CLI surface
  ───────────────────────────────────────────────────────────*/
#[derive(Debug, Parser)]
pub struct ConfigCommand {
    /// Output format: default | toml | json
    #[arg(long = "print", value_enum, default_value_t = PrintFmt::Default)]
    pub print: PrintFmt,

    /// Persist edits (`-s` or `--save[=PATH]`)
    ///
    ///   -s              → Some(None)
    ///   --save=foo.toml → Some(Some("foo.toml"))
    #[arg(short, long = "save")]
    pub save: Option<Option<Utf8PathBuf>>,

    /// Any number of `KEY`, `KEY=VALUE`, or `KEY?`
    #[arg(value_name = "SELECTOR")]
    pub selectors: Vec<String>,
}

/// Allowed values for `--print`
#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum PrintFmt {
    Default,
    Toml,
    Json,
}

/*───────────────────────────────────────────────────────────
   Entry point called from RootCommand
  ───────────────────────────────────────────────────────────*/
impl ConfigCommand {
    pub fn run(self, _root: &RootArgs) -> EyreResult<()> {
        // Phase-1 goal: prove exact flag parsing
        println!("--print = {:?}", self.print);

        match self.save {
            None               => println!("--save  = <not provided>"),
            Some(None)         => println!("--save  = rewrite original file"),
            Some(Some(path))   => println!("--save  = {}", path),
        }

        println!("selectors = {:?}", self.selectors);
        Ok(())
    }
}
