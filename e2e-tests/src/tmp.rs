use clap::{AppSettings, Clap, Parser};
use const_format::concatcp;
use serde::Deserialize;
use std::fs;
use std::process::{Child, Command};
use tokio::time::{sleep, Duration};

#[derive(Clap)]
#[clap(version = "1.0", author = "Author")]
#[clap(setting = AppSettings::ColoredHelp)]
struct Opts {
    #[clap(subcommand)]
    subcmd: SubCommand,
}

pub const EXAMPLES: &str = r"
  # List all applications
  $ meroctl -- --node-name node1 app ls

  # List all contexts
  $ meroctl -- --home data --node-name node1 context ls
";

#[derive(Debug, Parser)]
#[command(author, version, about, long_about = None)]
#[command(after_help = concatcp!(
    "Environment variables:\n",
    "  CALIMERO_HOME    Directory for config and data\n\n",
    "Examples:",
    EXAMPLES
))]
pub struct RootCommand {
    #[command(flatten)]
    pub args: RootArgs,

    #[command(subcommand)]
    pub action: SubCommands,
}

#[derive(Debug, Subcommand)]
pub enum SubCommands {
    App(AppCommand),
    Context(ContextCommand),
    Identity(IdentityCommand),
    JsonRpc(CallCommand),
}

#[derive(Debug, Parser)]
pub struct RootArgs {
    /// Directory for config and data
    #[arg(long, value_name = "PATH", default_value_t = defaults::default_node_dir())]
    #[arg(env = "CALIMERO_HOME", hide_env_values = true)]
    pub home: Utf8PathBuf,

    pub merod_binary: Utf8PathBuf,

    pub meroctl_binary: Utf8PathBuf,
}

#[derive(Clap)]
enum SubCommand {
    Init(Init),
    Run(Run),
}

#[derive(Clap)]
struct Init {
    #[clap(short, long)]
    node_name: String,
}

#[derive(Clap)]
struct Run {
    #[clap(short, long)]
    node_name: String,
}

async fn invite_join(meroctl: &str, app_path: &str) {
    let application = Command::new(meroctl)
        .args(&[
            "--node-name",
            "node1",
            "--output-format",
            "json",
            "app",
            "install",
            "--path",
            app_path,
        ])
        .output()
        .expect("Failed to install app");
    let application: serde_json::Value = serde_json::from_slice(&application.stdout).unwrap();
    let application_id = application["data"]["applicationId"].as_str().unwrap();

    let context = Command::new(meroctl)
        .args(&[
            "--node-name",
            "node1",
            "--output-format",
            "json",
            "context",
            "create",
            "-a",
            application_id,
        ])
        .output()
        .expect("Failed to create context");
    let context: serde_json::Value = serde_json::from_slice(&context.stdout).unwrap();
    let context_id = context["data"]["contextId"].as_str().unwrap();
    let inviteer_public_key = context["data"]["memberPublicKey"].as_str().unwrap();

    let invitee_identity = Command::new(meroctl)
        .args(&[
            "--node-name",
            "node2",
            "--output-format",
            "json",
            "identity",
            "generate",
        ])
        .output()
        .expect("Failed to generate identity");
    let invitee_identity: serde_json::Value =
        serde_json::from_slice(&invitee_identity.stdout).unwrap();
    let invitee_public_key = invitee_identity["data"]["publicKey"].as_str().unwrap();
    let invitee_private_key = invitee_identity["data"]["privateKey"].as_str().unwrap();

    let invitation = Command::new(meroctl)
        .args(&[
            "--node-name",
            "node1",
            "--output-format",
            "json",
            "context",
            "invite",
            context_id,
            inviteer_public_key,
            invitee_public_key,
        ])
        .output()
        .expect("Failed to invite");
    let invitation: serde_json::Value = serde_json::from_slice(&invitation.stdout).unwrap();
    let invitation_data = &invitation["data"];

    let join = Command::new(meroctl)
        .args(&[
            "--node-name",
            "node2",
            "--output-format",
            "json",
            "context",
            "join",
            invitee_private_key,
            invitation_data,
        ])
        .output()
        .expect("Failed to join context");
    println!("{}", String::from_utf8(join.stdout).unwrap());
}

#[tokio::main]
async fn main() {
    let opts: Opts = Opts::parse();

    let repo_root = Command::new("git")
        .args(&["rev-parse", "--show-toplevel"])
        .output()
        .expect("Failed to get repo root")
        .stdout;
    let repo_root = String::from_utf8(repo_root).unwrap().trim().to_string();
    println!("Discovered repo root: {}", repo_root);

    let app_path = format!("{}/apps/kv-store/res/kv_store.wasm", repo_root);
    if !std::path::Path::new(&app_path).exists() {
        eprintln!("Error: Application path {} does not exist.", app_path);
        std::process::exit(1);
    }
    println!("Using app path: {}", app_path);

    let merod = format!("{}/target/debug/merod", repo_root);
    if !std::path::Path::new(&merod).exists() {
        eprintln!("Error: Merod executable {} does not exist.", merod);
        std::process::exit(1);
    }
    println!("Discovered MEROD executable: {}", merod);

    let meroctl = format!("{}/target/debug/meroctl", repo_root);
    if !std::path::Path::new(&meroctl).exists() {
        eprintln!("Error: Meroctl executable {} does not exist.", meroctl);
        std::process::exit(1);
    }
    println!("Discovered MEROCTL executable: {}", meroctl);

    let test_dir = std::env::current_dir().unwrap();
    let corpus_dir = test_dir.join("corpus");
    fs::create_dir_all(&corpus_dir).unwrap();
    println!("Created test corpus dir: {}", test_dir.display());

    let calimero_home = corpus_dir.join("calimero");
    fs::create_dir_all(&calimero_home).unwrap();
    std::env::set_var("CALIMERO_HOME", calimero_home);

    match opts.subcmd {
        SubCommand::Init(_) => initialize_nodes(&calimero_home.to_str().unwrap(), &merod).await,
        SubCommand::Run(_) => {
            let (node1, node2) = start_nodes(&merod, &corpus_dir.to_str().unwrap()).await;
            invite_join(&meroctl, &app_path).await;
            // Ensure nodes are stopped before exiting
            node1.kill().unwrap();
            node2.kill().unwrap();
        }
    }
}
