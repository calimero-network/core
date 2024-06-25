use clap::Parser;

use crate::cli::RootArgs;

#[derive(Debug, Parser)]
pub struct MembersCommand {}

impl MembersCommand {
    pub async fn run(self, _root_args: RootArgs) -> eyre::Result<()> {
        println!("Running members command");
        Ok(())
    }
}
