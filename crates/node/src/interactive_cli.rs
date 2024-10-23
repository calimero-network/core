#![allow(clippy::print_stdout, reason = "Acceptable for CLI")]
#![allow(
    clippy::multiple_inherent_impl,
    reason = "TODO: Check if this is necessary"
)]

mod applications;
pub mod call;
pub mod context;
pub mod identity;
pub mod peers;
pub mod state;
pub mod store;
pub mod transactions;

use clap::{Parser, Subcommand};

use crate::Node;
#[derive(Debug, Parser)]
#[non_exhaustive]
pub struct RootCommand {
    #[command(subcommand)]
    pub action: SubCommands,
}

#[derive(Debug, Subcommand)]
#[non_exhaustive]
pub enum SubCommands {
    Application(applications::ApplicationCommand),
    Call(call::CallCommand),
    Context(context::ContextCommand),
    Identity(identity::IdentityCommand),
    Peers(peers::PeersCommand),
    Store(store::StoreCommand),
    State(state::StateCommand),
    Transactions(transactions::TransactionsCommand),
}

pub async fn handle_line(node: &mut Node, line: String) -> eyre::Result<()> {
    // IMPORTANT: Parser needs first string to be binary name
    let mut args = vec![""];
    args.extend(line.split_whitespace());

    let command = match RootCommand::try_parse_from(args) {
        Ok(command) => command,
        Err(err) => {
            println!("Failed to parse command: {}", err);
            eyre::bail!("Failed to parse command");
        }
    };

    let result = match command.action {
        SubCommands::Application(application) => application.run(node).await,
        SubCommands::Call(call) => call.run(node).await,
        SubCommands::Context(context) => context.run(node).await,
        SubCommands::Identity(identity) => identity.run(node).await,
        SubCommands::Peers(peers) => peers.run(node.network_client.clone().into()).await,
        SubCommands::State(state) => state.run(node).await,
        SubCommands::Store(store) => store.run(node).await,
        SubCommands::Transactions(transactions) => transactions.run(node).await,
    };

    if let Err(err) = result {
        println!("Error running command: {}", err);
        eyre::bail!("Failed to parse command");
    }

    Ok(())
}
