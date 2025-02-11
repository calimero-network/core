use calimero_primitives::alias::Kind;
use calimero_primitives::hash::Hash;
use calimero_store::key::ContextState as ContextStateKey;
use clap::Parser;
use eyre::Result as EyreResult;
use owo_colors::OwoColorize;

use crate::interactive_cli::commons::resolve_identifier;
use crate::Node;

/// View the raw state of contexts
#[derive(Clone, Debug, Parser)]
pub struct StateCommand {
    /// The context ID to view the state for
    context_id: Option<String>,
}

impl StateCommand {
    pub fn run(self, node: &Node) -> EyreResult<()> {
        let ind = ">>".blue();

        let context_id = self
            .context_id
            .map(|context_inner| resolve_identifier(node, &context_inner, Kind::Context, None))
            .transpose()?
            .map(|hash| hash.into());

        let handle = node.store.handle();
        let mut iter = handle.iter::<ContextStateKey>()?;

        println!(
            "{ind} {c1:44} | {c2:44} | Value",
            c1 = "Context ID",
            c2 = "State Key",
        );

        let first = context_id.and_then(|s| {
            Some((
                iter.seek(ContextStateKey::new(s, [0; 32])).transpose()?,
                iter.read().map(|v| v.value.into_boxed()),
            ))
        });

        let rest = iter
            .entries()
            .map(|(k, v)| (k, v.map(|v| v.value.into_boxed())));

        for (k, v) in first.into_iter().chain(rest) {
            let (k, v) = (k?, v?);

            let (cx, state_key) = (k.context_id(), k.state_key());

            if let Some(context_id) = context_id {
                if cx != context_id {
                    break;
                }
            }

            let sk = Hash::from(state_key);

            let entry = format!("{c1:44} | {c2:44} | {c3:?}", c1 = cx, c2 = sk, c3 = v);
            for line in entry.lines() {
                println!("{ind} {}", line.cyan());
            }
        }

        Ok(())
    }
}
