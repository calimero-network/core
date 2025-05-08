use calimero_node_primitives::client::NodeClient;
use clap::{Parser, Subcommand};
use eyre::Result as EyreResult;
use owo_colors::OwoColorize;


#[derive(Debug, Parser)]
#[non_exhaustive]
pub struct StoreCommand {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
    Ls,
    Set,
    Get,
}

impl StoreCommand {
    // todo! revisit: get specific context state
    pub fn run(self, _node: &NodeClient) -> EyreResult<()> {
        let ind = ">>".blue();

        println!("{ind} Not implemented yet",);

        // println!(
        //     "{ind} {c1:44} | {c2:44} | Value",
        //     c1 = "Context ID",
        //     c2 = "State Key",
        // );

        // let handle = node.store.handle();

        // let mut iter = handle.iter::<ContextStateKey>()?;

        // let first = self.context_id.and_then(|s| {
        //     Some((
        //         iter.seek(ContextStateKey::new(s, [0; 32])).transpose()?,
        //         iter.read().map(|v| v.value.into_boxed()),
        //     ))
        // });

        // let rest = iter
        //     .entries()
        //     .map(|(k, v)| (k, v.map(|v| v.value.into_boxed())));

        // for (k, v) in first.into_iter().chain(rest) {
        //     let (k, v) = (k?, v?);

        //     let (cx, state_key) = (k.context_id(), k.state_key());

        //     if let Some(context_id) = self.context_id {
        //         if context_id != cx {
        //             break;
        //         }
        //     }

        //     let sk = Hash::from(state_key);

        //     let entry = format!("{c1:44} | {c2:44} | {c3:?}", c1 = cx, c2 = sk, c3 = v);
        //     for line in entry.lines() {
        //         println!("{ind} {}", line.cyan());
        //     }
        // }

        Ok(())
    }
}
