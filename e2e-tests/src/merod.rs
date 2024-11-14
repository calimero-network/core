use std::cell::RefCell;
use std::process::Stdio;

use camino::Utf8PathBuf;
use eyre::{bail, Result as EyreResult};
use tokio::fs::{create_dir_all, File};
use tokio::io::copy;
use tokio::process::{Child, Command};

use crate::TestEnvironment;

pub struct Merod {
    pub name: String,
    test_id: u32,
    process: RefCell<Option<Child>>,
    nodes_dir: Utf8PathBuf,
    log_dir: Utf8PathBuf,
    binary: Utf8PathBuf,
}

impl Merod {
    pub fn new(name: String, environment: &TestEnvironment) -> Self {
        Self {
            test_id: environment.test_id,
            process: RefCell::new(None),
            nodes_dir: environment.nodes_dir.clone(),
            log_dir: environment.logs_dir.join(&name),
            binary: environment.merod_binary.clone(),
            name,
        }
    }

    pub async fn init(&self, swarm_port: u32, server_port: u32, args: &[&str]) -> EyreResult<()> {
        create_dir_all(&self.nodes_dir.join(&self.name)).await?;
        create_dir_all(&self.log_dir).await?;

        let mut child = self
            .run_cmd(
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
            bail!("Failed to initialize node '{}'", self.name);
        }

        let rendezvous_ns_arg = format!(
            "discovery.rendezvous.namespace=\"calimero/e2e-tests/{}\"",
            self.test_id
        );
        let mut config_args = vec!["config", rendezvous_ns_arg.as_str()];
        config_args.extend(args);

        let mut child = self.run_cmd(&config_args, "config").await?;
        let result = child.wait().await?;
        if !result.success() {
            bail!("Failed to configure node '{}'", self.name);
        }

        Ok(())
    }

    pub async fn run(&self) -> EyreResult<()> {
        let child = self.run_cmd(&["run"], "run").await?;

        *self.process.borrow_mut() = Some(child);

        Ok(())
    }

    pub async fn stop(&self) -> EyreResult<()> {
        if let Some(mut child) = self.process.borrow_mut().take() {
            use nix::sys::signal::{self, Signal};
            use nix::unistd::Pid;

            if let Some(child_id) = child.id() {
                signal::kill(Pid::from_raw(child_id as i32), Signal::SIGTERM)?;
            }

            let _ = child.wait().await?;
        }

        Ok(())
    }

    async fn run_cmd(&self, args: &[&str], log_suffix: &str) -> EyreResult<Child> {
        let mut root_args = vec!["--home", self.nodes_dir.as_str(), "--node-name", &self.name];

        root_args.extend(args);

        let log_file = self.log_dir.join(format!("{}.log", log_suffix));
        let mut log_file = File::create(&log_file).await?;

        println!("Command: '{:}' {:?}", &self.binary, root_args);

        let mut child = Command::new(&self.binary)
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
}
