use clap::{Parser, Subcommand};
use color_eyre::owo_colors::OwoColorize;
use inquire::{InquireError, Select, Text, Password};
use std::process;
use serde::Serialize;
use std::fs::File;
use std::io::prelude::*;
use dirs;
use open;
use colored::*;
use prettytable::Table;
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use std::thread;
use std::time::Duration;

#[derive(Serialize)]
struct Credentials {
    account_id: String,
    private_key: String,
    public_key: String
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
    ListNodes {}
}

fn main() {
    let cli = Cli::parse();

    match &cli.command {
        Some(Commands::Join { address , port}) => {
            if !address.is_empty() && !port.is_empty() {
                println!("Joining network at: {}:{}", address.green(), port.green());
            } else {
                println!("join address or port not specified.");
            }
            let m = MultiProgress::new();
            let sty = ProgressStyle::with_template(
                "[{elapsed_precise}] {bar:40.cyan/blue} {pos:>7}/{len:7} {msg}",
            )
            .unwrap()
            .progress_chars("##-");

            let n = 200;
            let pb = m.add(ProgressBar::new(n));
            pb.set_style(sty.clone());
            pb.set_message("todo");
            let pb2 = m.add(ProgressBar::new(n));
            pb2.set_style(sty.clone());
            pb2.set_message("finished");

            let pb3 = m.insert_after(&pb2, ProgressBar::new(1024));
            pb3.set_style(sty);

            m.println("Joining...").unwrap();


            let m_clone = m.clone();
            let h3 = thread::spawn(move || {
                for i in 0..1024 {
                    thread::sleep(Duration::from_millis(2));
                    pb3.set_message(format!("item #{}", i + 1));
                    pb3.inc(1);
                }
                m_clone.println("Connecting to boostrap node finished!").unwrap();
                pb3.finish_with_message("done");
            });

            
            pb.finish_with_message("all jobs started");
            let _ = h3.join();
            pb2.finish_with_message("all jobs done");
            m.clear().unwrap();
        },
        Some(Commands::StartSession { application, address, port }) => {
            println!("Starting new session...");
            println!("Joining application: {}", application.green());
            println!("Application address: {}:{}", address.green(), port.green());
            let pb = ProgressBar::new(10);

            for _ in 0..10 {
                thread::sleep(Duration::from_millis(200));
                pb.inc(1);
            }

            pb.finish_with_message("Done");

            
        },
        Some(Commands::Login {}) => {
                println!("Select Login Option.");
                let options: Vec<&str> = vec!["Browser Login", "CLI Login"];

                let ans: Result<&str, InquireError> = Select::new("Login Option?", options).prompt();

                match ans {
                    Ok(selected_option) => {
                        if selected_option == "Browser Login" {
                            let _ = open::that("https://www.mynearwallet.com/");
                        } else if selected_option == "CLI Login" {
                            let account_id = Text::new("Account ID:").prompt().unwrap_or_else(|_| {
                                println!("Error: Not a valid account ID!");
                                process::exit(1);
                            });

                            let private_key = Password::new("Account private key:").prompt().unwrap_or_else(|_| {
                                println!("Error: Not a valid private key!");
                                process::exit(1);
                            });
                            save_keys_to_storage(&account_id, &private_key, &String::new());

                            println!("Logged in with : {}",account_id);
                        }
                    },
                    Err(err) => {
                        println!("Error: {:?}", err);
                    }
                }
        },
        Some(Commands::AddKeyPair {}) => {
            println!("Adding raw keypair");
            let account_id = Text::new("Account ID:").prompt().unwrap_or_else(|_| {
                println!("Error: Not a valid account ID!");
                process::exit(1);
            });

            let public_key = Text::new("Account public key:").prompt().unwrap_or_else(|_| {
                println!("Error: Not a valid account ID!");
                process::exit(1);
            });

            let private_key = Password::new("Account private key:").prompt().unwrap_or_else(|_| {
                println!("Error: Not a valid private key!");
                process::exit(1);
            });

            save_keys_to_storage(&account_id, &private_key, &public_key);
        },
        Some(Commands::ListNodes {}) => {
            println!("Listing {}...", "Nodes".green());
            let mut table = Table::new();
            table.add_row(prettytable::row!["Node", "IP Address", "Configuration"]);
            table.add_row(prettytable::row!["q2edmwslq4w", "127.23.12.3", "P2P"]);
            table.add_row(prettytable::row!["gkelsm24ls13s", "94.43.123.2", "P2P"]);

            table.printstd();
        },
        Some(Commands::ListApps {}) => {
            println!("Listing {}...", "Applications".green());
            let mut table = Table::new();
            table.add_row(prettytable::row!["Application", "IP Address", "Configuration"]);
            table.add_row(prettytable::row!["P2P Chat", "123.34.21.4:5314", "Node ID, Metadata"]);
            table.add_row(prettytable::row!["P2P Docs", "143.32.1.89:1249", "Node ID, Metadata"]);

            table.printstd();
        }
        None => {}
    }
}

fn save_keys_to_storage(account_id: &String, private_key: &String, public_key: &String) {
    let credentials = Credentials {
        account_id: account_id.to_string(),
        private_key: private_key.to_string(),
        public_key: public_key.to_string()
    };

    let json_data = serde_json::to_string(&credentials).expect("Failed to serialize credentials to JSON");

    let home_dir = dirs::home_dir().expect("Failed to get home directory");
    let credentials_dir = home_dir.join(".calimero/credentials");
    let account_id_file = credentials_dir.join(format!("{}.json", account_id));

    if !credentials_dir.exists() {
        std::fs::create_dir_all(&credentials_dir).expect("Failed to create credentials directory");
    }

    let mut file = File::create(account_id_file).expect("Failed to create file");
    file.write_all(json_data.as_bytes()).expect("Failed to write to file");
}