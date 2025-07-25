use core::cell::RefCell;

use camino::{Utf8Path, Utf8PathBuf};
use tokio::process::Child;

pub struct Merod {
    process: RefCell<Option<Child>>,
    binary_path: Utf8PathBuf,
}

impl Merod {
    pub fn new(binary_path: Utf8PathBuf) -> Self {
        Self {
            process: RefCell::new(None),
            binary_path,
        }
    }

    pub async fn start(
        &self,
        home_dir: &Utf8Path,
        node_name: &str,
        protocol_args: Vec<String>,
    ) -> eyre::Result<()> {
        let mut init_command = tokio::process::Command::new(&self.binary_path);
        init_command
            .arg("--home")
            .arg(home_dir)
            .arg("--node-name")
            .arg(node_name)
            .arg("init")
            .arg("--swarm-port")
            .arg("2427")
            .arg("--server-port")
            .arg("2527");

        let init_status = init_command.status().await?;
        if !init_status.success() {
            return Err(eyre::eyre!("Failed to initialize node {}", node_name));
        }

        let mut command = tokio::process::Command::new(&self.binary_path);
        command
            .arg("--home")
            .arg(home_dir)
            .arg("--node-name")
            .arg(node_name)
            .arg("run");

        // Add protocol-specific args with --protocol-arg flag
        for arg in protocol_args {
            command.arg("--protocol-config").arg(arg);
        }

        let child = command.spawn()?;
        self.process.borrow_mut().replace(child);
        Ok(())
    }

    pub async fn stop(&self) -> eyre::Result<()> {
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
}
