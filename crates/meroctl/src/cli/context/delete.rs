use calimero_primitives::alias::Alias;
use calimero_primitives::context::ContextId;
use clap::Parser;
use eyre::{OptionExt, Result};

use crate::cli::Environment;

#[derive(Copy, Clone, Debug, Parser)]
#[command(about = "Delete a context")]
pub struct DeleteCommand {
    #[clap(name = "CONTEXT", help = "The context to delete")]
    pub context: Alias<ContextId>,
}

impl DeleteCommand {
    pub async fn run(self, environment: &mut Environment) -> Result<()> {
        let client = environment.client()?;

        let context_id = client
            .resolve_alias(self.context, None)
            .await?
            .value()
            .copied()
            .ok_or_eyre("unable to resolve")?;

        let response = client.delete_context(&context_id).await?;

        environment.output.write(&response);

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn test_delete_command_parsing_with_context_id() {
        let context_id = ContextId::from([42u8; 32]);

        let cmd = DeleteCommand::try_parse_from(["delete", &context_id.to_string()]).unwrap();

        // The context field is an Alias, which can be either a raw ID or an alias name
        assert!(!cmd.context.as_str().is_empty());
    }

    #[test]
    fn test_delete_command_parsing_with_alias() {
        let cmd = DeleteCommand::try_parse_from(["delete", "my-context-alias"]).unwrap();

        assert_eq!(cmd.context.as_str(), "my-context-alias");
    }

    #[test]
    fn test_delete_command_missing_context_fails() {
        let result = DeleteCommand::try_parse_from(["delete"]);
        assert!(
            result.is_err(),
            "Command should fail when context is missing"
        );
    }
}
