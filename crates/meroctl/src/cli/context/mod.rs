use clap::{Parser, Subcommand};
use reqwest::Url;

use crate::cli::RootArgs;

mod create;
mod ls;
mod members;
mod mutate;
mod query;

#[derive(Debug, Parser)]
pub struct ContextCommand {
    #[command(subcommand)]
    pub subcommand: ContextSubCommands,
}

#[derive(Debug, Subcommand)]
pub enum ContextSubCommands {
    Ls(ls::LsCommand),
    Create(create::CreateCommand),
    Query(query::QueryCommand),
    Mutate(mutate::MutateCommand),
    Members(members::MembersCommand),
}

impl ContextCommand {
    pub async fn run(self, args: RootArgs) -> eyre::Result<()> {
        match self.subcommand {
            ContextSubCommands::Ls(ls) => ls.run(args).await,
            ContextSubCommands::Create(create) => create.run(args).await,
            ContextSubCommands::Query(query) => query.run(args).await,
            ContextSubCommands::Mutate(mutate) => mutate.run(args).await,
            ContextSubCommands::Members(members) => members.run(args).await,
        }
    }
}

pub(crate) fn get_ip(multiaddr: &libp2p::Multiaddr) -> eyre::Result<reqwest::Url> {
    let ip = multiaddr
        .iter()
        .find_map(|p| match p {
            libp2p::multiaddr::Protocol::Ip4(ip) => Some(ip),
            _ => None,
        })
        .ok_or_else(|| eyre::eyre!("No IP address found in Multiaddr"))?;

    let port = multiaddr
        .iter()
        .find_map(|p| match p {
            libp2p::multiaddr::Protocol::Tcp(port) => Some(port),
            _ => None,
        })
        .ok_or_else(|| eyre::eyre!("No TCP port found in Multiaddr"))?;
    let url = Url::parse(&format!("http://{}:{}", ip, port))?;
    return Ok(url);
}
