#![allow(clippy::print_stdout, reason = "Acceptable for CLI")]
#![allow(
    clippy::multiple_inherent_impl,
    reason = "TODO: Check if this is necessary"
)]

use std::sync::Arc;

use calimero_context_primitives::client::ContextClient;
use calimero_node_primitives::client::NodeClient;
use calimero_store::Store;
use clap::{Parser, Subcommand};

mod applications;
pub mod blob;
pub mod call;
pub mod common;
pub mod context;
pub mod peers;
pub mod state;
pub mod store;
pub mod webui;

use crate::NodeConfig;

#[derive(Debug, Parser)]
#[command(multicall = true)]
#[non_exhaustive]
pub struct RootCommand {
    #[command(subcommand)]
    pub action: SubCommand,
}

#[derive(Debug, Subcommand)]
#[non_exhaustive]
pub enum SubCommand {
    #[command(alias = "app")]
    Application(applications::ApplicationCommand),
    Blob(blob::BlobCommand),
    Call(call::CallCommand),
    Context(context::ContextCommand),
    Peers(peers::PeersCommand),
    // Store(store::StoreCommand),
    State(state::StateCommand),
    #[command(name = "webui")]
    WebUI(webui::WebUICommand),
}

pub async fn handle_line(
    ctx_client: ContextClient,
    node_client: NodeClient,
    datastore: Store,
    config: Arc<NodeConfig>,
    line: String,
) -> eyre::Result<()> {
    // todo! use shell parsing
    // todo! employ clap completions

    let mut args = line.split_whitespace().peekable();

    if args.peek().is_none() {
        return Ok(());
    }

    let command = match RootCommand::try_parse_from(args) {
        Ok(command) => command,
        Err(err) => {
            println!("{err}");
            return Ok(());
        }
    };

    match command.action {
        SubCommand::Application(application) => application.run(&node_client).await?,
        SubCommand::Blob(blob) => blob.run(&node_client).await?,
        SubCommand::Call(call) => call.run(&node_client, &ctx_client).await?,
        SubCommand::Context(context) => context.run(&node_client, &ctx_client).await?,
        SubCommand::Peers(peers) => peers.run(&node_client).await?,
        SubCommand::State(state) => state.run(&node_client, datastore)?,
        SubCommand::WebUI(webui) => webui.run(&config)?,
        // SubCommand::Store(store) => store.run(node)?,
    }

    Ok(())
}
