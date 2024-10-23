use std::str::FromStr;

use calimero_primitives::context::ContextId;
use calimero_primitives::hash::Hash;
use calimero_store::key::ContextTransaction as ContextTransactionKey;
use clap::Parser;
use eyre::Result;
use owo_colors::OwoColorize;

use crate::Node;

#[derive(Debug, Parser)]
pub struct TransactionsCommand {
    context_id: String,
}

impl TransactionsCommand {
    pub async fn run(self, node: &Node) -> Result<()> {
        let handle = node.store.handle();
        let mut iter = handle.iter::<ContextTransactionKey>()?;

        let first = 'first: {
            let context_id = ContextId::from_str(&self.context_id)?;
            let Some(k) = iter
                .seek(ContextTransactionKey::new(context_id, [0u8; 32]))
                .transpose()
            else {
                break 'first None;
            };

            Some((k, iter.read()))
        };

        println!("{:44} | {:44}", "Hash", "Prior Hash");

        for (k, v) in first.into_iter().chain(iter.entries()) {
            let (k, v) = (k?, v?);
            let entry = format!(
                "{:44} | {}",
                Hash::from(k.transaction_id()),
                Hash::from(v.prior_hash),
            );
            for line in entry.lines() {
                println!("{}", line.cyan());
            }
        }

        Ok(())
    }
}
