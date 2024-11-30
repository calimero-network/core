pub mod cli;

#[cfg(test)]
mod tests {
    use cli::RootCommand;

    use super::*;

    #[tokio::test]
    async fn init_node() {
        // if Ok() => test passed
        // else test failed
        let arg = "init-node";
        let command = RootCommand::new(arg);

        println!("Running node...\n");
        let result: Result<(), eyre::Error> = command.run().await;

        assert_eq!(result.is_ok(), true);
    }
}
