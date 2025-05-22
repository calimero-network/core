use calimero_context_primitives::client::ContextClient;
use calimero_node_primitives::client::NodeClient;
use calimero_primitives::alias::Alias;
use calimero_primitives::context::ContextId;
use calimero_primitives::identity::PublicKey;
use clap::Parser;
use eyre::OptionExt;
use owo_colors::OwoColorize;
use serde_json::{json, Value};

/// Call a method on a context
#[derive(Debug, Parser)]
pub struct CallCommand {
    /// The context to call the method on
    #[clap(long, short, default_value = "default")]
    context: Alias<ContextId>,
    /// The method to call
    method: String,
    /// JSON arguments to pass to the method
    #[clap(long, value_parser = serde_value)]
    args: Option<Value>,
    /// The identity of the executor
    #[clap(long = "as", default_value = "default")]
    executor: Alias<PublicKey>,
    /// A list of aliases that should be substituted in the method payload.
    #[clap(
        long = "substitute",
        help = "Comma-separated list of aliases to substitute in the payload (use {alias} in payload)",
        value_name = "ALIAS",
        value_delimiter = ','
    )]
    substitutes: Vec<Alias<PublicKey>>,
}

fn serde_value(s: &str) -> serde_json::Result<Value> {
    serde_json::from_str(s)
}

impl CallCommand {
    pub async fn run(
        self,
        node_client: &NodeClient,
        ctx_client: &ContextClient,
    ) -> eyre::Result<()> {
        let ind = ">>".blue();
        let context_id = node_client
            .resolve_alias(self.context, None)?
            .ok_or_eyre("unable to resolve")?;

        let executor = node_client
            .resolve_alias(self.executor, Some(context_id))?
            .ok_or_eyre("unable to resolve")?;

        let Ok(Some(context)) = ctx_client.get_context(&context_id) else {
            println!("{} context not found: {}", ind, context_id);
            return Ok(());
        };

        let outcome_result = ctx_client
            .execute(
                &context.id,
                &executor,
                self.method,
                serde_json::to_vec(&self.args.unwrap_or(json!({})))?,
                self.substitutes,
                None,
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
