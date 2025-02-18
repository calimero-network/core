use std::process::Stdio;
use std::time::Duration;

use calimero_primitives::application::ApplicationId;
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
    #[clap(long, help = "Protocol to use for the bootstrap")]
    pub protocol: String,
}

impl StartBootstrapCommand {
    pub async fn run(mut self, environment: &Environment) -> EyreResult<()> {
        println!("Starting bootstrap process");
        let nodes_dir: Utf8PathBuf = environment.args.home.clone();
        let mut processes: Vec<Child> = vec![];

        // TODO app default from releases

        let mut demo_app = false;
        if self.app_path.is_none() {
            println!("Downloading demo app...");
            demo_app = true;

            let wasm_url = "https://github.com/calimero-network/core-app-template/raw/refs/heads/master/logic/res/increment.wasm";
            let output_path: Utf8PathBuf = "output/app.wasm".into();
            self.app_path = Some(output_path.clone());

            if let Err(e) = self.download_wasm(wasm_url, output_path).await {
                bail!("Failed to download the WASM file: {:?}", e);
            }
        }

        if self.protocol.is_empty() {
            bail!("Protocol is required for this operation");
        }

        let node1_log_dir: Utf8PathBuf = "output/node_1_output".into();
        let node1_name = "node1".to_owned();
        let node1_server_port: u32 = 2428;
        let node1_environment = &Environment::new(
            RootArgs::new(
                nodes_dir.clone(),
                node1_name.to_owned(),
                crate::output::Format::Json,
            ),
            Output::new(crate::output::Format::Json),
        );

        let node1_process = self
            .initialize_and_start_node(
                nodes_dir.to_owned(),
                node1_log_dir.to_owned(),
                &node1_name,
                2528,
                node1_server_port,
            )
            .await?;
        processes.push(node1_process);

        println!("Creating context in {:?}", node1_name);
        let (context_id, public_key, application_id) = self
            .create_context_in_bootstrap(node1_environment, self.protocol.clone())
            .await?;

        let node2_name = "node2".to_owned();
        let node2_log_dir: Utf8PathBuf = "output/node_2_output".into();
        let node2_server_port: u32 = 2429;
        let node2_environment = &Environment::new(
            RootArgs::new(
                nodes_dir.clone(),
                node2_name.to_owned(),
                crate::output::Format::Json,
            ),
            Output::new(crate::output::Format::Json),
        );

        let node2_process = self
            .initialize_and_start_node(
                nodes_dir.to_owned(),
                node2_log_dir.to_owned(),
                &node2_name,
                2529,
                node2_server_port,
            )
            .await?;
        processes.push(node2_process);

        let invitee_private_key = PrivateKey::random(&mut rand::thread_rng());

        self.invite_and_join_node(
            context_id,
            public_key,
            invitee_private_key,
            &node1_environment,
            &node2_environment,
        )
        .await?;

        println!("************************************************");
        println!("🚀 Bootstrap finished. Nodes are ready to use! 🚀");
        println!("Context id is {:?} ", context_id.to_string(),);

        if demo_app {
            println!(
                "Connect to the node from https://calimero-network.github.io/core-app-template/"
            );
            println!(
                "Open application in two separate windows to use it with two different nodes."
            );
            println!("Application setup screen requires application id and node url.");
            println!("Application id is {:?} ", application_id.to_string(),);

            println!(
                "Node {:?} url is http://localhost:{}",
                node1_environment.args.node_name, node1_server_port
            );
            println!(
                "Node {:?} url is http://localhost:{}",
                node1_environment.args.node_name, node2_server_port
            );
        }
        println!("************************************************");

        self.monitor_processes(processes).await;

        Ok(())
    }

    async fn initialize_and_start_node(
        &self,
        nodes_dir: Utf8PathBuf,
        log_dir: Utf8PathBuf,
        node_name: &str,
        swarm_port: u32,
        server_port: u32,
    ) -> EyreResult<Child> {
        println!("Initializing node {:?}", node_name);

        self.init(
            nodes_dir.to_owned(),
            log_dir.to_owned(),
            node_name.to_owned(),
            swarm_port,
            server_port,
        )
        .await?;

        println!("Starting node {:?}.", node_name);

        let process = self
            .run_node(nodes_dir, log_dir, node_name.to_owned())
            .await?;

        sleep(Duration::from_secs(10)).await;
        println!("Node {:?} started successfully.", &node_name);
        Ok(process)
    }

    async fn invite_and_join_node(
        &self,
        context_id: ContextId,
        inviter_public_key: PublicKey,
        invitee_private_key: PrivateKey,
        invitor_environment: &Environment,
        invitee_environment: &Environment,
    ) -> EyreResult<()> {
        println!(
            "Inviting node {:?} to context {:?}",
            invitee_environment.args.node_name,
            context_id.to_string()
        );

        let invite_command = InviteCommand {
            context: context_id.as_str().parse()?,
            inviter: inviter_public_key.as_str().parse()?,
            invitee_id: invitee_private_key.public_key(),
        };
        let invitation_payload = invite_command.invite(invitor_environment).await?;

        println!(
            "Node {:?} successfully invited.",
            invitee_environment.args.node_name
        );

        println!(
            "Joining node {:?} to context.",
            invitee_environment.args.node_name
        );

        let join_command = JoinCommand {
            private_key: invitee_private_key,
            invitation_payload,
        };
        join_command.run(invitee_environment).await?;
        println!(
            "Node {:?} joined successfully.",
            invitee_environment.args.node_name
        );

        Ok(())
    }

    pub async fn init(
        &self,
        nodes_dir: Utf8PathBuf,
        log_dir: Utf8PathBuf,
        node_name: String,
        swarm_port: u32,
        server_port: u32,
    ) -> EyreResult<()> {
        create_dir_all(&nodes_dir.join(&node_name)).await?;
        create_dir_all(&log_dir).await?;

        let mut child = self
            .run_cmd(
                nodes_dir.clone(),
                log_dir.clone(),
                node_name.clone(),
                [
                    "init",
                    "--swarm-port",
                    &swarm_port.to_string().as_str(),
                    "--server-port",
                    &server_port.to_string().as_str(),
                ],
                "init",
            )
            .await?;

        let result = child.wait().await?;
        if !result.success() {
            bail!("Failed to initialize node '{}'", node_name);
        }

        let mut child = self
            .run_cmd(nodes_dir, log_dir, node_name.clone(), ["config"], "config")
            .await?;
        let result = child.wait().await?;
        if !result.success() {
            bail!("Failed to configure node '{}'", node_name);
        }
        Ok(())
    }

    pub async fn run_node(
        &self,
        nodes_dir: Utf8PathBuf,
        log_dir: Utf8PathBuf,
        node_name: String,
    ) -> EyreResult<Child> {
        Ok(self
            .run_cmd(nodes_dir, log_dir, node_name, ["run"], "run")
            .await?)
    }

    pub async fn create_context_in_bootstrap(
        &self,
        environment: &Environment,
        protocol: String,
    ) -> EyreResult<(ContextId, PublicKey, ApplicationId)> {
        let config = load_config(&environment.args.home, &environment.args.node_name)?;
        let multiaddr = fetch_multiaddr(&config)?;
        let client = Client::new();

        let install_command = InstallCommand {
            path: self.app_path.clone(),
            url: Some("".to_owned()),
            metadata: Some("".to_owned()),
            hash: Some(Hash::new("hash".as_bytes())),
        };

        let application_id = install_command.install_app(environment).await?;

        let (context_id, public_key) = create_context(
            environment,
            &client,
            multiaddr,
            None,
            application_id,
            None,
            &config.identity,
            protocol,
            None,
        )
        .await?;

        println!("Context created: {:?}", context_id.as_str());

        Ok((context_id, public_key, application_id))
    }

    async fn run_cmd(
        &self,
        nodes_dir: Utf8PathBuf,
        log_dir: Utf8PathBuf,
        node_name: String,
        args: impl IntoIterator<Item = &str>,
        log_suffix: &str,
    ) -> EyreResult<Child> {
        let root_args = ["--home", &nodes_dir.as_str(), "--node-name", &node_name];

        let log_file = log_dir.join(format!("{}.log", log_suffix));
        let mut log_file = File::create(&log_file).await?;

        let mut child = Command::new(&self.merod_path)
            .args(root_args)
            .args(args)
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

    async fn monitor_processes(&self, mut processes: Vec<Child>) {
        loop {
            for (i, process) in processes.iter_mut().enumerate() {
                match process.try_wait() {
                    Ok(Some(status)) => {
                        println!("Node {} exited with status: {:?}", i + 1, status);
                        return;
                    }
                    Ok(None) => continue,
                    Err(e) => {
                        println!("Error checking node status: {:?}", e);
                        return;
                    }
                }
            }
            sleep(Duration::from_secs(1)).await;
        }
    }

    async fn download_wasm(&self, url: &str, output_path: Utf8PathBuf) -> EyreResult<()> {
        let client = Client::new();

        let response = client
            .get(url)
            .send()
            .await
            .map_err(|e| eyre::eyre!("Request failed: {}", e))?;

        if !response.status().is_success() {
            bail!("Request failed with status: {}", response.status());
        }

        let mut file = File::create(&output_path)
            .await
            .map_err(|e| eyre::eyre!("Failed to create file: {}", e))?;

        let _ = copy(&mut response.bytes().await?.as_ref(), &mut file)
            .await
            .map_err(|e| eyre::eyre!("Failed to copy response bytes: {}", e))?;

        println!("Demo app downloaded successfully.");
        Ok(())
    }
}
