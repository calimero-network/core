use crate::TestContext;
use eyre::{bail, Result as EyreResult};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct VerifyExternalStateStep {
    pub contract_id: String,
    pub key: String,
    pub expected_value: String,
    #[serde(default)]
    pub retries: Option<usize>,
    #[serde(default)]
    pub interval_ms: Option<u64>,
    #[serde(default)]
    pub description: Option<Vec<String>>,
}

impl VerifyExternalStateStep {
    pub async fn execute(&self, ctx: &mut TestContext) -> EyreResult<()> {
        let icp_env = ctx.get_icp_environment()?;
        
        let max_retries = self.retries.unwrap_or(1);
        let interval_ms = self.interval_ms.unwrap_or(1000);
        
        for attempt in 0..max_retries {
            match icp_env.check_external_contract_state(&self.contract_id, &self.key)? {
                Some(value) if value == self.expected_value => {
                    ctx.output_writer.write_str(&format!(
                        "External contract state verified: key '{}' has value '{}'",
                        self.key, self.expected_value
                    ));
                    return Ok(());
                }
                Some(value) => {
                    ctx.output_writer.write_str(&format!(
                        "External contract state mismatch: expected '{}', got '{}'",
                        self.expected_value, value
                    ));
                }
                None => {
                    ctx.output_writer.write_str(&format!(
                        "Key '{}' not found in external contract state",
                        self.key
                    ));
                }
            }
            
            if attempt < max_retries - 1 {
                ctx.output_writer.write_str(&format!(
                    "Retrying verification in {} ms (attempt {}/{})",
                    interval_ms, attempt + 1, max_retries
                ));
                
                tokio::time::sleep(tokio::time::Duration::from_millis(interval_ms)).await;
            }
        }
        
        bail!(
            "Failed to verify external contract state after {} attempts: key '{}' should have value '{}'",
            max_retries, self.key, self.expected_value
        )
    }
}
