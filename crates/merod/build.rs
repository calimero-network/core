use std::env;
use std::process::Command;

fn main() {
    // Get git describe output
    let git_describe = Command::new("git")
        .args([
            "describe",
            "--always",
            "--dirty=-modified",
            "--tags",
            "--match",
            "[0-9]*",
        ])
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

    // Get package version from Cargo.toml
    let pkg_version = env::var("CARGO_PKG_VERSION").unwrap_or_else(|_| "unknown".to_string());

    // Format the complete version string
    let version_string = format!(
        "(release {}) (build {}{}) (commit {}) (rustc {})",
        pkg_version,
        pkg_version,
        if git_describe.contains("-modified") {
            "-modified"
        } else {
            ""
        },
        git_commit.trim(),
        rustc_version.trim()
    );

    // Pass the version string to the binary
    println!("cargo:rustc-env=VERSION_STRING={}", version_string);
}
