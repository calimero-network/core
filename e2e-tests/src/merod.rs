use core::cell::RefCell;

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
