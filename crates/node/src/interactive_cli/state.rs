use calimero_primitives::context::ContextId;
use calimero_primitives::hash::Hash;
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
        let mut iter = handle.iter::<ContextStateKey>()?;

        println!("{ind} {:44} | {:44}", "State Key", "Value");

        let context_id = match self.context_id.parse::<ContextId>() {
            Ok(id) => id,
            Err(e) => eyre::bail!("{} Failed to parse context_id: {}", ind.red(), e),
        };

        let first = 'first: {
            let Some(k) = iter
                .seek(ContextStateKey::new(context_id, [0; 32].into()))
                .transpose()
            else {
                break 'first None;
            };

            Some((k, iter.read().map(|s| s.value.into_boxed())))
        };

        let rest = iter
            .entries()
            .map(|(k, v)| (k, v.map(|s| s.value.into_boxed())));

        for (k, v) in first.into_iter().chain(rest) {
            let (k, v) = (k?, v?);
            if k.context_id() != context_id {
                break;
            }
            let entry = format!("{:44} | {:?}", Hash::from(k.state_key()), v);
            for line in entry.lines() {
                println!("{ind} {}", line.cyan());
            }
        }

        Ok(())
    }
}
