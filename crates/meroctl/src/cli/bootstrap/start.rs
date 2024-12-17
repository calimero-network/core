use std::cell::RefCell;
use std::process::Stdio;
use std::time::Duration;

use calimero_primitives::context::ContextId;
use calimero_primitives::hash::Hash;
use calimero_primitives::identity::{PrivateKey, PublicKey};
use camino::Utf8PathBuf;
use clap::Parser;
use eyre::{bail, Result as EyreResult};
use reqwest::Client;
use tokio::fs::{create_dir_all, File};
use tokio::io::copy;
use tokio::process::{Child, Command};
use tokio::time::sleep;

use crate::cli::app::install::InstallCommand;
use crate::cli::context::create::create_context;
use crate::cli::context::invite::InviteCommand;
use crate::cli::context::join::JoinCommand;
use crate::cli::{Environment, RootArgs};
use crate::common::{fetch_multiaddr, load_config};
use crate::output::Output;

#[derive(Parser, Debug)]
#[command(about = "Start bootstrap process")]
pub struct StartBootstrapCommand {
    #[clap(long, help = "Path to the merod executabe file")]
    pub merod_path: Utf8PathBuf,
    #[clap(long, help = "Path to the app wasm file")]
    pub app_path: Option<Utf8PathBuf>,
}

impl StartBootstrapCommand {
    pub async fn run(self, environment: &Environment) -> EyreResult<()> {
        println!("Starting bootstrap process");
        let nodes_dir: Utf8PathBuf = "data".into();
        let binary = self.merod_path.clone();

        // TODO Check if merod is provided

        // TODO Check if app is provided -> default from releases

        let node1_log_dir: Utf8PathBuf = "output/node_1_output".into();
        let node1_name = "node1".to_owned();
        let node_1_process: RefCell<Option<Child>> = RefCell::new(None);
        let root_args = RootArgs::new(
            nodes_dir.clone(),
            node1_name.clone(),
            crate::output::Format::Json,
        );
        let node1_environment =
            &Environment::new(root_args, Output::new(crate::output::Format::Json));

        println!("Initializing node {:?}.", node1_name);

        let init_res = init(
            binary.clone(),
            nodes_dir.clone(),
            node1_log_dir.clone(),
            node1_name.clone(),
            2528,
            2428,
        )
        .await
        .map_err(|e| {
            println!("Error init node: {}", e);
        });

        println!("Node {:?} initialized.", node1_name);

        println!("Starting node {:?} -> 10 sec", node1_name);

        let _child = run(
            binary.clone(),
            nodes_dir.clone(),
            node1_log_dir,
            node1_name.clone(),
            node_1_process,
        )
        .await
        .map_err(|e| {
            println!("Error run node: {}", e);
        });

        sleep(Duration::from_secs(10)).await;
        println!("Node {:?} started.", node1_name);

        println!("Creating context in {:?}", node1_name);
        let (context_id, public_key) =
            create_context_in_bootstrap(self.app_path, node1_environment).await?;

        // NODE 2
        let node2_name = "node2".to_owned();
        let node2_log_dir: Utf8PathBuf = "output/node_2_output".into();
        let node_2_process: RefCell<Option<Child>> = RefCell::new(None);
        let root_args = RootArgs::new(
            nodes_dir.clone(),
            node2_name.clone(),
            crate::output::Format::Json,
        );
        let node2_environment =
            &Environment::new(root_args, Output::new(crate::output::Format::Json));

        println!("Initializing node {:?}", node2_name);

        let init_res = init(
            binary.clone(),
            nodes_dir.clone(),
            node2_log_dir.clone(),
            node2_name.clone(),
            2529,
            2429,
        )
        .await
        .map_err(|e| {
            println!("Error init node: {}", e);
            // ApiError {
            //     status_code: StatusCode::INTERNAL_SERVER_ERROR,
            //     message: e.to_string(),
            // }
        });

        println!("Starting node {:?} -> 10 sec", node2_name);
        let node2 = run(
            binary,
            nodes_dir.clone(),
            node2_log_dir,
            node2_name.clone(),
            node_2_process,
        )
        .await
        .map_err(|e| {
            println!("Error run node: {}", e);
        });

        sleep(Duration::from_secs(10)).await;
        println!("Node {:?} started.", node2_name);

        //invite other peer

        //create node2 context identity
        println!(
            "Inviting node {:?} into context {:?}",
            node2_name,
            context_id.as_str()
        );
        let node2_private_key = PrivateKey::random(&mut rand::thread_rng());
        let invitation_payload = InviteCommand::invite(
            context_id,
            public_key,
            node2_private_key.public_key(),
            node1_environment,
        )
        .await?;
        println!("Node {:?} invited into context.", node2_name);

        println!("Joining node {:?} into context.", node2_name);
        let _ = JoinCommand::join(node2_private_key, invitation_payload, node2_environment).await?;
        println!("Node {:?} joined context.", node2_name);

        println!("Bootstrap finished. Nodes are ready to use!");

        // TODO break when one of the nodes exits
        loop {}

        Ok(())
    }
}

pub async fn init(
    binary: Utf8PathBuf,
    nodes_dir: Utf8PathBuf,
    log_dir: Utf8PathBuf,
    node_name: String,
    swarm_port: u32,
    server_port: u32,
) -> EyreResult<()> {
    create_dir_all(&nodes_dir.join(&node_name)).await?;
    create_dir_all(&log_dir).await?;

    let mut child = run_cmd(
        binary.clone(),
        nodes_dir.clone(),
        log_dir.clone(),
        node_name.clone(),
        &[
            "init",
            "--swarm-port",
            swarm_port.to_string().as_str(),
            "--server-port",
            server_port.to_string().as_str(),
        ],
        "init",
    )
    .await?;
    let result = child.wait().await?;
    if !result.success() {
        bail!("Failed to initialize node '{}'", node_name);
    }

    let mut config_args = vec!["config"];

    let mut child = run_cmd(
        binary,
        nodes_dir,
        log_dir,
        node_name.clone(),
        &config_args,
        "config",
    )
    .await?;
    let result = child.wait().await?;
    if !result.success() {
        bail!("Failed to configure node '{}'", node_name);
    }

    Ok(())
}

pub async fn run(
    binary: Utf8PathBuf,
    nodes_dir: Utf8PathBuf,
    log_dir: Utf8PathBuf,
    node_name: String,
    process: RefCell<Option<Child>>,
) -> EyreResult<()> {
    let child = run_cmd(binary, nodes_dir, log_dir, node_name, &["run"], "run").await?;

    *process.borrow_mut() = Some(child);

    Ok(())
}

pub async fn create_context_in_bootstrap(
    app_path: Option<Utf8PathBuf>,
    environment: &Environment,
) -> EyreResult<(ContextId, PublicKey)> {
    let config = load_config(&environment.args.home, &environment.args.node_name)?;
    let multiaddr = fetch_multiaddr(&config)?;
    let client = Client::new();

    let app_hash = Some(Hash::new("hash".as_bytes()));
    let app_metadata = Some("".to_owned());
    let url = Some("".to_owned());

    let application_id =
        InstallCommand::install_app(app_path, app_hash, app_metadata, url, environment)
            .await
            .map_err(|e| {
                println!("Error install app: {}", e);
                // ApiError {
                //     status_code: StatusCode::INTERNAL_SERVER_ERROR,
                //     message: e.to_string(),
                // }
            });

    let application_id = match application_id {
        Ok(app_id) => app_id,
        Err(e) => {
            bail!("Error install app");
        }
    };
    //create context

    let create_context_result = create_context(
        environment,
        &client,
        multiaddr,
        None,
        application_id,
        None,
        &config.identity,
    )
    .await
    .map_err(|e| {
        println!("Error create context: {}", e);
        // ApiError {
        //     status_code: StatusCode::INTERNAL_SERVER_ERROR,
        //     message: e.to_string(),
        // }
    });

    let (context_id, public_key) = match create_context_result {
        Ok((context_id, public_key)) => (context_id, public_key),
        Err(e) => {
            bail!("Error create context");
        }
    };
    println!("Context created: {:?}", context_id.as_str());

    Ok((context_id, public_key))
}

// cargo run -p meroctl -- --home <path_to_home> --node-name <node_name> context create --watch <path>

async fn run_cmd(
    binary: Utf8PathBuf,
    nodes_dir: Utf8PathBuf,
    log_dir: Utf8PathBuf,
    node_name: String,
    args: &[&str],
    log_suffix: &str,
) -> EyreResult<Child> {
    let mut root_args = vec!["--home", &nodes_dir.as_str(), "--node-name", &node_name];

    root_args.extend(args);

    let log_file = log_dir.join(format!("{}.log", log_suffix));
    let mut log_file = File::create(&log_file).await?;

    // output_writer
    //     .write_string(format!("Command: '{:}' {:?}", &binary, root_args));

    let mut child = Command::new(&binary)
        .args(root_args)
        .stdout(Stdio::piped())
        .spawn()?;

    if let Some(mut stdout) = child.stdout.take() {
        drop(tokio::spawn(async move {
            if let Err(err) = copy(&mut stdout, &mut log_file).await {
                eprintln!("Error copying stdout: {:?}", err);
            }
        }));
    }

    Ok(child)
}
