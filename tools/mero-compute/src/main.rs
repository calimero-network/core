//! `mero-compute`: kept for one release; the command is `meroctl local compute run`.

use std::process::ExitCode;

use clap::{Parser, Subcommand};
use mero_compute::RunArgs;

#[derive(Parser)]
#[command(name = "mero-compute", version)]
#[command(
    about = "Test a compute component alone, as a job node would run it (deprecated: use `meroctl local compute run`)"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Run one compute method
    Run(RunArgs),
}

fn main() -> ExitCode {
    eprintln!("mero-compute is deprecated: use `meroctl local compute run`");
    let Command::Run(args) = Cli::parse().command;
    mero_compute::main_run(args)
}
