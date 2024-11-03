use std::cell::RefCell;
use std::process::Stdio;

use camino::Utf8PathBuf;
use eyre::Result as EyreResult;
use tokio::fs::{create_dir_all, File};
use tokio::io::copy;
use tokio::process::{Child, Command};

use crate::TestEnvironment;

pub struct Merod {
    pub name: String,
    process: RefCell<Option<Child>>,
    nodes_dir: Utf8PathBuf,
    logs_dir: Utf8PathBuf,
    binary: Utf8PathBuf,
}

impl Merod {
    pub fn new(name: String, environment: &TestEnvironment) -> Self {
        let logs_dir = environment.logs_dir.join(&name);

        Self {
            name,
            process: RefCell::new(None),
            nodes_dir: environment.nodes_dir.clone(),
            logs_dir,
            binary: environment.merod_binary.clone(),
        }
    }

    pub async fn init(&self, swarm_port: u32, server_port: u32) -> EyreResult<()> {
        let node_dir = self.nodes_dir.join(&self.name);
        create_dir_all(&node_dir).await?;

        let logs_dir = self.logs_dir.join(&self.name);
        create_dir_all(&logs_dir).await?;

        let mut child = self
            .run_cmd(
                Box::new([
                    "init",
                    "--swarm-port",
                    swarm_port.to_string().as_str(),
                    "--server-port",
                    server_port.to_string().as_str(),
                ]),
                "init",
            )
            .await?;

        let result = child.wait().await?;
        assert_eq!(result.code(), Some(0));

        Ok(())
    }

    pub async fn run(&self) -> EyreResult<()> {
        let child = self.run_cmd(Box::new(["run"]), "run").await?;

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

    async fn run_cmd(&self, args: Box<[&str]>, log_suffix: &str) -> EyreResult<Child> {
        let mut root_args = vec!["--home", self.nodes_dir.as_str(), "--node-name", &self.name];

        root_args.extend(args);

        let log_file = self
            .logs_dir
            .join(format!("{}-{}.log", self.name, log_suffix));
        let mut log_file = File::create(&log_file).await?;

        println!("Running command '{:}' {:?}", &self.binary, root_args);

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
