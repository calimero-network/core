use calimero_primitives::{context::ContextId, hash::Hash};
use calimero_store::key::ContextState as ContextStateKey;
use clap::Parser;
use eyre::Result;
use owo_colors::OwoColorize;

use crate::Node;

#[derive(Debug, Parser)]
pub struct StateCommand {
    context_id: String,
}
impl StateCommand {
    pub async fn run(self, node: &Node) -> Result<()> {
        let ind = ">>".blue();
        let handle = node.store.handle();

        println!("{ind} {:44} | {:44}", "State Key", "Value");

        let mut iter = handle.iter::<ContextStateKey>()?;

        for (k, v) in iter.entries() {
            let (k, v) = (k?, v?);
            let context_id_bytes: [u8; 32] = self
                .context_id
                .as_bytes()
                .try_into()
                .expect("Context ID must be 32 bytes long");
            if k.context_id() != ContextId::from(context_id_bytes) {
                continue;
            }
            let entry = format!("{:44} | {:?}", Hash::from(k.state_key()), v.value,);
            for line in entry.lines() {
                println!("{ind} {}", line.cyan());
            }
        }

        Ok(())
    }
}
