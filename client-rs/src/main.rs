use clap::{Parser, Subcommand, ValueEnum};
use color_eyre::owo_colors::OwoColorize;
use inquire::{InquireError, Select};
use libp2p::Multiaddr;
use std::net::IpAddr;


mod storage;
mod output;
mod login_handler;
mod config;
mod network;
#[derive(Parser)]
#[command(
    version = "0.0.1",
    about = "CLI tool for interacting with P2P network components",
    long_about = None
)]
struct Cli {
    name: Option<String>,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Debug,Subcommand)]
enum Commands {
    /// Initialize nodes
    Init {
        /// List of bootstrap nodes
        #[clap(long, value_name = "ADDR")]
        boot_nodes: Vec<Multiaddr>,

        /// Use nodes from a known network
        #[clap(long, value_name = "NETWORK")]
        boot_network: Option<BootstrapNodes>,

        /// Host to listen on
        #[clap(long, value_name = "HOST")]
        #[clap(default_value = "0.0.0.0,::")]
        #[clap(use_value_delimiter = true)]
        host: Vec<IpAddr>,

        /// Port to listen on
        #[clap(long, value_name = "PORT")]
        #[clap(default_value_t = config::DEFAULT_PORT)]
        port: u16,

        /// Host to listen on
        #[clap(long, value_name = "RPC_HOST")]
        #[clap(default_value = "127.0.0.1")]
        rpc_host: String,

        /// Port to listen on
        #[clap(long, value_name = "RPC_PORT")]
        #[clap(default_value_t = config::DEFAULT_RPC_PORT)]
        rpc_port: u16,

        /// Enable mDNS discovery
        #[clap(long, default_value_t = true)]
        #[clap(overrides_with("no_mdns"))]
        mdns: bool,

        #[clap(long, hide = true)]
        #[clap(overrides_with("mdns"))]
        no_mdns: bool,

        /// Force initialization even if the directory already exists
        #[clap(long)]
        force: bool,
    },
    /// Connect P2P node to bootstrap node
    Join {
        #[arg(value_name = "ADDRESS", short = 'a', long = "address", aliases = ["addr", "address", "a"], required = true)]
        address: String,

        #[arg(value_name = "PORT", short = 'p', long = "port", aliases = ["p", "port"], required = true)]
        port: String,
    },
    /// Start an Application Session
    StartSession {
        #[arg(value_name = "application", long = "app", aliases = ["app", "application"], required = true)]
        application: String,

        #[arg(value_name = "ADDRESS", short = 'a', long = "address", aliases = ["addr", "address", "a"], required = true)]
        address: String,

        #[arg(value_name = "PORT", short = 'p', long = "port", aliases = ["p", "port"], required = true)]
        port: String,
    },
    /// Support for importing raw key pairs
    AddKeyPair {},
    /// Support for browser login
    Login {},
    /// List applications available in the Application Registry
    ListApps {},
    /// List available nodes in the network
    ListNodes {},
    SendMessage {
        #[arg(value_name = "ADDRESS", short = 'a', long = "address", aliases = ["addr", "address", "a"], required = true)]
        address: String,

        #[arg(value_name = "message", short = 'm', aliases = ["message", "msg", "m"], required = true)]
        message: String,
    },
    ReadMessage {
        #[arg(value_name = "ADDRESS", short = 'a', long = "address", aliases = ["addr", "address", "a"], required = true)]
        address: String,
    },
}

#[derive(Clone, Debug, ValueEnum)]
pub enum BootstrapNodes {
    Ipfs,
}

fn main() {
    let cli = Cli::parse();

    match &cli.command {
        Some(Commands::Init {
            boot_nodes,
            boot_network,
            host,
            port,
            rpc_host,
            rpc_port,
            mdns,
            no_mdns,
            force
        }) => {
                //to do (?)
        },
        Some(Commands::Join { address , port}) => {
            match (address.is_empty(), port.is_empty()) {
                (false, false) => {
                    println!("Joining network at: {}:{}", address.green(), port.green());
                    output::multi_progressbar();
                },
                _ => println!("Join address or port not specified."),
            }
            
        },
        Some(Commands::StartSession { application, address, port }) => {
            println!(
                "Starting new session...\nJoining application: {}\nApplication address: {}:{}", 
                application.green(), 
                address.green(), 
                port.green()
            );

            output::single_progressbar();
        },
        Some(Commands::Login {}) => {
                // implement listener for browser login (?)
                println!("Select Login Option.");
                let options: Vec<&str> = vec!["Browser Login", "CLI Login"];

                let result: Result<&str, InquireError> = Select::new("Login Option?", options).prompt();
                login_handler::handle_login_result(result);
        },
        Some(Commands::AddKeyPair {}) => {
            login_handler::cli_login();
        },
        Some(Commands::ListNodes {}) => {
            // fetch nodes from running node
            let asset = String::from("Nodes");
            let header: Vec<[&str; 3]> = vec![
                ["Node", "IP Address", "Configuration"]
            ];
            let data: Vec<[&str; 3]> = vec![
                ["q2edmwslq4w", "127.23.12.3", "P2P"],
                ["gkelsm24ls13s", "94.43.123.2", "P2P"],
            ];
            output::print_table(&asset, &header, &data);
        },
        Some(Commands::ListApps {}) => {
            // fetch applications from running node
            let asset = String::from("Applications");
            let header: Vec<[&str; 3]> = vec![
                ["Application", "IP Address", "Configuration"]
            ];
            let data: Vec<[&str; 3]> = vec![
                ["P2P Chat", "123.34.21.4:5314", "Node ID, Metadata"],
                ["P2P Docs", "143.32.1.89:1249", "Node ID, Metadata"],
            ];

            output::print_table(&asset,&header, &data);
        }
        Some(Commands::SendMessage {address, message}) => {
            network::send_message(address, message);
        
        },
        Some(Commands::ReadMessage {address}) => {
            network::read_message(address)
        },
        None => {}
    }
}