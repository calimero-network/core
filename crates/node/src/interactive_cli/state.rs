use calimero_node_primitives::client::NodeClient;
use calimero_primitives::alias::Alias;
use calimero_primitives::context::ContextId;
use calimero_primitives::hash::Hash;
use calimero_store::key::ContextState as ContextStateKey;
use calimero_store::Store;
use clap::Parser;
use eyre::{bail, Result as EyreResult};
use owo_colors::OwoColorize;

/// View the raw state of contexts
#[derive(Copy, Clone, Debug, Parser)]
pub struct StateCommand {
    /// The context to view the state for
    context: Option<Alias<ContextId>>,
}

impl StateCommand {
    pub fn run(self, node_client: &NodeClient, datastore: Store) -> EyreResult<()> {
        let ind = ">>".blue();

        let mut context_id = None;

        if let Some(context) = self.context {
            let Some(id) = node_client.resolve_alias(context, None)? else {
                bail!("unable to resolve context");
            };

            context_id = Some(id);
        }

        let handle = datastore.handle();
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
