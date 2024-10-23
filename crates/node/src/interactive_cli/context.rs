use core::str::FromStr;
use std::mem::replace;

use calimero_primitives::hash::Hash;
use calimero_store::key::ContextMeta as ContextMetaKey;
use clap::{Parser, Subcommand};
use eyre::Result;
use owo_colors::OwoColorize;

use crate::Node;

#[derive(Debug, Parser)]
pub struct ContextCommand {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
    Ls,
    Join {
        private_key: String,
        invitation_payload: String,
    },
    Leave {
        context_id: String,
    },
    Create {
        application_id: String,
        context_seed: Option<String>,
        params: Option<String>,
    },
    Invite {
        context_id: String,
        inviter_id: String,
        invitee_id: String,
    },
    Delete {
        context_id: String,
    },
}

impl ContextCommand {
    pub async fn run(self, node: &Node) -> Result<()> {
        let ind = ">>".blue();

        match self.command {
            Commands::Ls => {
                let ind = ""; // Define the variable `ind` as an empty string or any desired value

                println!(
                    "{ind} {c1:44} | {c2:44} | Last Transaction",
                    c1 = "Context ID",
                    c2 = "Application ID",
                );

                let handle = node.store.handle();

                for (k, v) in handle.iter::<ContextMetaKey>()?.entries() {
                    let (k, v) = (k?, v?);
                    let (cx, app_id, last_tx) =
                        (k.context_id(), v.application.application_id(), v.root_hash);
                    let entry = format!(
                        "{c1:44} | {c2:44} | {c3}",
                        c1 = cx,
                        c2 = app_id,
                        c3 = Hash::from(last_tx)
                    );
                    for line in entry.lines() {
                        println!("{ind} {}", line.cyan());
                    }
                }
            }
            Commands::Join {
                private_key,
                invitation_payload,
            } => {
                let private_key = private_key.parse()?;
                let invitation_payload = invitation_payload.parse()?;

                let response = node
                    .ctx_manager
                    .join_context(private_key, invitation_payload)
                    .await?;

                if let Some((context_id, identity)) = response {
                    println!(
                        "{ind} Joined context {context_id} as {identity}, waiting for catchup to complete..."
                    );
                } else {
                    println!(
                        "{ind} Unable to join context at this time, a catchup is in progress."
                    );
                }
            }
            Commands::Leave { context_id } => {
                let context_id = context_id.parse()?;
                if node.ctx_manager.delete_context(&context_id).await? {
                    println!("{ind} Successfully deleted context {context_id}");
                } else {
                    println!("{ind} Failed to delete context {context_id}");
                }
                println!("{ind} Left context {context_id}");
            }
            Commands::Create {
                application_id,
                context_seed,
                mut params,
            } => {
                let application_id = application_id.parse()?;

                let (context_seed, params) = 'infer: {
                    let Some(context_seed) = context_seed.clone() else {
                        break 'infer (None, None);
                    };
                    let context_seed_clone = context_seed.clone();

                    if let Ok(context_seed) = context_seed.parse::<Hash>() {
                        break 'infer (Some(context_seed), params);
                    };

                    match replace(&mut params, Some(context_seed))
                        .map(|arg0| FromStr::from_str(&arg0))
                    {
                        Some(Ok(context_seed)) => break 'infer (Some(context_seed), params),
                        None => break 'infer (None, params),
                        _ => {}
                    };
                    println!("{ind} Invalid context seed: {}", context_seed_clone);
                    return Err(eyre::eyre!("Invalid context seed"));
                };

                let (context_id, identity) = node
                    .ctx_manager
                    .create_context(
                        context_seed.map(Into::into),
                        application_id,
                        None,
                        params.unwrap_or_default().into_bytes(),
                    )
                    .await?;

                println!("{ind} Created context {context_id} with identity {identity}");
            }
            Commands::Invite {
                context_id,
                inviter_id,
                invitee_id,
            } => {
                let context_id = context_id.parse()?;
                let inviter_id = inviter_id.parse()?;
                let invitee_id = invitee_id.parse()?;

                if let Some(invitation_payload) = node
                    .ctx_manager
                    .invite_to_context(context_id, inviter_id, invitee_id)
                    .await?
                {
                    println!("{ind} Invited {invitee_id} to context {context_id}");
                    println!("{ind} Invitation Payload: {invitation_payload}");
                } else {
                    println!("{ind} Unable to invite {invitee_id} to context {context_id}");
                }
            }
            Commands::Delete { context_id } => {
                let context_id = context_id.parse()?;
                let _ = node.ctx_manager.delete_context(&context_id).await?;
                println!("{ind} Deleted context {context_id}");
            }
        }
        Ok(())
    }
}
