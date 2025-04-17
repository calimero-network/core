use std::process::Command;

fn main() {
    // Get git describe output for build version
    let git_describe = Command::new("git")
        .args(["describe", "--always", "--dirty=-modified", "--tags", "--match", "[0-9]*"])
        .output()
        .ok()
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .unwrap_or_else(|| "unknown".to_string());

    // Get git commit hash
    let git_commit = Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .unwrap_or_else(|| "unknown".to_string());

    // Get rustc version
    let rustc_version = Command::new("rustc")
        .arg("--version")
        .output()
        .ok()
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .map(|s| s.split_whitespace().nth(1).unwrap_or("unknown").to_string())
        .unwrap_or_else(|| "unknown".to_string());

    // Pass individual components to the binary
    println!("cargo:rustc-env=GIT_DESCRIBE={}", git_describe.trim());
    println!("cargo:rustc-env=GIT_COMMIT={}", git_commit.trim());
    println!("cargo:rustc-env=RUSTC_VERSION={}", rustc_version.trim());
} 