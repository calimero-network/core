use eyre::{bail, Result as EyreResult};
use serde::{Deserialize, Serialize};
use tokio::time::{sleep, Duration};

use crate::driver::{Test, TestContext};

/// Step to verify the state of an external contract by checking if a specific method call
/// returns an expected value. Supports retrying the verification multiple times with configurable intervals.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VerifyExternalStateStep {
    /// ID or address of the contract to verify
    pub contract_id: String,
    /// Name of the method to call on the contract
    pub method_name: String,
    /// Arguments to pass to the method call
    pub args: Vec<String>,
    /// Expected value that should be contained in the method's response
    pub expected_value: String,
    /// Number of times to retry the verification if it fails
    /// Defaults to 1 (no retries) if not specified
    #[serde(default)]
    pub retries: Option<usize>,
    /// Milliseconds to wait between retries
    /// Defaults to 1000ms if not specified
    #[serde(default)]
    pub interval_ms: Option<u64>,
    /// Optional description of what this verification step is checking
    #[serde(default)]
    pub description: Option<Vec<String>>,
}

impl Test for VerifyExternalStateStep {
    fn display_name(&self) -> String {
        "verify external state".to_owned()
    }

    /// Executes the verification step by calling the specified method on the external contract
    /// and comparing the result with the expected value.
    ///
    /// # Process
    /// 1. Calls the specified method on the contract
    /// 2. Checks if the returned value contains the expected value
    /// 3. If verification fails, retries the specified number of times with configured intervals
    /// 4. Logs the progress and results of each attempt
    ///
    /// # Errors
    /// * If verification fails after all retry attempts
    /// * If the contract call itself fails
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
                    }
                }
                None => {
                    ctx.output_writer.write_str(&format!(
                        "Key '{:?}' not found in external contract state",
                        self.args
                    ));
                }
            }

            if attempt < max_retries - 1 {
                ctx.output_writer.write_str(&format!(
                    "Retrying verification in {} ms (attempt {}/{})",
                    interval_ms,
                    attempt + 1,
                    max_retries
                ));

                sleep(Duration::from_millis(interval_ms)).await;
            }
        }

        bail!(
            "Failed to verify external contract state after {} attempts: key '{:?}' should have value '{}'",
            max_retries, self.args, self.expected_value
        )
    }
}
