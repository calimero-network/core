use core::cell::RefCell;

use camino::Utf8Path;
use tokio::process::Child;

pub struct Merod {
    process: RefCell<Option<Child>>,
}

impl Merod {
    pub fn new() -> Self {
        Self {
            process: RefCell::new(None),
        }
    }

    pub async fn start(&self, home_dir: &Utf8Path, node_name: &str) -> eyre::Result<()> {
        let mut command = tokio::process::Command::new("merod");
        command
            .arg("--home")
            .arg(home_dir)
            .arg("--node-name")
            .arg(node_name)
            .arg("run");

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
