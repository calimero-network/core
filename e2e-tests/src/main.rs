#![expect(unused_results, reason = "clap has a dangling returned type")]

use camino::Utf8PathBuf;
use clap::{Args, Parser, Subcommand};
use const_format::concatcp;
use eyre::Result as EyreResult;
use rand::Rng;
use tokio::fs::{create_dir_all, read_to_string, remove_dir_all};

mod config;
mod driver;
mod meroctl;
mod merod;
mod output;
mod protocol;
mod steps;
mod utils;

use config::Config;
use driver::{Driver, TestRunReport};
use output::{OutputFormat, OutputWriter};

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
#[clap(args_conflicts_with_subcommands = true)]
pub struct Command {
    #[command(subcommand)]
    commands: Option<Commands>,

    #[command(flatten)]
    args: Option<RootArgs>,
}

#[derive(Debug, Subcommand)]
pub enum Commands {
    Combine {
        /// The directories that contains the test data to be combined.
        #[arg(value_name = "PATH", num_args=1.., required = true)]
        dirs: Vec<Utf8PathBuf>,

        /// Directory to write the combined test results.
        #[arg(long, value_name = "PATH")]
        #[arg(env = "E2E_OUTPUT_DIR", hide_env_values = true)]
        output_dir: Utf8PathBuf,
    },
}

#[derive(Debug, Args)]
#[command(author, version, about, long_about = None)]
#[command(after_help = concatcp!(
    "Examples:",
    EXAMPLES
))]
pub struct RootArgs {
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

    /// Format of the E2E test output.
    #[arg(long, value_name = "OUTPUT_FORMAT", default_value_t, value_enum)]
    #[arg(env = "E2E_OUTPUT_FORMAT", hide_env_values = true)]
    pub output_format: OutputFormat,

    /// Scenario to run ("ethereum", "near", "stellar", "icp")
    #[arg(long, value_name = "SCENARIO")]
    #[arg(value_parser = parse_scenario)]
    pub scenario: String,
}

fn parse_scenario(s: &str) -> Result<String, String> {
    match s {
        "ethereum" | "near" | "stellar" | "icp" => Ok(s.to_string()),
        _ => Err(format!(
            "Invalid scenario. Must be one of: ethereum, near, stellar, icp. Got: {}", 
            s
        ))
    }
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
    pub icp_dir: Utf8PathBuf,
    pub output_writer: OutputWriter,
    pub scenario: String,
}

impl From<RootArgs> for TestEnvironment {
    fn from(val: RootArgs) -> Self {
        let mut rng = rand::thread_rng();

        Self {
            test_id: rng.gen::<u32>(),
            merod_binary: val.merod_binary,
            meroctl_binary: val.meroctl_binary,
            input_dir: val.input_dir.clone(),
            output_dir: val.output_dir.clone(),
            nodes_dir: val.output_dir.join("nodes"),
            logs_dir: val.output_dir.join("logs"),
            icp_dir: val.output_dir.join("icp"),
            output_writer: OutputWriter::new(val.output_format),
            scenario: val.scenario,
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
    let args = Command::parse();

    if let Some(args) = args.args {
        let config_path = args.input_dir.join("config.json");
        let config_content = read_to_string(config_path).await?;
        let config: Config = serde_json::from_str(&config_content)?;

        let driver = Driver::new(args.into(), config);

        driver.run().await?;
    }

    if let Some(Commands::Combine { dirs, output_dir }) = args.commands {
        let mut dirs = dirs.into_iter();

        let first = dirs.next().expect("first dir should be present");

        let mut report = TestRunReport::from_dir(&first).await?;

        for dir in dirs {
            let other = TestRunReport::from_dir(&dir).await?;

            report.merge(other).await;
        }

        let writer = OutputWriter::new(OutputFormat::PlainText);

        report.store_to_file(&output_dir, &writer).await?;
    }

    Ok(())
}
