use clap::Parser;
use eyre::Result;
use owo_colors::OwoColorize;
use serde_json::{from_slice as from_json_slice, Value};

use crate::Node;

#[derive(Debug, Parser)]
#[allow(missing_copy_implementations)]
pub struct PoolCommand;

impl PoolCommand {
    pub async fn run(self, node: &Node) -> Result<()> {
        let ind = ">>".blue();
        if node.tx_pool.transactions.is_empty() {
            println!("{ind} Transaction pool is empty.");
        }
        for (hash, entry) in &node.tx_pool.transactions {
            println!("{ind} • {:?}", hash.cyan());
            println!("{ind}     Sender: {}", entry.sender.cyan());
            println!("{ind}     Method: {:?}", entry.transaction.method.cyan());
            println!("{ind}     Payload:");
            #[expect(clippy::option_if_let_else, reason = "Clearer here")]
            let payload = if let Ok(value) = from_json_slice::<Value>(&entry.transaction.payload) {
                format!(
                    "(json): {}",
                    format!("{value:#}")
                        .lines()
                        .map(|line| line.cyan().to_string())
                        .collect::<Vec<_>>()
                        .join("\n")
                )
            } else {
                format!("(raw): {:?}", entry.transaction.payload.cyan())
            };

            for line in payload.lines() {
                println!("{ind}       > {line}");
            }
            println!("{ind}     Prior: {:?}", entry.transaction.prior_hash.cyan());
        }

        Ok(())
    }
}
