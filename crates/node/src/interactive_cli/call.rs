use calimero_primitives::context::ContextId;
use calimero_primitives::identity::PublicKey;
use clap::Parser;
use owo_colors::OwoColorize;
use serde_json::Value;

use crate::Node;

#[derive(Debug, Parser)]
pub struct CallCommand {
    context_id: ContextId,
    method: String,
    payload: Value,
    executor_key: PublicKey,
}

impl CallCommand {
    pub async fn run(self, node: &mut Node) -> eyre::Result<()> {
        let ind = ">>".blue();

        let Ok(Some(context)) = node.ctx_manager.get_context(&self.context_id) else {
            println!("{} context not found: {}", ind, self.context_id);
            return Ok(());
        };

        let outcome_result = node
            .handle_call(
                context.id,
                &self.method,
                serde_json::to_vec(&self.payload)?,
                self.executor_key,
            )
            .await;

        match outcome_result {
            Ok(outcome) => {
                match outcome.returns {
                    Ok(result) => match result {
                        Some(result) => {
                            println!("{ind}   return value:");
                            #[expect(clippy::option_if_let_else, reason = "clearer here")]
                            let result = if let Ok(value) = serde_json::from_slice::<Value>(&result)
                            {
                                format!(
                                    "(json): {}",
                                    format!("{value:#}")
                                        .lines()
                                        .map(|line| line.cyan().to_string())
                                        .collect::<Vec<_>>()
                                        .join("\n")
                                )
                            } else {
                                format!("(raw): {:?}", result.cyan())
                            };

                            for line in result.lines() {
                                println!("{ind}     > {line}");
                            }
                        }
                        None => println!("{ind}   (no return value)"),
                    },
                    Err(err) => {
                        let err = format!("{err:#?}");

                        println!("{ind}   error:");
                        for line in err.lines() {
                            println!("{ind}     > {}", line.yellow());
                        }
                    }
                }

                if !outcome.logs.is_empty() {
                    println!("{ind}   logs:");

                    for log in outcome.logs {
                        println!("{ind}     > {}", log.cyan());
                    }
                }
            }
            Err(err) => {
                let err = format!("{err:#?}");

                println!("{ind}   error:");
                for line in err.lines() {
                    println!("{ind}     > {}", line.yellow());
                }
            }
        }

        Ok(())
    }
}
