use core::cell::RefCell;
use std::process::Stdio;

use camino::Utf8PathBuf;
use eyre::{bail, Result as EyreResult};
use tokio::fs::{create_dir_all, File};
use tokio::io::copy;
use tokio::process::{Child, Command};

use crate::output::OutputWriter;

pub struct Merod {
    pub name: String,
    process: RefCell<Option<Child>>,
    home_dir: Utf8PathBuf,
    log_dir: Utf8PathBuf,
    binary: Utf8PathBuf,
    output_writer: OutputWriter,
}

impl Merod {
    pub fn new(
        name: String,
        home_dir: Utf8PathBuf,
        logs_dir: &Utf8PathBuf,
        binary: Utf8PathBuf,
        output_writer: OutputWriter,
    ) -> Self {
        Self {
            process: RefCell::new(None),
            home_dir,
            log_dir: logs_dir.join(&name),
            binary,
            name,
            output_writer,
        }
    }

    pub async fn init<'a>(
        &'a self,
        swarm_host: &str,
        server_host: &str,
        swarm_port: u16,
        server_port: u16,
        args: impl IntoIterator<Item = &'a str>,
    ) -> EyreResult<()> {
        create_dir_all(&self.log_dir).await?;

        let mut child = self
            .run_cmd(
                [
                    "init",
                    "--swarm-host",
                    swarm_host,
                    "--server-host",
                    server_host,
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

        let config_args = [
            "config",
            "sync.timeout_ms=120000", // tolerable for now
            "sync.interval_ms=0",     // sync on every frequency tick
            "sync.frequency_ms=10000",
            "bootstrap.nodes=[]",
        ]
        .into_iter()
        .chain(args);

        let mut child = self.run_cmd(config_args, "config").await?;
        let result = child.wait().await?;
        if !result.success() {
            bail!("Failed to configure node '{}'", self.name);
        }

        Ok(())
    }

    pub async fn run(&self) -> EyreResult<()> {
        let child = self.run_cmd(["run"], "run").await?;

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

    async fn run_cmd<'a>(
        &'a self,
        args: impl IntoIterator<Item = &'a str>,
        log_suffix: &str,
    ) -> EyreResult<Child> {
        let mut command = Command::new(&self.binary);

        // Enable debug logging for merod run command
        if log_suffix == "run" {
            command.env("RUST_LOG", "calimero_node=debug,calimero_context=debug,calimero_storage=debug,calimero_dag=debug");
        }

        let mut command_line = format!("Command: '{}", &self.binary);

        let root_args = ["--home", self.home_dir.as_str(), "--node-name", &self.name];

        for arg in root_args.into_iter().chain(args) {
            let _ignored = command.arg(arg);
            command_line.reserve(arg.len() + 1);
            command_line.push(' ');
            command_line.push_str(arg);
        }

        command_line.push('\'');

        self.output_writer.write_str(&command_line);

        let log_file = self.log_dir.join(format!("{log_suffix}.log"));
        let mut log_file = File::create(&log_file).await?;

        let mut child = command.stdout(Stdio::piped()).spawn()?;

        if let Some(mut stdout) = child.stdout.take() {
            drop(tokio::spawn(async move {
                if let Err(err) = copy(&mut stdout, &mut log_file).await {
                    eprintln!("Error copying stdout: {err:?}");
                }
            }));
        }

        Ok(child)
    }

    pub async fn try_wait(&self) -> EyreResult<Option<i32>> {
        if let Some(child) = self.process.borrow_mut().as_mut() {
            Ok(child.try_wait()?.map(|status| status.code().unwrap_or(-1)))
        } else {
            Ok(None)
        }
    }
}
