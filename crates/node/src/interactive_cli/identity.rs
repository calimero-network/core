use core::str::FromStr;

use calimero_primitives::context::ContextId;
use calimero_store::key::ContextIdentity as ContextIdentityKey;
use clap::{Parser, Subcommand};
use eyre::Result;
use owo_colors::OwoColorize;

use crate::Node;

#[derive(Debug, Parser)]
pub struct IdentityCommand {
    #[command(subcommand)]
    subcommand: IdentitySubcommands,
}

#[derive(Debug, Subcommand)]
enum IdentitySubcommands {
    Ls { context_id: String },
    New,
}

impl IdentityCommand {
    pub fn run(self, node: &Node) -> Result<()> {
        match &self.subcommand {
            IdentitySubcommands::Ls { context_id } => {
                match ContextId::from_str(context_id) {
                    Ok(context_id) => {
                        // Handle the "ls" subcommand
                        let handle = node.store.handle();
                        let mut iter = handle.iter::<ContextIdentityKey>()?;

                        let first = 'first: {
                            let Some(k) = iter
                                .seek(ContextIdentityKey::new(context_id, [0; 32].into()))
                                .transpose()
                            else {
                                break 'first None;
                            };

                            Some((k, iter.read()))
                        };

                        println!("{:44} | Owned", "Identity");

                        for (k, v) in first.into_iter().chain(iter.entries()) {
                            let (k, v) = (k?, v?);

                            if k.context_id() != context_id {
                                break;
                            }

                            let entry = format!(
                                "{:44} | {}",
                                k.public_key(),
                                if v.private_key.is_some() { "*" } else { " " },
                            );
                            for line in entry.lines() {
                                println!("{}", line.cyan());
                            }
                        }
                    }
                    Err(_) => {
                        println!("Invalid context ID: {context_id}");
                    }
                }
            }
            IdentitySubcommands::New => {
                // Handle the "new" subcommand
                let identity = node.ctx_manager.new_private_key();
                println!("Private Key: {}", identity.cyan());
                println!("Public Key: {}", identity.public_key().cyan());
            }
        }

        Ok(())
    }
}
