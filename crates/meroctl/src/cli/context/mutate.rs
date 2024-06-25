use clap::Parser;

use crate::cli::RootArgs;

#[derive(Debug, Parser)]
pub struct MutateCommand {}

impl MutateCommand {
    pub async fn run(self, _root_args: RootArgs) -> eyre::Result<()> {
        println!("Running mutate command");
        Ok(())
    }
}
