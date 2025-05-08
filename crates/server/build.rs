use std::process::Command;

fn main() {
    let status = Command::new("sh")
        .arg("../../scripts/download-node-ui.sh")
        .status()
        .expect("Failed to execute script");

    if !status.success() {
        panic!("Script failed with status: {:?}", status);
    }
}