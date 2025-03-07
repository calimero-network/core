mod context;

use context::ContextCommand;

#[derive(Debug, Parser)]
pub enum Command {
    #[command(about = "Manage contexts")]
    Context(ContextCommand),
}

impl Command {
    pub async fn run(self, environment: &Environment) -> EyreResult<()> {
        match self {
            Self::Context(cmd) => cmd.run(environment).await,
        }
    }
} 