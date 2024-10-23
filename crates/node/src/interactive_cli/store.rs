use calimero_primitives::hash::Hash;
use calimero_store::key::ContextState as ContextStateKey;
use clap::Parser;
use eyre::Result;
use owo_colors::OwoColorize;

use crate::Node;

#[derive(Debug, Parser)]
#[allow(missing_copy_implementations, reason = "TODO")]
#[non_exhaustive]
pub struct StoreCommand;

impl StoreCommand {
    // todo! revisit: get specific context state
    pub async fn run(self, node: &Node) -> Result<()> {
        println!("Executing Store command");
        let ind = ">>".blue();

        println!(
            "{ind} {c1:44} | {c2:44} | Value",
            c1 = "Context ID",
            c2 = "State Key",
        );

        let handle = node.store.handle();

        for (k, v) in handle.iter::<ContextStateKey>()?.entries() {
            let (k, v) = (k?, v?);
            let (cx, state_key) = (k.context_id(), k.state_key());
            let sk = Hash::from(state_key);
            let entry = format!("{c1:44} | {c2:44}| {c3:?}", c1 = cx, c2 = sk, c3 = v.value);
            for line in entry.lines() {
                println!("{ind} {}", line.cyan());
            }
        }

        Ok(())
    }
}
