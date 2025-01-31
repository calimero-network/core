use core::str::FromStr;

use calimero_primitives::context::ContextId;
use calimero_store::key::ContextIdentity as ContextIdentityKey;
use clap::{Parser, Subcommand};
use eyre::Result as EyreResult;
use owo_colors::OwoColorize;

use crate::Node;

/// Manage identities
#[derive(Debug, Parser)]
pub struct IdentityCommand {
    #[command(subcommand)]
    subcommand: IdentitySubcommands,
}

#[derive(Debug, Subcommand)]
enum IdentitySubcommands {
    /// List identities in a context
    Ls {
        /// The context ID to list identities in
        context_id: String,
    },
    /// Create a new identity
    New,
}

impl IdentityCommand {
    pub fn run(self, node: &Node) -> EyreResult<()> {
        let ind = ">>".blue();

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

                        println!("{ind} {:44} | Owned", "Identity");

                        for (k, v) in first.into_iter().chain(iter.entries()) {
                            let (k, v) = (k?, v?);

                            if k.context_id() != context_id {
                                break;
                            }

                            let entry = format!(
                                "{:44} | {}",
                                k.public_key(),
                                if v.private_key.is_some() { "Yes" } else { "No" },
                            );
                            for line in entry.lines() {
                                println!("{ind} {}", line.cyan());
                            }
                        }
                    }
                    Err(_) => {
                        println!("{ind} Invalid context ID: {context_id}");
                    }
                }
            }
            IdentitySubcommands::New => {
                // Handle the "new" subcommand
                let identity = node.ctx_manager.new_private_key();
                println!("{ind} Private Key: {}", identity.cyan());
                println!("{ind} Public Key: {}", identity.public_key().cyan());
            }
        }

        Ok(())
    }
}
