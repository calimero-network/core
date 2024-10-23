use calimero_node_primitives::ExecutionRequest;
use calimero_primitives::context::ContextId;
use calimero_primitives::identity::PublicKey;
use clap::Parser;
use owo_colors::OwoColorize;
use serde_json::Value;
use tokio::sync::oneshot;

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
        let (outcome_sender, outcome_receiver) = oneshot::channel();

        let Ok(Some(context)) = node.ctx_manager.get_context(&self.context_id) else {
            println!("{} context not found: {}", ind, self.context_id);
            return Ok(());
        };

        node.handle_call(ExecutionRequest::new(
            context.id,
            self.method.to_owned(),
            serde_json::to_vec(&self.payload)?,
            self.executor_key,
            outcome_sender,
            None,
        ))
        .await;

        drop(tokio::spawn(async move {
            if let Ok(outcome_result) = outcome_receiver.await {
                println!("{}", ind);

                match outcome_result {
                    Ok(outcome) => {
                        match outcome.returns {
                            Ok(result) => match result {
                                Some(result) => {
                                    println!("{}   return value:", ind);
                                    #[expect(clippy::option_if_let_else, reason = "clearer here")]
                                    let result = if let Ok(value) =
                                        serde_json::from_slice::<Value>(&result)
                                    {
                                        format!(
                                            "(json): {}",
                                            format!("{:#}", value)
                                                .lines()
                                                .map(|line| line.cyan().to_string())
                                                .collect::<Vec<_>>()
                                                .join("\n")
                                        )
                                    } else {
                                        format!("(raw): {:?}", result.cyan())
                                    };

                                    for line in result.lines() {
                                        println!("{}     > {}", ind, line);
                                    }
                                }
                                None => println!("{}   (no return value)", ind),
                            },
                            Err(err) => {
                                let err = format!("{:#?}", err);

                                println!("{}   error:", ind);
                                for line in err.lines() {
                                    println!("{}     > {}", ind, line.yellow());
                                }
                            }
                        }

                        if !outcome.logs.is_empty() {
                            println!("{}   logs:", ind);

                            for log in outcome.logs {
                                println!("{}     > {}", ind, log.cyan());
                            }
                        }
                    }
                    Err(err) => {
                        let err = format!("{:#?}", err);

                        println!("{}   error:", ind);
                        for line in err.lines() {
                            println!("{}     > {}", ind, line.yellow());
                        }
                    }
                }
            }
        }));

        Ok(())
    }
}
