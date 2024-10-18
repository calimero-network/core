use calimero_primitives::{identity::PublicKey, transaction::Transaction};
use clap::Parser;
use owo_colors::OwoColorize;
use serde_json::Value;
use tokio::sync::oneshot;

use crate::Node;
use eyre::Result;

#[derive(Debug, Parser)]
pub struct CallCommand {
    context_id: String,
    method: String,
    payload: Value,
    executor_key: PublicKey,
}

impl CallCommand {
    pub async fn run(self, node: &mut Node) -> Result<()> {
        let ind = ">>".blue();
        let (outcome_sender, outcome_receiver) = oneshot::channel();

        let Ok(context_id) = self.context_id.parse() else {
            println!("{} invalid context id: {}", ind, self.context_id);
            return Ok(());
        };

        let Ok(Some(context)) = node.ctx_manager.get_context(&context_id) else {
            println!("{} context not found: {}", ind, context_id);
            return Ok(());
        };

        let tx = Transaction::new(
            context.id,
            self.method.to_owned(),
            serde_json::to_string(&self.payload)?.into_bytes(),
            context.last_transaction_hash,
            self.executor_key,
        );

        let tx_hash = match node.call_mutate(&context, tx, outcome_sender).await {
            Ok(tx_hash) => tx_hash,
            Err(e) => {
                println!("{} failed to execute transaction: {:?}", ind, e);
                return Ok(());
            }
        };

        println!("{} scheduled transaction! {:?}", ind, tx_hash);

        drop(tokio::spawn(async move {
            if let Ok(outcome_result) = outcome_receiver.await {
                println!("{} {:?}", ind, tx_hash);

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
