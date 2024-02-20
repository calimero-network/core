mod storage;
mod output;
mod login_handler;
mod config;
mod network;
mod init;
mod ws_client;
mod api;
mod app;

use std::net::IpAddr;
use std::thread;

use clap::{Parser, Subcommand, ValueEnum};
use color_eyre::owo_colors::OwoColorize;
use inquire::{InquireError, Select};
use libp2p::Multiaddr;

#[derive(Clone, Debug, ValueEnum)]
pub enum BootstrapNodes {
    Ipfs,
}

pub struct RootArgs {
    pub home: camino::Utf8PathBuf,
}

pub struct InitParams {
    pub boot_nodes: Vec<Multiaddr>,
    pub boot_network: Option<BootstrapNodes>,
    pub host: Vec<IpAddr>,
    pub port: u16,
    pub rpc_host: String,
    pub rpc_port: u16,
    pub mdns: bool,
    pub no_mdns: bool,
    pub force: bool,
}

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
        /// Directory for config and data
        #[clap(long, value_name = "PATH", default_value_t = config::default_chat_dir())]
        #[clap(env = "CALIMERO_CHAT_HOME", hide_env_values = true)]
        home: camino::Utf8PathBuf,
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
    /// List available nodes in the network
    ListNodes {},
    /// Send message through P2P chat
    SendMessage {
        #[arg(value_name = "ADDRESS", short = 'a', long = "address", aliases = ["addr", "address", "a"], required = true)]
        address: String,

        #[arg(value_name = "message", short = 'm', aliases = ["message", "msg", "m"], required = true)]
        message: String,
    },
    /// Read message that was broadcasted
    ReadMessage {
        #[arg(value_name = "ADDRESS", short = 'a', long = "address", aliases = ["addr", "address", "a"], required = true)]
        address: String,
    },
    /// List applications available in the Application Registry - Executable bundle of functionalities
    ListRemoteApps {
        #[arg(value_name = "ADDRESS", short = 'a', long = "ws-address", aliases = ["addr", "address", "a", "ws-address"], required = true)]
        ws_address: String,
    },
    /// List installed Apps
    ListInstalledApps {
        #[arg(value_name = "ADDRESS", short = 'a', long = "ws-address", aliases = ["addr", "address", "a", "ws-address"], required = true)]
        ws_address: String,
    },
    /// Install application from Binary
    InstallBinaryApp {
        #[arg(value_name = "ADDRESS", short = 'a', long = "ws-address", aliases = ["addr", "address", "a", "ws-address"], required = true)]
        ws_address: String,

        #[arg(value_name = "BINARY", short = 'p', long = "binary-path", aliases = ["path", "p", "binary-path"], required = true)]
        binary_path: String,
    },
    /// Install application from Application Registry
    InstallRemoteApp {
        #[arg(value_name = "ADDRESS", short = 'a', long = "ws-address", aliases = ["addr", "address", "a", "ws-address"], required = true)]
        ws_address: String,

        #[arg(value_name = "APP_ID", long = "app-id", aliases = ["app-id", "app", "id"], required = true)]
        app_id: u32,
    },
    /// Uninstall applicaton,
    UninstallApp {
        #[arg(value_name = "ADDRESS", short = 'a', long = "ws-address", aliases = ["addr", "address", "a", "ws-address"], required = true)]
        ws_address: String,

        #[arg(value_name = "APP_ID", long = "app-id", aliases = ["app-id", "app", "id"], required = true)]
        app_id: u32,
    },
    /// Subscribe to application websocket,
    Subscribe {
        #[arg(value_name = "ADDRESS", short = 'a', long = "ws-address", aliases = ["addr", "address", "a", "ws-address"], required = true)]
        ws_address: String,

        #[arg(value_name = "APP_ID", long = "app-id", aliases = ["app-id", "app", "id"], required = true)]
        app_id: u32,
    },
    /// Unsubscribe application websocket
    Unsubscribe {
        #[arg(value_name = "ADDRESS", short = 'a', long = "ws-address", aliases = ["addr", "address", "a", "ws-address"], required = true)]
        ws_address: String,

        #[arg(value_name = "APP_ID", long = "app-id", aliases = ["app-id", "app", "id"], required = true)]
        app_id: u32,
    },
    /// Unsubscribe All websockets
    UnsubscribeAll {
        #[arg(value_name = "ADDRESS", short = 'a', long = "ws-address", aliases = ["addr", "address", "a", "ws-address"], required = true)]
        ws_address: String,
    }
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    match &cli.command {
        Some(Commands::Init {
            home,
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

            let main_thread = thread::current();

            let root_args = RootArgs {
                home: home.clone(),
            };
        
            let init_params = InitParams {
                boot_nodes: boot_nodes.iter().map(|addr| addr.clone()).collect(),
                boot_network: boot_network.clone(),
                host: host.iter().map(|ip| ip.clone()).collect(),
                port: *port,
                rpc_host: rpc_host.clone(),
                rpc_port: *rpc_port,
                mdns: *mdns,
                no_mdns: *no_mdns,
                force: *force,
            };
            println!("{}",init_params.mdns);
            let _handle = thread::spawn(move || {
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .unwrap();
                
                rt.block_on(async {

                    
                    let _ = init::run(root_args, init_params).await;
                });

                main_thread.unpark();
            });

            thread::park();
            
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
        Some(Commands::SendMessage {address, message}) => {
            network::send_message(address, message);
        
        },
        Some(Commands::ReadMessage {address}) => {
            network::read_message(address)
        },
        Some(Commands::ListRemoteApps { ws_address }) => {
            let method = String::from("listRemoteApps");
            ws_client::list_remote_apps(ws_address, &method).await;
        },
        Some(Commands::ListInstalledApps {ws_address}) => {
            
        },
        Some(Commands::InstallBinaryApp {ws_address, binary_path }) => {

        },
        Some(Commands::InstallRemoteApp { ws_address, app_id }) => {

        },
        Some(Commands::UninstallApp { ws_address, app_id }) => {

        },
        Some(Commands::Subscribe { ws_address, app_id }) => {

        },
        Some(Commands::Unsubscribe { ws_address, app_id }) => {

        },
        Some(Commands::UnsubscribeAll { ws_address }) => {

        }
        None => {}
    }
}
