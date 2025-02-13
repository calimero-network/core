use calimero_primitives::alias::Kind;
use clap::Parser;
use owo_colors::OwoColorize;
use serde_json::{json, Value};

use super::commons::resolve_identifier;
use crate::Node;

/// Call a method on a context
#[derive(Debug, Parser)]
pub struct CallCommand {
    /// The context ID to call the method on
    context_id: String,
    /// The method to call
    method: String,
    /// JSON arguments to pass to the method
    #[clap(long, value_parser = serde_value)]
    args: Option<Value>,
    /// The public key of the executor
    #[clap(long = "as")]
    executor: String,
}

fn serde_value(s: &str) -> serde_json::Result<Value> {
    serde_json::from_str(s)
}

impl CallCommand {
    pub async fn run(self, node: &mut Node) -> eyre::Result<()> {
        let ind = ">>".blue();

        let context_id = resolve_identifier(node, &self.context_id, Kind::Context, None)?.into();

        let executor =
            resolve_identifier(node, &self.executor, Kind::Identity, Some(context_id))?.into();

        let Ok(Some(context)) = node.ctx_manager.get_context(&context_id) else {
            println!("{} context not found: {}", ind, self.context_id);
            return Ok(());
        };

        let outcome_result = node
            .handle_call(
                context.id,
                &self.method,
                serde_json::to_vec(&self.args.unwrap_or(json!({})))?,
                executor,
            )
            .await;

        match outcome_result {
            Ok(outcome) => {
                match outcome.returns {
                    Ok(result) => match result {
                        Some(result) => {
                            println!("{ind} return value:");
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
                                println!("{ind}   > {line}");
                            }
                        }
                        None => println!("{ind} (no return value)"),
                    },
                    Err(err) => {
                        let err = format!("{err:#?}");

                        println!("{ind} error:");
                        for line in err.lines() {
                            println!("{ind}   > {}", line.yellow());
                        }
                    }
                }

                if !outcome.logs.is_empty() {
                    println!("{ind} logs:");

                    for log in outcome.logs {
                        println!("{ind}   > {}", log.cyan());
                    }
                }
            }
            Err(err) => {
                let err = format!("{err:#?}");

                println!("{ind} error:");
                for line in err.lines() {
                    println!("{ind}   > {}", line.yellow());
                }
            }
        }

        Ok(())
    }
}
