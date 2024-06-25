use clap::Parser;

use crate::cli::RootArgs;

#[derive(Debug, Parser)]
pub struct QueryCommand {}

impl QueryCommand {
    pub async fn run(self, _root_args: RootArgs) -> eyre::Result<()> {
        println!("Running query command");
        Ok(())
    }
}
