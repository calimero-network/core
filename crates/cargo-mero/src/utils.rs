use std::ffi::OsStr;
use std::process::Stdio;

use tokio::process::Command;

pub fn new_command<S: AsRef<OsStr>>(program: S) -> Command {
    let mut command = Command::new(program);
    let _ = command.stdout(Stdio::inherit()).stderr(Stdio::inherit());
    command
}
