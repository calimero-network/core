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
        args: Vec<&'a str>, // accept owned Vec to allow multiple iterations
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
                    &swarm_port.to_string(),
                    "--server-port",
                    &server_port.to_string(),
                ],
                "init",
            )
            .await?;

        let result = child.wait().await?;
        if !result.success() {
            bail!("Failed to initialize node '{}'", self.name);
        }

        // Clone for safe reuse
        let config_args: Vec<String> = args.iter().map(|s| s.to_string()).collect();

        let config_changed = !config_args.is_empty();

        if config_changed {
            // Reuse original args for chaining
            let mut child = self
                .run_cmd(["config", "set"].into_iter().chain(args.iter().copied()), "config")
                .await?;

            let result = child.wait().await?;
            if !result.success() {
                bail!("Failed to configure node '{}'", self.name);
            }
        } else {
            // Check for output format
            let output_format = args
                .iter()
                .find(|arg| arg.starts_with("--output-format"))
                .and_then(|arg| arg.split('=').nth(1));

            let print_format = match output_format {
                Some("json") => "--format json",
                _ => "",
            };

            let mut child = self
                .run_cmd(
                    ["config", "print"]
                        .into_iter()
                        .chain(if print_format.is_empty() {
                            None
                        } else {
                            Some(print_format)
                        }),
                    "config-print",
                )
                .await?;

            let result = child.wait().await?;
            if !result.success() {
                bail!("Failed to print node '{}' configuration", self.name);
            }
        }

        Ok(())
    }

    pub async fn hints(&self) -> EyreResult<()> {
        let hints = r#"
        Hints:
        - sync.timeout_ms: Valid values are any positive integer in milliseconds.
        - sync.interval_ms: Valid values are any positive integer in milliseconds.
        - network.swarm.port: The port for the network swarm, valid range is 1024-65535.
        - network.server.listen: A list of addresses the server will listen on (e.g., "127.0.0.1:8080").
        "#;

        self.output_writer.write_str(hints);
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

        let root_args = ["--home", self.home_dir.as_str(), "--node-name", &self.name];
        let mut command_line = format!("Command: '{}", &self.binary);

        for arg in root_args.into_iter().chain(args) {
            command.arg(arg);
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
