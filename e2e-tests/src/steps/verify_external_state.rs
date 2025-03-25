use eyre::{bail, Result as EyreResult};
use serde::{Deserialize, Serialize};

use crate::driver::{Test, TestContext};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VerifyExternalStateStep {
    pub contract_id: String,
    pub method_name: String,
    pub args: Vec<String>,
    pub expected_value: String,
    #[serde(default)]
    pub retries: Option<usize>,
    #[serde(default)]
    pub interval_ms: Option<u64>,
    #[serde(default)]
    pub description: Option<Vec<String>>,
}

impl Test for VerifyExternalStateStep {
    fn display_name(&self) -> String {
        "verify external state".to_owned()
    }

    async fn run_assert(&self, ctx: &mut TestContext<'_>) -> EyreResult<()> {
        ctx.output_writer.write_str(&format!(
            "Verifying external contract state in {} protocol for contract: {}",
            ctx.protocol_name, self.contract_id
        ));

        let max_retries = self.retries.unwrap_or(1);
        let interval_ms = self.interval_ms.unwrap_or(1000);

        for attempt in 0..max_retries {
            match ctx
                .protocol
                .verify_external_contract_state(&self.contract_id, &self.method_name, &self.args)
                .await?
            {
                Some(actual_value) => {
                    // Compare the actual value with expected value
                    if actual_value.contains(&self.expected_value) {
                        ctx.output_writer.write_str(&format!(
                            "External contract state verified: key '{:?}' has value containing '{}'",
                            self.args, self.expected_value
                        ));
                        return Ok(());
                    } else {
                        ctx.output_writer.write_str(&format!(
                            "Value mismatch: expected '{}' but got '{}'",
                            self.expected_value, actual_value
                        ));
                        return Err(eyre!("Value mismatch: expected '{}' but got '{}'", self.expected_value, actual_value));
                    }
                }
                None => {
                    ctx.output_writer.write_str(&format!(
                        "Key '{:?}' not found in external contract state",
                        self.args
                    ));
                    return Err(eyre!("Key '{:?}' not found in external contract state", self.args));
                }
            }

            if attempt < max_retries - 1 {
                ctx.output_writer.write_str(&format!(
                    "Retrying verification in {} ms (attempt {}/{})",
                    interval_ms,
                    attempt + 1,
                    max_retries
                ));

                tokio::time::sleep(tokio::time::Duration::from_millis(interval_ms)).await;
            }
        }

        bail!(
            "Failed to verify external contract state after {} attempts: key '{:?}' should have value '{}'",
            max_retries, self.args, self.expected_value
        )
    }
}
