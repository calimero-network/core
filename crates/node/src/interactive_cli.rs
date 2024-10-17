#![allow(clippy::print_stdout, reason = "Acceptable for CLI")]
#![allow(
    clippy::multiple_inherent_impl,
    reason = "TODO: Check if this is necessary"
)]

mod applications;
pub mod call;
pub mod context;
pub mod gc;
pub mod identity;
pub mod peers;
pub mod pool;
pub mod state;
pub mod store;
pub mod transactions;

use clap::{Parser, Subcommand};
use eyre::Report;

use crate::Node;
#[derive(Debug, Parser)]
pub struct RootCommand {
    #[command(subcommand)]
    pub action: SubCommands,
}

#[derive(Debug, Subcommand)]
pub enum SubCommands {
    Application(applications::ApplicationCommand),
    Call(call::CallCommand),
    Context(context::ContextCommand),
    Gc(gc::GarbageCollectCommand),
    Identity(identity::IdentityCommand),
    Peers(peers::PeersCommand),
    Pool(pool::PoolCommand),
    Store(store::StoreCommand),
    State(state::StateCommand),
    Transactions(transactions::TransactionsCommand),
}

pub async fn handle_line(node: &mut Node, line: String) -> Result<(), Report> {
    // IMPORTANT: Parser needs first string to be binary name
    let mut args = vec![""];
    args.extend(line.split_whitespace());
    println!("args: {:?}", args);

    match RootCommand::try_parse_from(args) {
        Ok(command) => match command.action {
            SubCommands::Application(application) => {
                if let Err(e) = application.run(node).await {
                    println!("Error running application command: {}", e);
                }
            }
            SubCommands::Call(call) => {
                if let Err(e) = call.run(node).await {
                    println!("Error running call command: {}", e);
                }
            }
            SubCommands::Context(context) => {
                if let Err(e) = context.run(node).await {
                    println!("Error running context command: {}", e);
                }
            }
            SubCommands::Gc(gc) => {
                if let Err(e) = gc.run(node).await {
                    println!("Error running gc command: {}", e);
                }
            }
            SubCommands::Identity(identity) => {
                if let Err(e) = identity.run(node).await {
                    println!("Error running identity command: {}", e);
                }
            }
            SubCommands::Peers(peers) => {
                if let Err(e) = peers.run(node.network_client.clone().into()).await {
                    println!("Error running peers command: {}", e);
                }
            }
            SubCommands::Pool(pool) => {
                if let Err(e) = pool.run(node).await {
                    println!("Error running pool command: {}", e);
                }
            }
            SubCommands::State(state) => {
                if let Err(e) = state.run(node).await {
                    println!("Error running state command: {}", e);
                }
            }
            SubCommands::Store(store) => {
                if let Err(e) = store.run(node).await {
                    println!("Error running store command: {}", e);
                }
            }
            SubCommands::Transactions(transactions) => {
                if let Err(e) = transactions.run(node).await {
                    println!("Error running transactions command: {}", e);
                }
            }
        },
        Err(err) => {
            println!("Failed to parse command: {}", err);
            return Ok(());
        }
    };

    Ok(())
}
