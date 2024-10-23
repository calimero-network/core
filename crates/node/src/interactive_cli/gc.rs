use clap::Parser;
use eyre::Result;
use owo_colors::OwoColorize;

use crate::transaction_pool::TransactionPool;
use crate::Node;

#[derive(Debug, Parser)]
#[allow(missing_copy_implementations, reason = "TODO")]
#[non_exhaustive]
pub struct GarbageCollectCommand;
impl GarbageCollectCommand {
    pub async fn run(self, node: &mut Node) -> Result<()> {
        let ind = ">>".blue();
        if node.tx_pool.transactions.is_empty() {
            println!("{ind} Transaction pool is empty.");
        } else {
            println!(
                "{ind} Garbage collecting {} transactions.",
                node.tx_pool.transactions.len().cyan()
            );
            node.tx_pool = TransactionPool::default();
        }

        Ok(())
    }
}
