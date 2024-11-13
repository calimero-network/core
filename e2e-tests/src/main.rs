use camino::Utf8PathBuf;
use clap::Parser;
use config::Config;
use const_format::concatcp;
use driver::Driver;
use eyre::Result as EyreResult;
use rand::Rng;
use tokio::fs::{create_dir_all, read_to_string, remove_dir_all};

mod config;
mod driver;
mod meroctl;
mod merod;
mod steps;

pub const EXAMPLES: &str = r"
  # Run from the repository root with debug binaries
  $ e2e-tests --input-dir ./e2e-tests/config
    --output-dir ./e2e-tests/corpus
    --merod-binary ./target/debug/merod
    --meroctl-binary ./target/debug/meroctl
";

#[derive(Debug, Parser)]
#[command(author, version, about, long_about = None)]
#[command(after_help = concatcp!(
    "Examples:",
    EXAMPLES
))]
pub struct Args {
    /// Directory containing the test configuration and test scenarios.
    /// In root directory, there should be a `config.json` file. This file
    /// contains the configuration for the test run. Refer to the `Config`
    /// struct for more information.
    /// Each test case is a directory containing a `test.json` file.
    #[arg(long, value_name = "PATH")]
    #[arg(env = "E2E_INPUT_DIR", hide_env_values = true)]
    pub input_dir: Utf8PathBuf,

    /// Directory to write the test results, logs and node data.
    #[arg(long, value_name = "PATH")]
    #[arg(env = "E2E_OUTPUT_DIR", hide_env_values = true)]
    pub output_dir: Utf8PathBuf,

    /// Path to the merod binary.
    #[arg(long, value_name = "PATH")]
    #[arg(env = "MEROD_BINARY", hide_env_values = true)]
    pub merod_binary: Utf8PathBuf,

    /// Path to the meroctl binary.
    #[arg(long, value_name = "PATH")]
    #[arg(env = "MEROCTL_BINARY", hide_env_values = true)]
    pub meroctl_binary: Utf8PathBuf,
}

#[derive(Debug)]
pub struct TestEnvironment {
    pub test_id: u32,
    pub merod_binary: Utf8PathBuf,
    pub meroctl_binary: Utf8PathBuf,
    pub input_dir: Utf8PathBuf,
    pub output_dir: Utf8PathBuf,
    pub nodes_dir: Utf8PathBuf,
    pub logs_dir: Utf8PathBuf,
}

impl Into<TestEnvironment> for Args {
    fn into(self) -> TestEnvironment {
        let mut rng = rand::thread_rng();

        TestEnvironment {
            test_id: rng.gen::<u32>(),
            merod_binary: self.merod_binary,
            meroctl_binary: self.meroctl_binary,
            input_dir: self.input_dir.clone(),
            output_dir: self.output_dir.clone(),
            nodes_dir: self.output_dir.join("nodes"),
            logs_dir: self.output_dir.join("logs"),
        }
    }
}

impl TestEnvironment {
    pub async fn init(&self) -> EyreResult<()> {
        if self.output_dir.exists() {
            remove_dir_all(&self.output_dir).await?;
        }

        create_dir_all(&self.nodes_dir).await?;
        create_dir_all(&self.logs_dir).await?;

        Ok(())
    }

    pub async fn cleanup(&self) -> EyreResult<()> {
        remove_dir_all(&self.output_dir).await?;
        Ok(())
    }
}

#[tokio::main]
async fn main() -> EyreResult<()> {
    let args = Args::parse();

    let config_path = args.input_dir.join("config.json");
    let config_content = read_to_string(config_path).await?;
    let config: Config = serde_json::from_str(&config_content)?;

    let driver = Driver::new(args.into(), config);

    driver.run().await
}
