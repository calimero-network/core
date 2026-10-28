//! `meroctl node`: saved nodes (v1's), so `--node ALIAS` names a cluster.

use clap::Subcommand;
use serde_json::{json, Value};

use crate::env::{Home, SavedNode};

#[derive(Debug, Subcommand)]
pub enum NodeCmd {
    /// Save a node under a name (the first one saved becomes the default).
    Add {
        name: String,
        /// Its URL (any node of the cluster).
        url: String,
        /// A development cluster: trust its mock quote.
        #[arg(long)]
        allow_mock: bool,
    },
    /// Forget a saved node.
    #[command(alias = "rm")]
    Remove { name: String },
    /// Make a saved node the default.
    Use { name: String },
    /// The saved nodes and the default.
    #[command(alias = "ls")]
    List,
}

fn valid(name: &str) -> eyre::Result<()> {
    if name.is_empty()
        || !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        eyre::bail!("a node name is letters, digits, - and _: {name:?}");
    }
    Ok(())
}

pub fn run(cmd: &NodeCmd, home: &Home) -> eyre::Result<Value> {
    let mut nodes = home.nodes();
    match cmd {
        NodeCmd::Add {
            name,
            url,
            allow_mock,
        } => {
            valid(name)?;
            let _ = url::Url::parse(url).map_err(|e| eyre::eyre!("{url}: {e}"))?;
            let _prev = nodes.nodes.insert(
                name.clone(),
                SavedNode {
                    url: url.clone(),
                    allow_mock: *allow_mock,
                },
            );
            if nodes.default.is_none() {
                nodes.default = Some(name.clone());
            }
        }
        NodeCmd::Remove { name } => {
            if nodes.nodes.remove(name).is_none() {
                eyre::bail!("no saved node {name}");
            }
            if nodes.default.as_deref() == Some(name.as_str()) {
                nodes.default = None;
            }
        }
        NodeCmd::Use { name } => {
            if !nodes.nodes.contains_key(name) {
                eyre::bail!("no saved node {name}: meroctl node add {name} URL");
            }
            nodes.default = Some(name.clone());
        }
        NodeCmd::List => {}
    }
    if !matches!(cmd, NodeCmd::List) {
        home.save_nodes(&nodes)?;
    }
    Ok(json!({ "default": nodes.default, "nodes": nodes.nodes }))
}
