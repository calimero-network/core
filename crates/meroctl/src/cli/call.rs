use calimero_primitives::alias::Alias;
use calimero_primitives::context::ContextId;
use calimero_primitives::identity::PublicKey;
use calimero_server_primitives::jsonrpc::{
    ExecutionRequest, Request, RequestId, RequestPayload, Version,
};
use clap::Parser;
use const_format::concatcp;
use eyre::{OptionExt, Result};
use serde_json::{json, Value};

use crate::cli::validation::non_empty_string;
use crate::cli::Environment;

pub const EXAMPLES: &str = r"
  # Call a mutation (e.g. add_item, set) on a context
  $ meroctl --node <NODE_ID> call <METHOD_NAME> \
    --context <CONTEXT_ID> \
    --args '<ARGS_JSON>' \
    --as <IDENTITY_PUBLIC_KEY>

  # Call a view (e.g. get_item, get) on a context
  $ meroctl --node <NODE_ID> call <METHOD_NAME> \
    --context <CONTEXT_ID> \
    --args '<ARGS_JSON>' \
    --as <IDENTITY_PUBLIC_KEY>
";

#[derive(Debug, Parser)]
#[command(about = "Call a method on a context")]
#[command(after_help = concatcp!(
    "Examples:",
    EXAMPLES
))]
pub struct CallCommand {
    #[arg(long, short)]
    #[arg(
        value_name = "CONTEXT",
        help = "The context to call the method on",
        default_value = "default"
    )]
    pub context: Alias<ContextId>,

    #[arg(value_name = "METHOD", help = "The method to call", value_parser = non_empty_string)]
    pub method: String,

    #[arg(long, value_parser = serde_value, help = "JSON arguments to pass to the method")]
    pub args: Option<Value>,

    #[arg(
        long = "as",
        help = "The identity of the executor",
        default_value = "default"
    )]
    pub executor: Alias<PublicKey>,

    #[arg(long, help = "Id of the JsonRpc call")]
    pub id: Option<String>,

    #[arg(
        long = "substitute",
        help = "Comma-separated list of aliases to substitute in the payload (use {alias} in payload)",
        value_name = "ALIAS",
        value_delimiter = ','
    )]
    pub substitute: Vec<Alias<PublicKey>>,
}

fn serde_value(s: &str) -> serde_json::Result<Value> {
    serde_json::from_str(s)
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn test_call_command_parsing_minimal() {
        let cmd = CallCommand::try_parse_from(["call", "my_method"]).unwrap();

        assert_eq!(cmd.method, "my_method");
        assert!(cmd.args.is_none());
        assert!(cmd.id.is_none());
        assert!(cmd.substitute.is_empty());
    }

    #[test]
    fn test_call_command_parsing_with_json_args() {
        let cmd =
            CallCommand::try_parse_from(["call", "set_value", "--args", r#"{"key": "value"}"#])
                .unwrap();

        assert_eq!(cmd.method, "set_value");
        assert!(cmd.args.is_some());
        let args = cmd.args.unwrap();
        assert_eq!(args["key"], "value");
    }

    #[test]
    fn test_call_command_parsing_with_complex_json_args() {
        let cmd = CallCommand::try_parse_from([
            "call",
            "complex_method",
            "--args",
            r#"{"nested": {"array": [1, 2, 3]}, "bool": true}"#,
        ])
        .unwrap();

        assert_eq!(cmd.method, "complex_method");
        let args = cmd.args.unwrap();
        assert_eq!(args["nested"]["array"][0], 1);
        assert_eq!(args["bool"], true);
    }

    #[test]
    fn test_call_command_parsing_with_id() {
        let cmd =
            CallCommand::try_parse_from(["call", "my_method", "--id", "request-123"]).unwrap();

        assert_eq!(cmd.method, "my_method");
        assert_eq!(cmd.id, Some("request-123".to_string()));
    }

    #[test]
    fn test_call_command_parsing_short_context_flag() {
        let cmd =
            CallCommand::try_parse_from(["call", "my_method", "-c", "my_context_alias"]).unwrap();

        assert_eq!(cmd.method, "my_method");
        assert_eq!(cmd.context.as_str(), "my_context_alias");
    }

    #[test]
    fn test_call_command_missing_method_fails() {
        let result = CallCommand::try_parse_from(["call"]);
        assert!(
            result.is_err(),
            "Command should fail when method is missing"
        );
    }

    #[test]
    fn test_call_command_invalid_json_args_fails() {
        let result = CallCommand::try_parse_from(["call", "my_method", "--args", "not-valid-json"]);
        assert!(
            result.is_err(),
            "Command should fail with invalid JSON args"
        );
    }

    #[test]
    fn test_call_command_empty_method_fails() {
        let result = CallCommand::try_parse_from(["call", ""]);
        assert!(
            result.is_err(),
            "Command should fail with empty method name"
        );
    }

    #[test]
    fn test_serde_value_parser_valid_json() {
        let result = serde_value(r#"{"key": "value"}"#);
        assert!(result.is_ok());
        assert_eq!(result.unwrap()["key"], "value");
    }

    #[test]
    fn test_serde_value_parser_invalid_json() {
        let result = serde_value("not json");
        assert!(result.is_err());
    }

    #[test]
    fn test_serde_value_parser_array() {
        let result = serde_value("[1, 2, 3]");
        assert!(result.is_ok());
        let arr = result.unwrap();
        assert_eq!(arr[0], 1);
    }

    #[test]
    fn test_serde_value_parser_primitive() {
        assert!(serde_value("123").is_ok());
        assert!(serde_value("true").is_ok());
        assert!(serde_value(r#""string""#).is_ok());
        assert!(serde_value("null").is_ok());
    }
}

impl CallCommand {
    pub async fn run(self, environment: &mut Environment) -> Result<()> {
        let client = environment.client()?;

        let resolve_response = client.resolve_alias(self.context, None).await?;
        let context_id = resolve_response
            .value()
            .cloned()
            .ok_or_eyre("Failed to resolve context: no value found")?;

        let executor = client
            .resolve_alias(self.executor, Some(context_id))
            .await?
            .value()
            .cloned()
            .ok_or_eyre("unable to resolve")?;

        let payload = RequestPayload::Execute(ExecutionRequest::new(
            context_id,
            self.method,
            self.args.unwrap_or(json!({})),
            executor,
            self.substitute,
        ));

        let request = Request::new(
            Version::TwoPointZero,
            self.id.map(RequestId::String).unwrap_or_default(),
            payload,
        );

        let response = client.execute_jsonrpc(request).await?;

        // Debug: Print what we're about to output
        eprintln!(
            "🔍 meroctl call output: {}",
            serde_json::to_string_pretty(&response)?
        );

        environment.output.write(&response);

        Ok(())
    }
}
