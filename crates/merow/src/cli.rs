use clap::Parser;
use const_format::concatcp;
use eyre::Result as EyreResult;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{self, Write};
use std::path::Path;
use std::process::exit;
use std::process::{Command, Output, Stdio};
use toml;

pub const EXAMPLES: &str = r"

  # Initialize a node  
  $ merow -- init-node 

  # Start a running node 
  $ merow -- start-node 
";

#[derive(Debug, Parser)]
#[command(author, version, about, long_about = None)]
#[command(after_help = concatcp!(
    "Examples:",
    EXAMPLES
))]
pub struct RootCommand {
    /// Name of the command
    #[arg(default_value_t = String::from("init-node"))]
    pub action: String,
}

#[derive(Serialize, Deserialize, Debug)]
struct NodeData {
    node: NodeConfig,
}

#[derive(Serialize, Deserialize, Debug)]
struct NodeConfig {
    name: String,
    server_port: u16,
    swarm_port: u16,
    home: String,
}

fn build_command(
    name: &str,
    home: &str,
    server: Option<&str>,
    swarm: Option<&str>,
    run_node: bool,
) -> Command {
    let mut command: Command = Command::new("cargo");

    // Sets the default CLI arguments
    let _ = command.args([
        "run",
        "-p",
        "merod",
        "--",
        "--node-name",
        name,
        "--home",
        home,
    ]);

    // Sets the custom CLI arguments
    if !run_node {
        let _ = command.args([
            "init",
            "--server-port",
            server.unwrap(),
            "--swarm-port",
            swarm.unwrap(),
        ]);
    } else {
        let _ = command.arg("run");
    }

    // Sets the standard IO configuration
    // let _ = command.stdout(Stdio::piped()); // Capture stdout
    // let _ = command.stderr(Stdio::piped()); // Capture stderr

    return command;
}

fn display_command_output(output: Output) {
    println!("Status: {}", output.status);
    io::stdout().write_all(&output.stdout).unwrap();
    // println!("Stdout: {}", String::from_utf8_lossy(&output.stdout));
    // println!("Stderr: {}", String::from_utf8_lossy(&output.stderr));
}

fn make_direcory(node_home: &str) {
    match fs::create_dir(node_home) {
        Ok(()) => println!("Created Home Directory: ./{}\n", node_home),
        Err(error) => panic!("Problem creating the Node Home directory: {error:?}"),
    };
}

fn init_node(config: &NodeConfig) -> EyreResult<()> {
    // Sets the default configuration for the node
    let node_name: &str = config.name.as_str();
    let node_home: &str = config.home.as_str();

    let server_port: &str = &config.server_port.to_string();
    let swarm_port: &str = &config.swarm_port.to_string();

    // create the home directory if it doesnt exist
    if !Path::new(node_home).is_dir() {
        // Make the Node home directory
        make_direcory(node_home);
    }

    let mut command: Command = build_command(
        node_name,
        node_home,
        Some(server_port),
        Some(swarm_port),
        false,
    );

    let child: Output = command.output().expect("failed to execute process"); // Execute the command and get the output

    display_command_output(child);

    Ok(()) // Return the output (stdout, stderr, and exit status)
}

async fn start_node(node_name: &str, node_home: &str) -> EyreResult<()> {
    let mut command: Command = build_command(node_name, node_home, None, None, true);
    let child: Output = command.output()?;

    display_command_output(child);
    Ok(())
}

impl RootCommand {
    pub fn new(arg: &str) -> RootCommand {
        let action = String::from(arg);
        RootCommand { action }
    }

    pub fn init(self) -> EyreResult<()> {
        // Fetch the nodes configuration
        let data = NodeData::get_node_data();
        let admin = data.node;
        init_node(&admin)
    }

    pub async fn run(self) -> EyreResult<()> {
        // Fetch the nodes configuration
        let data = NodeData::get_node_data();
        let admin = data.node;

        match self.action.as_str() {
            "init-node" => {
                println!("Initializing node...\n");
                init_node(&admin)
            }
            "start-node" => {
                println!("Running node...\n");

                let name: &str = admin.name.as_str();
                let home: &str = admin.home.as_str();

                start_node(name, home).await
            }
            _ => {
                println!("Unknown command...");
                Ok(())
            }
        }
    }
}

impl NodeData {
    fn get_node_data() -> NodeData {
        // Sets the contents of the configuration file to a String
        let config_file_path = crate::read_env();
        let contents = match fs::read_to_string(&config_file_path) {
            Ok(c) => c,
            Err(_) => {
                eprintln!("Could not read file `{}`", &config_file_path);
                exit(1);
            }
        };

        // Deserializes the String into a type (NodeData)
        let node_data: NodeData = match toml::from_str(&contents) {
            Ok(nd) => nd,
            Err(_) => {
                // Write `msg` to `stderr`.
                eprintln!("Unable to load data from `{}`", &config_file_path);
                // Exit the program with exit code `1`.
                exit(1);
            }
        };

        return node_data;
    }
}
