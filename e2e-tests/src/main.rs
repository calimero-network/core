use camino::Utf8PathBuf;
use clap::Parser;
use config::Config;
use const_format::concatcp;
use driver::Driver;
use eyre::Result as EyreResult;
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
    pub merod_binary: Utf8PathBuf,
    pub meroctl_binary: Utf8PathBuf,
    pub input_dir: Utf8PathBuf,
    pub output_dir: Utf8PathBuf,
    pub nodes_dir: Utf8PathBuf,
    pub logs_dir: Utf8PathBuf,
}

impl Into<TestEnvironment> for Args {
    fn into(self) -> TestEnvironment {
        TestEnvironment {
            merod_binary: self.merod_binary,
            meroctl_binary: self.meroctl_binary,
            input_dir: self.input_dir.clone(),
            output_dir: self.output_dir.clone(),
            nodes_dir: self.output_dir.join("calimero"),
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

    let controller = Driver::new(args.into(), config);

    controller.run().await
}

// async fn initialize_nodes(calimero_home: &str, merod: &str) {
//     fs::create_dir_all(format!("{}/node1", calimero_home)).unwrap();
//     Command::new(merod)
//         .args(&[
//             "--node-name",
//             "node1",
//             "init",
//             "--swarm-port",
//             "2427",
//             "--server-port",
//             "2527",
//         ])
//         .spawn()
//         .expect("Failed to initialize node1");

//     fs::create_dir_all(format!("{}/node2", calimero_home)).unwrap();
//     Command::new(merod)
//         .args(&[
//             "--node-name",
//             "node2",
//             "init",
//             "--swarm-port",
//             "2428",
//             "--server-port",
//             "2528",
//         ])
//         .spawn()
//         .expect("Failed to initialize node2");
// }

// async fn start_node(merod: &str, node_name: &str, pid_file: &str) -> Child {
//     let child = Command::new(merod)
//         .args(&["--node-name", node_name, "run"])
//         .spawn()
//         .expect("Failed to start node");
//     fs::write(pid_file, format!("{}", child.id())).unwrap();
//     child
// }

// async fn start_nodes(merod: &str, corpus_dir: &str) -> (Child, Child) {
//     let node1 = start_node(merod, "node1", &format!("{}/pid-node1", corpus_dir)).await;
//     sleep(Duration::from_secs(5)).await; // Wait for the node to start
//     let node2 = start_node(merod, "node2", &format!("{}/pid-node2", corpus_dir)).await;
//     sleep(Duration::from_secs(5)).await; // Wait for the node to start
//     (node1, node2)
// }

// async fn invite_join(meroctl: &str, app_path: &str) {
//     let application = Command::new(meroctl)
//         .args(&[
//             "--node-name",
//             "node1",
//             "--output-format",
//             "json",
//             "app",
//             "install",
//             "--path",
//             app_path,
//         ])
//         .output()
//         .expect("Failed to install app");
//     let application: serde_json::Value = serde_json::from_slice(&application.stdout).unwrap();
//     let application_id = application["data"]["applicationId"].as_str().unwrap();

//     let context = Command::new(meroctl)
//         .args(&[
//             "--node-name",
//             "node1",
//             "--output-format",
//             "json",
//             "context",
//             "create",
//             "-a",
//             application_id,
//         ])
//         .output()
//         .expect("Failed to create context");
//     let context: serde_json::Value = serde_json::from_slice(&context.stdout).unwrap();
//     let context_id = context["data"]["contextId"].as_str().unwrap();
//     let inviteer_public_key = context["data"]["memberPublicKey"].as_str().unwrap();

//     let invitee_identity = Command::new(meroctl)
//         .args(&[
//             "--node-name",
//             "node2",
//             "--output-format",
//             "json",
//             "identity",
//             "generate",
//         ])
//         .output()
//         .expect("Failed to generate identity");
//     let invitee_identity: serde_json::Value =
//         serde_json::from_slice(&invitee_identity.stdout).unwrap();
//     let invitee_public_key = invitee_identity["data"]["publicKey"].as_str().unwrap();
//     let invitee_private_key = invitee_identity["data"]["privateKey"].as_str().unwrap();

//     let invitation = Command::new(meroctl)
//         .args(&[
//             "--node-name",
//             "node1",
//             "--output-format",
//             "json",
//             "context",
//             "invite",
//             context_id,
//             inviteer_public_key,
//             invitee_public_key,
//         ])
//         .output()
//         .expect("Failed to invite");
//     let invitation: serde_json::Value = serde_json::from_slice(&invitation.stdout).unwrap();
//     let invitation_data = &invitation["data"];

//     let join = Command::new(meroctl)
//         .args(&[
//             "--node-name",
//             "node2",
//             "--output-format",
//             "json",
//             "context",
//             "join",
//             invitee_private_key,
//             invitation_data,
//         ])
//         .output()
//         .expect("Failed to join context");
//     println!("{}", String::from_utf8(join.stdout).unwrap());
// }
