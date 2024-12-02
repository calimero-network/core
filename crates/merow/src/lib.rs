pub mod cli;
use dotenvy::dotenv;
use std::env;

pub fn read_env() -> String {
    if let Some(path) = dotenv().ok() {
        println!("Loaded .env file from: {:?}", path);
    } else {
        println!(".env file not found, proceeding without it.");
    }

    let config_file_path = env::var("CONFIG_FILE_PATH").expect("CONFIG_FILE_PATH not set");
    return config_file_path;
}

#[cfg(test)]
mod tests {
    use cli::RootCommand;

    use super::*;

    #[tokio::test]
    #[ignore]
    async fn init_node() {
        // if Ok() => test passed
        // else test failed
        let arg = "init-node";
        let command = RootCommand::new(arg);

        println!("Running node...\n");
        let result: Result<(), eyre::Error> = command.run().await;

        assert_eq!(result.is_ok(), true);
    }

    #[test]
    fn read_env_test() {
        let result = read_env();
        assert_eq!(result, "crates/merow/config/default.toml");
    }
}
