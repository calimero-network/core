use eyre::{self, bail};
use serde_json::Value;

use crate::driver::TestContext;

/// Recursively processes a JSON value, replacing variable references
/// like {variable_name} with corresponding values from the TestContext.
pub fn process_json_variables(value: &mut Value, ctx: &TestContext<'_>) -> eyre::Result<()> {
    match value {
        Value::String(s) => {
            // Check if the string looks like a variable reference {variable_name}
            if s.starts_with("${") && s.ends_with("}") {
                // Extract the variable name without braces
                let var_name = s.trim_start_matches("${").trim_end_matches("}");

                // Check if the context has this field and replace it
                let replacement = match var_name {
                    "proposal_id" => ctx.proposal_id.clone(),
                    // Add other fields as needed
                    _ => None,
                };

                // If we found a replacement, use it
                if let Some(new_value) = replacement {
                    *s = new_value;
                } else {
                    bail!("Variable '{}' not found in context", var_name);
                }
            }
        }
        Value::Object(obj) => {
            for (_, v) in obj {
                process_json_variables(v, ctx)?;
            }
        }
        Value::Array(arr) => {
            for item in arr {
                process_json_variables(item, ctx)?;
            }
        }
        _ => {}
    }
    Ok(())
}
